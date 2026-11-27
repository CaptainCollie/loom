use super::{App, Mode};
use crate::syntax::themes::{self, Entry, Selection};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};
use syntect::highlighting::Theme;

#[derive(Debug)]
pub(super) struct Catalog {
    requests: Sender<u64>,
    results: Receiver<(u64, Result<Vec<Entry>, String>)>,
    generation: Arc<AtomicU64>,
    directory: PathBuf,
    pub selection: Selection,
    pub theme: Option<Arc<Theme>>,
    restoring: bool,
    warning: Option<String>,
}
impl Catalog {
    pub fn open(directory: PathBuf) -> std::io::Result<Self> {
        let selection = themes::read_selection(&directory);
        let error = selection.as_ref().err().map(ToString::to_string);
        let selection = selection.unwrap_or(Selection::Builtin);
        let (requests, incoming) = mpsc::channel();
        let (outgoing, results) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let state = generation.clone();
        let root = directory.clone();
        std::thread::Builder::new()
            .name("loom-syntax-themes".into())
            .spawn(move || {
                while let Ok(id) = incoming.recv() {
                    let cancelled = || state.load(Ordering::Relaxed) != id;
                    if cancelled() {
                        continue;
                    }
                    let result = themes::entries(&root, &cancelled).map_err(|e| format!("{e:#}"));
                    if !cancelled() && outgoing.send((id, result)).is_err() {
                        break;
                    }
                }
            })?;
        let mut catalog = Self {
            requests,
            results,
            generation,
            directory,
            selection,
            theme: Some(themes::builtin()),
            restoring: true,
            warning: error.clone(),
        };
        if error.is_some() {
            catalog.selection = Selection::Builtin;
        }
        catalog.scan();
        Ok(catalog)
    }
    fn scan(&self) -> u64 {
        let id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.requests.send(id);
        id
    }
    pub(super) fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}
impl Drop for Catalog {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Debug)]
pub(super) struct Picker {
    entries: Vec<Entry>,
    state: ListState,
    generation: u64,
    loading: bool,
    message: Option<String>,
}

impl App {
    pub(super) fn ensure_syntax_catalog(&mut self) {
        if self.syntax_catalog.is_none() {
            match Catalog::open(self.theme_store.directory().to_owned()) {
                Ok(catalog) => self.syntax_catalog = Some(catalog),
                Err(error) => self.error = Some(format!("Cannot load syntax themes: {error}")),
            }
        } else if let Some(catalog) = &self.syntax_catalog
            && catalog.restoring
        {
            catalog.scan();
        }
    }
    pub(super) fn poll_syntax_catalog(&mut self) {
        let Some(catalog) = &mut self.syntax_catalog else {
            return;
        };
        while let Ok((id, result)) = catalog.results.try_recv() {
            if id != catalog.generation.load(Ordering::Relaxed) {
                continue;
            }
            let entries = match result {
                Ok(entries) => entries,
                Err(error) => {
                    catalog.restoring = false;
                    catalog.warning = Some(error.clone());
                    if let Mode::Read(reader) = &mut self.mode {
                        if let Some(picker) = &mut reader.syntax_picker {
                            picker.loading = false;
                            picker.message = Some(error);
                        } else {
                            reader.viewer.syntax_message = Some(error);
                        }
                    }
                    continue;
                }
            };
            if catalog.restoring {
                catalog.restoring = false;
                let choice = entries
                    .iter()
                    .find(|entry| entry.selection == catalog.selection);
                let result = choice.map(|entry| entry.theme.clone()).unwrap_or_else(|| {
                    Err("Saved syntax theme is unavailable; using built-in theme".into())
                });
                match result {
                    Ok(theme) => catalog.theme = theme,
                    Err(error) => {
                        catalog.selection = Selection::Builtin;
                        catalog.theme = Some(themes::builtin());
                        catalog.warning = Some(error);
                    }
                }
                if let Mode::Read(reader) = &mut self.mode {
                    reader.viewer.set_syntax_theme(catalog.theme.clone());
                }
            }
            if let Mode::Read(reader) = &mut self.mode
                && let Some(picker) = &mut reader.syntax_picker
                && picker.generation == id
            {
                picker.loading = false;
                picker.state.select(
                    entries
                        .iter()
                        .position(|entry| entry.selection == catalog.selection)
                        .or(Some(0)),
                );
                picker.entries = entries;
            }
        }
        if let Mode::Read(reader) = &mut self.mode
            && reader.viewer.page.is_some()
            && !reader.viewer.busy
            && let Some(warning) = catalog.warning.take()
        {
            reader.viewer.message = Some(warning);
        }
    }
    pub(super) fn begin_syntax_selection(&mut self) {
        self.ensure_syntax_catalog();
        let Some(catalog) = &self.syntax_catalog else {
            return;
        };
        let Mode::Read(reader) = &mut self.mode else {
            return;
        };
        reader.syntax_picker = Some(Picker {
            entries: Vec::new(),
            state: ListState::default(),
            generation: catalog.scan(),
            loading: true,
            message: None,
        });
    }
    pub(super) fn handle_syntax_key(&mut self, key: KeyEvent) {
        let Mode::Read(reader) = &mut self.mode else {
            return;
        };
        let Some(picker) = &mut reader.syntax_picker else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                reader.syntax_picker = None;
                if let Some(catalog) = &self.syntax_catalog {
                    catalog.cancel();
                }
            }
            KeyCode::Up | KeyCode::Down => {
                if picker.entries.is_empty() {
                    return;
                }
                let index = picker.state.selected().unwrap_or(0);
                picker.state.select(Some(if key.code == KeyCode::Up {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(picker.entries.len() - 1)
                }));
                picker.message = None;
            }
            KeyCode::Enter if !picker.loading => {
                let Some(entry) = picker
                    .state
                    .selected()
                    .and_then(|index| picker.entries.get(index))
                else {
                    return;
                };
                match &entry.theme {
                    Ok(theme) => {
                        reader.viewer.set_syntax_theme(theme.clone());
                        if let Some(catalog) = &mut self.syntax_catalog {
                            catalog.theme = theme.clone();
                            catalog.selection = entry.selection.clone();
                            catalog.restoring = false;
                            if let Err(error) =
                                themes::save_selection(&catalog.directory, &catalog.selection)
                            {
                                reader.viewer.message = Some(format!(
                                    "Theme applied but selection could not be saved: {error}"
                                ));
                            }
                        }
                        reader.syntax_picker = None;
                    }
                    Err(error) => picker.message = Some(error.clone()),
                }
            }
            _ => {}
        }
    }
    pub(super) fn draw_syntax_picker(&mut self, frame: &mut Frame, content: Rect) {
        let Mode::Read(reader) = &mut self.mode else {
            return;
        };
        let Some(picker) = &mut reader.syntax_picker else {
            return;
        };
        let width = content.width.min(90);
        let height = content
            .height
            .min((picker.entries.len() + 5).min(u16::MAX as usize) as u16)
            .max(content.height.min(7));
        let area = Rect::new(
            content.x + (content.width - width) / 2,
            content.y + (content.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, area);
        let block = Block::bordered().title(" Syntax themes ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width < 12 || inner.height < 3 {
            frame.render_widget(Paragraph::new("Enlarge window. Esc cancels"), inner);
            return;
        }
        let [list, status] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
        let items = picker.entries.iter().map(|entry| {
            ListItem::new(entry.label.as_str()).style(if entry.theme.is_ok() {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            })
        });
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("> ")
                .highlight_style(Style::default().bg(Color::White).fg(Color::Black)),
            list,
            &mut picker.state,
        );
        let message = picker.message.as_deref().unwrap_or(if picker.loading {
            "Loading VSIX color themes… Esc cancels."
        } else {
            "Enter applies; Esc cancels. Reopen to refresh."
        });
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: false }), status);
    }
}
