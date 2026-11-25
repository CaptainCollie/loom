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
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState},
};

#[derive(Debug)]
pub(crate) struct App {
    base_dir: PathBuf,
    expanded: HashSet<PathBuf>,
    visible: Vec<TreeEntry>,
    list_state: ListState,
    exit: bool,
    error: Option<String>,
}

#[derive(Debug)]
struct TreeEntry {
    path: PathBuf,
    display_line: Line<'static>,
    is_dir: bool,
}

impl App {
    pub(crate) fn new(base_dir: PathBuf) -> io::Result<Self> {
        let mut app = Self {
            expanded: HashSet::from([base_dir.clone()]),
            base_dir,
            visible: Vec::new(),
            list_state: ListState::default(),
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
                self.error = self.rebuild_visible().err().map(|error| error.to_string());

                last_refresh = Instant::now();
            }
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
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

        frame.render_stateful_widget(list, frame.area(), &mut self.list_state);
    }

    fn handle_events(&mut self) -> io::Result<()> {
        if event::poll(Duration::from_millis(100))?
            && let Some(key) = event::read()?.as_key_press_event()
        {
            self.handle_key_event(key);
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if matches!(
            key_event.code,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
        ) {
            self.error = None;
        }
        match key_event.code {
            KeyCode::Char('c' | 'C') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.exit();
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => self.exit(),
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
                self.list_state.select_next();
            }
            KeyCode::Up => {
                self.list_state.select_previous();
            }
            _ => {}
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }
}
