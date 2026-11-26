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
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::create_structure;
use crate::files::{create_directory, create_file, delete_entry};

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
    refresh_error: Option<String>,
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
    Create {
        kind: CreateKind,
        name: String,
        directory: PathBuf,
    },
    ConfirmDelete {
        path: PathBuf,
        is_dir: bool,
        index: usize,
        confirm: bool,
    },
}

#[derive(Debug, Clone, Copy)]
enum CreateKind {
    File,
    Directory,
    Structure,
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
            refresh_error: None,
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
                self.refresh_tree();

                last_refresh = Instant::now();
            }
        }
        Ok(())
    }

    fn refresh_tree(&mut self) {
        self.refresh_error = self.rebuild_visible().err().map(|error| error.to_string());
    }

    fn screen_layout(&self, screen: Rect) -> [Rect; 3] {
        let footer_height = u16::try_from(self.key_hints(screen.width).len()).unwrap_or(u16::MAX);
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .areas(screen)
    }

    fn delete_dialog(&self, screen: Rect) -> Option<(Rect, Vec<Line<'static>>)> {
        let Mode::ConfirmDelete { path, is_dir, .. } = &self.mode else {
            return None;
        };
        let content = self.screen_layout(screen)[1];
        let width = content.width.min(90);
        // Both buttons must fit in full, in addition to the complete warning and path.
        if width < 21 {
            return None;
        }
        let warning = if *is_dir {
            "The directory and ALL its contents will be deleted. No undo."
        } else {
            "This entry will be permanently deleted. No undo."
        };
        let mut lines = Vec::new();
        for (text, style) in [
            (path.display().to_string(), Style::default()),
            (String::new(), Style::default()),
            (warning.to_owned(), Style::default().fg(Color::Red)),
        ] {
            let mut line = String::new();
            let mut used = 0;
            for ch in text.chars() {
                let cells = ch.width().unwrap_or(0);
                if used + cells > usize::from(width - 2) {
                    lines.push(Line::styled(std::mem::take(&mut line), style));
                    used = 0;
                }
                line.push(ch);
                used += cells;
            }
            lines.push(Line::styled(line, style));
        }
        let height = u16::try_from(lines.len().checked_add(3)?).ok()?;
        if height > content.height {
            return None;
        }
        Some((
            Rect::new(
                content.x + (content.width - width) / 2,
                content.y + (content.height - height) / 2,
                width,
                height,
            ),
            lines,
        ))
    }

    fn draw(&mut self, frame: &mut Frame) {
        let hints = self.key_hints(frame.area().width);
        let [header_area, content_area, footer_area] = self.screen_layout(frame.area());
        frame.render_widget(Line::from("loom".cyan().bold()).centered(), header_area);
        frame.render_widget(Paragraph::new(hints), footer_area);

        let input_height = match &self.mode {
            Mode::Navigation | Mode::ConfirmDelete { .. } => 0,
            Mode::Create { .. } => 4,
        };

        let [tree_area, input_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(input_height)])
                .areas(content_area);

        let items: Vec<ListItem> = self
            .visible
            .iter()
            .map(|entry| ListItem::new(entry.display_line.clone()))
            .collect();

        let mut block = Block::bordered().title(self.base_dir.display().to_string());
        if let Some(error) = self.error.as_ref().or(self.refresh_error.as_ref()) {
            block = block.title_bottom(Line::from(error.as_str()).red());
        }
        let list = List::new(items)
            .block(block)
            .highlight_symbol("> ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(list, tree_area, &mut self.list_state);

        if let Mode::Create {
            kind,
            name,
            directory,
        } = &self.mode
        {
            let title = match kind {
                CreateKind::File => " Create new file ",
                CreateKind::Directory => " Create new dir ",
                CreateKind::Structure => " Create structure ",
            };
            let block = Block::bordered().title(title);

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
        if matches!(self.mode, Mode::ConfirmDelete { .. }) {
            let Some((area, lines)) = self.delete_dialog(frame.area()) else {
                if let Mode::ConfirmDelete { confirm, .. } = &mut self.mode {
                    *confirm = false;
                }
                frame.render_widget(Clear, content_area);
                frame.render_widget(
                    Paragraph::new("Enlarge the window to review deletion. Esc - cancel")
                        .wrap(Wrap { trim: false })
                        .yellow(),
                    content_area,
                );
                return;
            };
            let confirm = matches!(self.mode, Mode::ConfirmDelete { confirm: true, .. });
            let block = Block::bordered().title(" Delete permanently? ");
            let inner = block.inner(area);
            frame.render_widget(Clear, area);
            frame.render_widget(block, area);
            let [message_area, buttons_area] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
            frame.render_widget(Paragraph::new(lines), message_area);
            let selected = Style::default().add_modifier(Modifier::REVERSED);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        " Cancel ",
                        if confirm { Style::default() } else { selected },
                    ),
                    Span::raw("   "),
                    Span::styled(
                        " Delete ",
                        if confirm { selected } else { Style::default() },
                    ),
                ])),
                buttons_area,
            );
        }
    }

    fn key_hints(&self, width: u16) -> Vec<Line<'static>> {
        let actions: &[(&str, &str)] = match self.mode {
            Mode::Navigation => &[
                ("Up/Down", "move"),
                ("Left/Right", "collapse/expand"),
                ("f", "file"),
                ("d", "directory"),
                ("s", "structure"),
                ("Backspace", "delete"),
                ("q", "quit"),
                ("Ctrl+C", "quit"),
            ],
            Mode::Create { .. } => &[
                ("Enter", "create"),
                ("Esc", "cancel"),
                ("Left/Right", "cursor"),
                ("Backspace", "erase"),
                ("Ctrl+C", "quit"),
            ],
            Mode::ConfirmDelete { .. } => &[
                ("Left/Right", "choose"),
                ("Enter", "confirm"),
                ("Esc", "cancel"),
                ("Ctrl+C", "quit"),
            ],
        };
        let mut lines = Vec::new();
        let mut spans = Vec::new();
        let mut used = 0;
        for &(key, description) in actions {
            let action_width = key.width() + 1 + description.width();
            if !spans.is_empty() && used + 3 + action_width > usize::from(width) {
                lines.push(Line::from(std::mem::take(&mut spans)));
                used = 0;
            }
            if !spans.is_empty() {
                spans.push(Span::raw("   "));
                used += 3;
            }
            spans.push(Span::styled(key, Style::default().fg(Color::Yellow).bold()));
            spans.push(Span::styled(
                format!(" {description}"),
                Style::default().fg(Color::Gray),
            ));
            used += action_width;
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
        lines
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
        match &self.mode {
            Mode::Create { name, .. } => name
                .char_indices()
                .map(|(i, _)| i)
                .nth(self.character_index)
                .unwrap_or(name.len()),
            _ => 0,
        }
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        match &self.mode {
            Mode::Create { name, .. } => new_cursor_pos.clamp(0, name.chars().count()),
            _ => 0,
        }
    }

    const fn reset_cursor(&mut self) {
        self.character_index = 0;
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

    // --------------------------------------------------------

    fn handle_key_event(&mut self, key_event: KeyEvent, screen: Rect) {
        if matches!(key_event.code, KeyCode::Char('c' | 'C'))
            && key_event.modifiers.contains(KeyModifiers::CONTROL)
            && !key_event.modifiers.contains(KeyModifiers::ALT)
        {
            self.exit();
            return;
        }
        let text_input = matches!(self.mode, Mode::Create { .. })
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
        let acknowledges_error = match self.mode {
            Mode::Navigation => matches!(
                key_event.code,
                KeyCode::Char('q' | 'Q' | 'f' | 'd' | 's')
                    | KeyCode::Backspace
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
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
        };
        if acknowledges_error {
            self.error = None;
        }
        match &self.mode {
            Mode::Navigation => match key_event.code {
                KeyCode::Char('q') | KeyCode::Char('Q') => self.exit(),
                KeyCode::Backspace => {
                    if let Some(entry) = self.selected_entry() {
                        if entry.path == self.base_dir {
                            self.error = Some("Cannot delete the tree root".into());
                        } else {
                            self.mode = Mode::ConfirmDelete {
                                path: entry.path.clone(),
                                is_dir: entry.is_dir,
                                index: self.list_state.selected().unwrap_or(0),
                                confirm: false,
                            };
                            self.error = None;
                        }
                    }
                }
                KeyCode::Char(key @ ('f' | 'd' | 's')) => {
                    if let Some(entry) = self.selected_entry() {
                        let directory = if entry.is_dir {
                            Some(entry.path.clone())
                        } else {
                            entry.path.parent().map(Path::to_path_buf)
                        };
                        if let Some(directory) = directory {
                            self.reset_cursor();
                            self.error = None;
                            self.mode = Mode::Create {
                                kind: match key {
                                    'f' => CreateKind::File,
                                    'd' => CreateKind::Directory,
                                    _ => CreateKind::Structure,
                                },
                                name: String::new(),
                                directory,
                            };
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
            Mode::ConfirmDelete {
                path,
                index,
                confirm,
                ..
            } => match key_event.code {
                KeyCode::Esc => self.mode = Mode::Navigation,
                KeyCode::Left | KeyCode::Right => {
                    if let Mode::ConfirmDelete { confirm, .. } = &mut self.mode {
                        *confirm = key_event.code == KeyCode::Right;
                    }
                }
                KeyCode::Enter => {
                    if *confirm {
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
                    } else {
                        self.mode = Mode::Navigation;
                    }
                }
                _ => {}
            },
            Mode::Create {
                kind,
                name,
                directory,
            } => match key_event.code {
                KeyCode::Enter => {
                    let result = match kind {
                        CreateKind::File => create_file(directory, name),
                        CreateKind::Directory => create_directory(directory, name),
                        CreateKind::Structure => {
                            if name.trim().is_empty() {
                                Err(anyhow::anyhow!("Structure pattern must not be empty"))
                            } else {
                                create_structure(directory, name, false).map(|()| directory.clone())
                            }
                        }
                    };
                    match result {
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
                                    self.error = Some(format!(
                                        "Entry created, but tree refresh failed: {error}"
                                    ));
                                }
                            }
                        }
                        Err(error) => {
                            let message = format!("{error:#}");
                            if matches!(kind, CreateKind::Structure) {
                                // A filesystem error may leave part of the structure created.
                                if let Err(refresh_error) = self.rebuild_visible() {
                                    self.error = Some(format!(
                                        "{message}; tree refresh failed: {refresh_error}"
                                    ));
                                } else {
                                    self.error = Some(message);
                                }
                            } else {
                                self.error = Some(message);
                            }
                        }
                    }
                }
                KeyCode::Char(to_insert) if !to_insert.is_control() => self.enter_char(to_insert),
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
