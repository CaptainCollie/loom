mod folder_icons;
mod icons;
mod input;
mod reader;
mod svg_color;
mod syntax_picker;
mod theme;
mod theme_picker;
mod theme_store;
mod tree;
mod ui;
mod vsix;
use std::{
    collections::HashSet,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{DefaultTerminal, layout::Rect, widgets::ListState};

use crate::create_structure;
use crate::files::{create_directory, create_file, delete_entry};
use input::TextInput;
use theme::IconTheme;
use theme_store::{ThemeId, ThemeStartup, ThemeStore};
use tree::TreeEntry;

#[derive(Debug)]
pub(crate) struct App {
    base_dir: PathBuf,
    expanded: HashSet<PathBuf>,
    visible: Vec<TreeEntry>,
    list_state: ListState,
    mode: Mode,
    exit: bool,
    error: Option<String>,
    refresh_error: Option<String>,
    theme_store: ThemeStore,
    icon_theme: IconTheme,
    selected_theme: Option<ThemeId>,
    syntax_catalog: Option<syntax_picker::Catalog>,
}

#[derive(Debug)]
enum Mode {
    Navigation,
    Create {
        kind: CreateKind,
        input: TextInput,
        directory: PathBuf,
    },
    ConfirmDelete {
        path: PathBuf,
        is_dir: bool,
        index: usize,
        confirm: bool,
    },
    Read(Box<reader::Reader>),
    ThemePicker(theme_picker::ThemePicker),
}

#[derive(Debug, Clone, Copy)]
enum CreateKind {
    File,
    Directory,
    Structure,
}

impl App {
    pub(crate) fn new(base_dir: PathBuf) -> anyhow::Result<Self> {
        Ok(Self::with_themes(base_dir, ThemeStore::open()?)?)
    }

    fn with_themes(base_dir: PathBuf, themes: ThemeStartup) -> io::Result<Self> {
        let mut app = Self {
            expanded: HashSet::from([base_dir.clone()]),
            base_dir,
            visible: Vec::new(),
            list_state: ListState::default(),
            mode: Mode::Navigation,
            exit: false,
            error: themes.warning,
            refresh_error: None,
            theme_store: themes.store,
            icon_theme: themes.theme,
            selected_theme: themes.selected,
            syntax_catalog: None,
        };
        app.rebuild_visible()?;
        Ok(app)
    }

    pub(crate) fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let refresh_interval = Duration::from_secs(1);
        let mut last_refresh = Instant::now();

        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.poll_reader();
            self.poll_themes();
            self.handle_events()?;

            if self.exit {
                break;
            }

            if last_refresh.elapsed() >= refresh_interval && !matches!(self.mode, Mode::Read(_)) {
                self.refresh_tree();

                last_refresh = Instant::now();
            }
        }
        Ok(())
    }

    fn handle_events(&mut self) -> io::Result<()> {
        if event::poll(Duration::from_millis(100))?
            && let Some(key) = event::read()?.as_key_press_event()
        {
            let (width, height) = crossterm::terminal::size()?;
            self.handle_key_event(key, Rect::new(0, 0, width, height));
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent, screen: Rect) {
        if matches!(key_event.code, KeyCode::Char('c' | 'C'))
            && key_event.modifiers.contains(KeyModifiers::CONTROL)
            && !key_event.modifiers.contains(KeyModifiers::ALT)
        {
            self.exit();
            return;
        }
        let text_input = (matches!(self.mode, Mode::Create { .. })
            || matches!(&self.mode, Mode::Read(reader) if reader.prompt.is_some()))
            && matches!(key_event.code, KeyCode::Char(ch) if !ch.is_control())
            && (!key_event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                || key_event
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::ALT));
        if key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && !text_input
        {
            return;
        }
        if matches!(self.mode, Mode::ConfirmDelete { .. }) && self.delete_dialog(screen).is_none() {
            if let Mode::ConfirmDelete { confirm, .. } = &mut self.mode {
                *confirm = false;
            }
            if !matches!(key_event.code, KeyCode::Esc) {
                return;
            }
        }
        let acknowledges_error = match &self.mode {
            Mode::Navigation => matches!(
                key_event.code,
                KeyCode::Char('q' | 'Q' | 'f' | 'd' | 's' | 't')
                    | KeyCode::Backspace
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Enter
            ),
            Mode::Create { .. } => matches!(
                key_event.code,
                KeyCode::Char(_)
                    | KeyCode::Backspace
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Enter
                    | KeyCode::Esc
            ),
            Mode::ConfirmDelete { .. } => matches!(
                key_event.code,
                KeyCode::Left | KeyCode::Right | KeyCode::Enter | KeyCode::Esc
            ),
            Mode::Read(_) => {
                matches!(key_event.code, KeyCode::Up | KeyCode::Down | KeyCode::Esc)
            }
            Mode::ThemePicker(_) => false,
        };
        if acknowledges_error {
            self.error = None;
        }
        match &self.mode {
            Mode::Navigation => self.handle_navigation_key(key_event),
            Mode::Create { .. } => self.handle_create_key(key_event),
            Mode::ConfirmDelete { .. } => self.handle_delete_key(key_event, screen),
            Mode::Read(_) => self.handle_read_key(key_event),
            Mode::ThemePicker(_) => self.handle_theme_key(key_event),
        }
    }

    fn handle_navigation_key(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.exit(),
            KeyCode::Backspace => self.begin_deletion(),
            KeyCode::Char('f') => self.begin_creation(CreateKind::File),
            KeyCode::Char('d') => self.begin_creation(CreateKind::Directory),
            KeyCode::Char('s') => self.begin_creation(CreateKind::Structure),
            KeyCode::Char('t') => self.begin_theme_selection(),
            KeyCode::Left => {
                if let Some((path, is_dir)) =
                    self.selected_entry().map(|e| (e.path.clone(), e.is_dir))
                {
                    if is_dir && self.expanded.remove(&path) {
                        if let Err(error) = self.rebuild_visible() {
                            self.expanded.insert(path);
                            self.error = Some(error.to_string());
                        }
                    } else if let Some(parent) = path.parent()
                        && let Some(parent_idx) = self.visible.iter().position(|e| e.path == parent)
                    {
                        self.list_state.select(Some(parent_idx));
                    }
                }
            }
            KeyCode::Right => {
                if let Some((path, true)) =
                    self.selected_entry().map(|e| (e.path.clone(), e.is_dir))
                {
                    if self.expanded.insert(path.clone()) {
                        if let Err(error) = self.rebuild_visible() {
                            self.expanded.remove(&path);
                            self.error = Some(error.to_string());
                        }
                    } else if let Some(index) = self.list_state.selected()
                        && self
                            .visible
                            .get(index + 1)
                            .is_some_and(|entry| entry.path.parent() == Some(path.as_path()))
                    {
                        self.list_state.select(Some(index + 1));
                    }
                }
            }
            KeyCode::Down => {
                if !self.visible.is_empty() {
                    let next = self
                        .list_state
                        .selected()
                        .map_or(0, |index| index.saturating_add(1))
                        .min(self.visible.len() - 1);
                    self.list_state.select(Some(next));
                }
            }
            KeyCode::Up => {
                self.list_state.select_previous();
            }
            KeyCode::Enter => self.open_reader(),
            _ => {}
        }
    }

    fn begin_creation(&mut self, kind: CreateKind) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let directory = if entry.is_dir {
            Some(entry.path.clone())
        } else {
            entry.path.parent().map(std::path::Path::to_path_buf)
        };
        if let Some(directory) = directory {
            self.error = None;
            self.mode = Mode::Create {
                kind,
                input: TextInput::default(),
                directory,
            };
        }
    }

    fn begin_deletion(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if entry.path == self.base_dir {
            self.error = Some("Cannot delete the tree root".into());
            return;
        }
        self.mode = Mode::ConfirmDelete {
            path: entry.path.clone(),
            is_dir: entry.is_dir,
            index: self.list_state.selected().unwrap_or(0),
            confirm: false,
        };
        self.error = None;
    }

    fn handle_create_key(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Enter => self.submit_creation(),
            KeyCode::Esc => {
                self.mode = Mode::Navigation;
                self.error = None;
            }
            _ => {
                let Mode::Create { input, .. } = &mut self.mode else {
                    return;
                };
                match key_event.code {
                    KeyCode::Char(ch) if !ch.is_control() => input.insert(ch),
                    KeyCode::Backspace => input.backspace(),
                    KeyCode::Left => input.move_left(),
                    KeyCode::Right => input.move_right(),
                    _ => {}
                }
            }
        }
    }

    fn handle_delete_key(&mut self, key_event: KeyEvent, _screen: Rect) {
        match key_event.code {
            KeyCode::Esc => self.mode = Mode::Navigation,
            KeyCode::Left | KeyCode::Right => {
                if let Mode::ConfirmDelete { confirm, .. } = &mut self.mode {
                    *confirm = key_event.code == KeyCode::Right;
                }
            }
            KeyCode::Enter => {
                if matches!(self.mode, Mode::ConfirmDelete { confirm: true, .. }) {
                    self.confirm_deletion();
                } else {
                    self.mode = Mode::Navigation;
                }
            }
            _ => {}
        }
    }

    fn submit_creation(&mut self) {
        let Mode::Create {
            kind,
            input,
            directory,
        } = &self.mode
        else {
            return;
        };
        let kind = *kind;
        let directory = directory.clone();
        let name = input.text();
        let result = match kind {
            CreateKind::File => create_file(&directory, name),
            CreateKind::Directory => create_directory(&directory, name),
            CreateKind::Structure => {
                if name.trim().is_empty() {
                    Err(anyhow::anyhow!("Structure pattern must not be empty"))
                } else {
                    create_structure(&directory, name, false).map(|()| directory.clone())
                }
            }
        };
        match result {
            Ok(path) => {
                self.mode = Mode::Navigation;
                self.error = None;
                self.expanded.insert(directory);
                match self.rebuild_visible() {
                    Ok(()) => {
                        let index = self.visible.iter().position(|entry| entry.path == path);
                        if let Some(index) = index {
                            self.list_state.select(Some(index));
                        }
                    }
                    Err(error) => {
                        self.error =
                            Some(format!("Entry created, but tree refresh failed: {error}"));
                    }
                }
            }
            Err(error) => {
                let message = format!("{error:#}");
                if matches!(kind, CreateKind::Structure) {
                    // A filesystem error may leave part of the structure created.
                    if let Err(refresh_error) = self.rebuild_visible() {
                        self.error =
                            Some(format!("{message}; tree refresh failed: {refresh_error}"));
                    } else {
                        self.error = Some(message);
                    }
                } else {
                    self.error = Some(message);
                }
            }
        }
    }

    fn confirm_deletion(&mut self) {
        let Mode::ConfirmDelete { path, index, .. } = &self.mode else {
            return;
        };
        let path = path.clone();
        let index = *index;
        let result = delete_entry(&self.base_dir, &path);
        let deleted = result.is_ok();
        self.error = result.err().map(|error| format!("{error:#}"));
        if deleted {
            self.expanded.retain(|entry| !entry.starts_with(&path));
        }
        self.mode = Mode::Navigation;
        match self.rebuild_visible() {
            Ok(()) if deleted => {
                self.list_state
                    .select(Some(index.min(self.visible.len() - 1)));
            }
            Ok(()) => {}
            Err(error) => {
                let refresh_error = format!("Tree refresh failed: {error}");
                self.error = Some(match self.error.take() {
                    Some(message) => format!("{message}; {refresh_error}"),
                    None => refresh_error,
                });
            }
        }
    }

    fn exit(&mut self) {
        self.theme_store.cancel();
        if let Some(catalog) = &self.syntax_catalog {
            catalog.cancel();
        }
        if let Mode::Read(reader) = &mut self.mode {
            reader.viewer.cancel();
        }
        self.exit = true;
    }
}
