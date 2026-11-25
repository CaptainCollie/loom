use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crossterm::event::{
    self,
    KeyCode::{self},
    KeyEvent, KeyModifiers,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Position},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph},
};

use unicode_width::UnicodeWidthStr;

use crate::files::create_file;

#[derive(Debug)]
pub(crate) struct App {
    base_dir: PathBuf,
    expanded: HashSet<PathBuf>,
    visible: Vec<TreeEntry>,
    list_state: ListState,
    character_index: usize,
    mode: Mode,
    exit: bool,
    error: Option<String>,
}

#[derive(Debug)]
struct TreeEntry {
    path: PathBuf,
    display_line: Line<'static>,
    is_dir: bool,
}

#[derive(Debug)]
enum Mode {
    Navigation,
    Create { name: String, directory: PathBuf },
}

impl App {
    pub(crate) fn new(base_dir: PathBuf) -> io::Result<Self> {
        let mut app = Self {
            expanded: HashSet::from([base_dir.clone()]),
            base_dir,
            visible: Vec::new(),
            list_state: ListState::default(),
            character_index: 0,
            mode: Mode::Navigation,
            exit: false,
            error: None,
        };
        app.rebuild_visible()?;
        Ok(app)
    }

    fn rebuild_visible(&mut self) -> io::Result<()> {
        let selected_path = self.selected_entry().map(|entry| entry.path.clone());
        let mut visible = vec![TreeEntry {
            path: self.base_dir.clone(),
            display_line: Line::from(" .".blue().bold()),
            is_dir: true,
        }];

        if self.expanded.contains(&self.base_dir) {
            self.scan_dir(&self.base_dir, "", &mut visible)?;
        }
        let mut selected = selected_path.as_deref();
        let index = loop {
            match selected {
                Some(path) => {
                    if let Some(index) = visible.iter().position(|entry| entry.path == path) {
                        break index;
                    }
                    selected = path.parent();
                }
                None => break 0,
            }
        };
        self.visible = visible;
        self.list_state.select(Some(index));
        Ok(())
    }

    fn scan_dir(&self, dir: &Path, prefix: &str, visible: &mut Vec<TreeEntry>) -> io::Result<()> {
        let mut entries = fs::read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;

        entries.sort_by_key(|e| e.file_name());

        let count = entries.len();
        for (idx, entry) in entries.into_iter().enumerate() {
            let is_last = idx == count - 1;
            let path = entry.path();
            let is_dir = entry.metadata()?.is_dir();
            let is_expanded = self.expanded.contains(&path);

            let raw_name = entry.file_name().to_string_lossy().into_owned();
            let styled_name = if is_dir {
                Span::from(format!(" {raw_name}")).blue().bold()
            } else {
                Span::from(raw_name).gray()
            };

            let branch = if is_last { "└── " } else { "├── " };
            let tree_prefix = Span::from(format!("{prefix}{branch}"));

            visible.push(TreeEntry {
                path: path.clone(),
                display_line: Line::from(vec![tree_prefix, styled_name]),
                is_dir,
            });

            if is_dir && is_expanded {
                let next_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
                self.scan_dir(&path, &next_prefix, visible)?;
            }
        }
        Ok(())
    }

    fn selected_entry(&self) -> Option<&TreeEntry> {
        self.list_state
            .selected()
            .and_then(|index| self.visible.get(index))
    }

    pub(crate) fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let refresh_interval = Duration::from_secs(1);
        let mut last_refresh = Instant::now();

        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;

            if self.exit {
                break;
            }

            if last_refresh.elapsed() >= refresh_interval {
                let refresh_error = self.rebuild_visible().err().map(|error| error.to_string());
                if matches!(self.mode, Mode::Navigation) || self.error.is_none() {
                    self.error = refresh_error;
                }

                last_refresh = Instant::now();
            }
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let input_height = match &self.mode {
            Mode::Navigation => 0,
            Mode::Create { .. } => 4,
        };

        let [tree_area, input_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(input_height)])
                .areas(frame.area());

        let items: Vec<ListItem> = self
            .visible
            .iter()
            .map(|entry| ListItem::new(entry.display_line.clone()))
            .collect();

        let mut block = Block::bordered().title(" loom ");
        if let Some(error) = &self.error {
            block = block.title_bottom(Line::from(error.as_str()).red());
        }
        let list = List::new(items)
            .block(block)
            .highlight_symbol("> ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(list, tree_area, &mut self.list_state);

        if let Mode::Create { name, directory } = &self.mode {
            let block = Block::bordered()
                .title(" Create file ")
                .title_bottom(" Enter - create | Esc - cancel ");
            let inner = block.inner(input_area);
            frame.render_widget(block.style(Style::default().fg(Color::Yellow)), input_area);
            let [directory_area, name_area] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
            frame.render_widget(
                Paragraph::new(format!("Current dir: {}", directory.display())).blue(),
                directory_area,
            );
            if name_area.width > 0 && name_area.height > 0 {
                let cursor_byte = self.byte_index();
                let before_cursor = &name[..cursor_byte];
                let max_cursor_x = usize::from(name_area.width - 1);
                let mut start = 0;
                for (index, _) in before_cursor.char_indices() {
                    start = index;
                    if before_cursor[start..].width() <= max_cursor_x {
                        break;
                    }
                    start = cursor_byte;
                }
                let cursor_x = before_cursor[start..].width();
                frame.render_widget(Paragraph::new(&name[start..]).blue(), name_area);
                frame
                    .set_cursor_position(Position::new(name_area.x + cursor_x as u16, name_area.y));
            }
        }
    }

    // ----------------------- INPUT -----------------------

    fn delete_char(&mut self) {
        if let Mode::Create { name, .. } = &mut self.mode {
            let is_not_cursor_leftmost = self.character_index != 0;
            if is_not_cursor_leftmost {
                let current_index = self.character_index;
                let from_left_to_current_index = current_index - 1;

                let before_char_to_delete = name.chars().take(from_left_to_current_index);
                let after_char_to_delete = name.chars().skip(current_index);

                *name = before_char_to_delete.chain(after_char_to_delete).collect();
                self.move_cursor_left();
            }
        }
    }

    fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.character_index.saturating_sub(1);
        self.character_index = self.clamp_cursor(cursor_moved_left);
    }

    fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.character_index.saturating_add(1);
        self.character_index = self.clamp_cursor(cursor_moved_right);
    }

    fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        if let Mode::Create { name, .. } = &mut self.mode {
            name.insert(index, new_char);
            self.move_cursor_right();
        }
    }

    fn byte_index(&self) -> usize {
        if let Mode::Create { name, .. } = &self.mode {
            name.char_indices()
                .map(|(i, _)| i)
                .nth(self.character_index)
                .unwrap_or(name.len())
        } else {
            0
        }
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        if let Mode::Create { name, .. } = &self.mode {
            new_cursor_pos.clamp(0, name.chars().count())
        } else {
            0
        }
    }

    const fn reset_cursor(&mut self) {
        self.character_index = 0;
    }

    fn handle_events(&mut self) -> io::Result<()> {
        if event::poll(Duration::from_millis(100))?
            && let Some(key) = event::read()?.as_key_press_event()
        {
            self.handle_key_event(key);
        }
        Ok(())
    }

    // --------------------------------------------------------

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if matches!(key_event.code, KeyCode::Char('c' | 'C'))
            && key_event.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.exit();
            return;
        }
        if key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        if matches!(self.mode, Mode::Navigation)
            && matches!(
                key_event.code,
                KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
            )
        {
            self.error = None;
        }
        match &self.mode {
            Mode::Navigation => match key_event.code {
                KeyCode::Char('q') | KeyCode::Char('Q') => self.exit(),
                KeyCode::Char('f') => {
                    if let Some(entry) = self.selected_entry() {
                        let dir = if entry.is_dir {
                            Some(entry.path.clone())
                        } else {
                            entry.path.parent().map(Path::to_path_buf)
                        };

                        if let Some(dir) = dir {
                            self.reset_cursor();
                            self.error = None;
                            self.mode = Mode::Create {
                                name: String::new(),
                                directory: dir,
                            }
                        }
                    }
                }
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
                            && let Some(parent_idx) =
                                self.visible.iter().position(|e| e.path == parent)
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
                    self.list_state.select_next();
                }
                KeyCode::Up => {
                    self.list_state.select_previous();
                }
                _ => {}
            },
            Mode::Create { name, directory } => match key_event.code {
                KeyCode::Enter => match create_file(directory, name) {
                    Ok(path) => {
                        let directory = directory.clone();
                        self.mode = Mode::Navigation;
                        self.reset_cursor();
                        self.error = None;
                        self.expanded.insert(directory);
                        match self.rebuild_visible() {
                            Ok(()) => {
                                let index =
                                    self.visible.iter().position(|entry| entry.path == path);
                                if let Some(index) = index {
                                    self.list_state.select(Some(index));
                                }
                            }
                            Err(error) => {
                                self.error =
                                    Some(format!("File created, but tree refresh failed: {error}"));
                            }
                        }
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                },
                KeyCode::Char(to_insert) => self.enter_char(to_insert),
                KeyCode::Backspace => self.delete_char(),
                KeyCode::Left => self.move_cursor_left(),
                KeyCode::Right => self.move_cursor_right(),
                KeyCode::Esc => {
                    self.mode = Mode::Navigation;
                    self.reset_cursor();
                    self.error = None;
                }
                _ => {}
            },
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }
}
