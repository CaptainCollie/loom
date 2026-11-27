use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use super::{App, Mode, theme_store::ThemeEntry};

#[derive(Debug)]
pub(super) struct ThemePicker {
    entries: Vec<ThemeEntry>,
    state: ListState,
    message: Option<String>,
    generation: u64,
    loading: bool,
}

impl App {
    pub(super) fn begin_theme_selection(&mut self) {
        let generation = match self.theme_store.request_scan() {
            Ok(generation) => generation,
            Err(error) => {
                self.error = Some(format!("Failed to list icon themes: {error:#}"));
                return;
            }
        };
        self.mode = Mode::ThemePicker(ThemePicker {
            entries: Vec::new(),
            state: ListState::default(),
            message: None,
            generation,
            loading: true,
        });
    }

    pub(super) fn poll_themes(&mut self) {
        while let Some(result) = self.theme_store.poll() {
            let Mode::ThemePicker(picker) = &mut self.mode else {
                continue;
            };
            if result.generation != picker.generation {
                continue;
            }
            picker.loading = false;
            match result.entries {
                Ok(entries) => {
                    let selected = entries
                        .iter()
                        .position(|entry| Some(&entry.id) == self.selected_theme.as_ref())
                        .or_else(|| entries.iter().position(|entry| entry.theme.is_ok()));
                    picker.entries = entries;
                    picker.state.select(selected);
                }
                Err(error) => picker.message = Some(error),
            }
        }
    }

    pub(super) fn handle_theme_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.theme_store.cancel();
                self.mode = Mode::Navigation;
            }
            KeyCode::Enter => self.apply_selected_theme(),
            KeyCode::Up | KeyCode::Down => {
                let Mode::ThemePicker(picker) = &mut self.mode else {
                    return;
                };
                if !picker.entries.is_empty() {
                    let index = picker.state.selected().unwrap_or(0);
                    let index = if key.code == KeyCode::Up {
                        index.saturating_sub(1)
                    } else {
                        index.saturating_add(1).min(picker.entries.len() - 1)
                    };
                    picker.state.select(Some(index));
                    picker.message = None;
                }
            }
            _ => {}
        }
    }

    fn apply_selected_theme(&mut self) {
        let Mode::ThemePicker(picker) = &mut self.mode else {
            return;
        };
        if picker.loading {
            return;
        }
        let Some(entry) = picker.state.selected().and_then(|i| picker.entries.get(i)) else {
            picker.message = Some("No icon themes available".into());
            return;
        };
        let theme = match &entry.theme {
            Ok(theme) => theme.as_ref().clone(),
            Err(error) => {
                picker.message = Some(error.clone());
                return;
            }
        };
        let id = entry.id.clone();
        let warning = theme.warning.clone();
        let previous = std::mem::replace(&mut self.icon_theme, theme);
        if let Err(error) = self.rebuild_visible() {
            self.icon_theme = previous;
            if let Mode::ThemePicker(picker) = &mut self.mode {
                picker.message = Some(format!("Failed to refresh tree: {error}"));
            }
            return;
        }
        self.selected_theme = Some(id.clone());
        self.mode = Mode::Navigation;
        self.error = self
            .theme_store
            .save_selection(&id)
            .err()
            .map(|error| {
                format!(
                    "Theme applied for this session, but selection could not be saved: {error:#}"
                )
            })
            .or(warning);
    }

    pub(super) fn draw_theme_picker(&mut self, frame: &mut Frame, content: Rect) {
        let Mode::ThemePicker(picker) = &mut self.mode else {
            return;
        };
        let width = content.width.min(90);
        let height = content.height.min(
            u16::try_from(picker.entries.len().saturating_add(5))
                .unwrap_or(u16::MAX)
                .max(7),
        );
        let area = Rect::new(
            content.x + (content.width - width) / 2,
            content.y + (content.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, area);
        let block = Block::bordered().title(" Icon themes ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width < 12 || inner.height < 2 {
            frame.render_widget(Paragraph::new("Enlarge window. Esc cancels"), inner);
            return;
        }
        let [list_area, message_area] =
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
            list_area,
            &mut picker.state,
        );
        let message = picker.message.as_deref().unwrap_or(if picker.loading {
            "Loading VSIX packages… Esc cancels."
        } else if picker.entries.is_empty() {
            "Add a VSIX package and reopen this menu."
        } else {
            "Enter applies; Esc cancels. Reopen to refresh the list."
        });
        frame.render_widget(
            Paragraph::new(message).wrap(Wrap { trim: false }),
            message_area,
        );
    }
}
