use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use super::{App, Mode, input::TextInput};
use crate::viewer::Viewer;

#[derive(Debug)]
pub(super) enum Prompt {
    Goto(TextInput),
    Search(TextInput),
}

#[derive(Debug)]
pub(super) struct Reader {
    pub(super) viewer: Viewer,
    pub(super) prompt: Option<Prompt>,
    desired: (usize, usize),
    pub(super) syntax_picker: Option<super::syntax_picker::Picker>,
}

impl App {
    pub(super) fn open_reader(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if entry.is_dir {
            let path = entry.path.clone();
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            self.refresh_tree();
            return;
        }
        let path = entry.path.clone();
        self.ensure_syntax_catalog();
        match Viewer::open(path) {
            Ok(mut viewer) => {
                if let Some(catalog) = &self.syntax_catalog {
                    viewer.set_syntax_theme(catalog.theme.clone());
                }
                self.mode = Mode::Read(Box::new(Reader {
                    viewer,
                    prompt: None,
                    desired: (0, 0),
                    syntax_picker: None,
                }))
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub(super) fn poll_reader(&mut self) {
        self.poll_syntax_catalog();
        let Mode::Read(reader) = &mut self.mode else {
            return;
        };
        if reader.viewer.poll() {
            self.error = reader.viewer.message.clone();
            self.mode = Mode::Navigation;
            self.refresh_tree();
            return;
        }
        // The first request waits until draw has measured the actual viewport.
        if !reader.viewer.busy
            && !reader.viewer.invalidated
            && reader.viewer.viewport != reader.desired
        {
            reader.viewer.viewport = reader.desired;
            reader.viewer.load(reader.viewer.view());
        }
    }

    pub(super) fn handle_read_key(&mut self, key: KeyEvent) {
        if matches!(&self.mode,Mode::Read(reader) if reader.syntax_picker.is_some()) {
            self.handle_syntax_key(key);
            return;
        }
        let Mode::Read(reader) = &mut self.mode else {
            return;
        };
        if let Some(prompt) = &mut reader.prompt {
            if key.code == KeyCode::Esc {
                reader.prompt = None;
                return;
            }
            let input = match prompt {
                Prompt::Goto(input) | Prompt::Search(input) => input,
            };
            match key.code {
                KeyCode::Enter => {
                    let text = input.text().to_owned();
                    match prompt {
                        Prompt::Goto(_) => match text.parse::<u64>() {
                            Ok(number) if number > 0 => {
                                let mut view = reader.viewer.view();
                                view.selected = number;
                                view.top = number.saturating_sub(view.height as u64 / 2).max(1);
                                reader.viewer.load(view);
                                reader.prompt = None;
                            }
                            _ => {
                                reader.viewer.message = Some("Enter a positive line number".into())
                            }
                        },
                        Prompt::Search(_) => {
                            if text.is_empty() || text.contains(['\r', '\n']) {
                                reader.viewer.message =
                                    Some("Enter a nonempty single-line query".into());
                            } else {
                                reader.viewer.query = text;
                                reader.viewer.search(false, true);
                                reader.prompt = None;
                            }
                        }
                    }
                }
                KeyCode::Char(ch) if !ch.is_control() => input.insert(ch),
                KeyCode::Backspace => input.backspace(),
                KeyCode::Left => input.move_left(),
                KeyCode::Right => input.move_right(),
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Esc {
            if reader.viewer.busy {
                reader.viewer.cancel();
            } else {
                if let Some(catalog) = &self.syntax_catalog {
                    catalog.cancel();
                }
                self.mode = Mode::Navigation;
                self.refresh_tree();
            }
            return;
        }
        if key.code == KeyCode::Char('t') {
            self.begin_syntax_selection();
            return;
        }
        if reader.viewer.busy || reader.viewer.invalidated {
            return;
        }
        let mut view = reader.viewer.view();
        match key.code {
            KeyCode::Char('g') => {
                reader.prompt = Some(Prompt::Goto(TextInput::default()));
                return;
            }
            KeyCode::Char('/') => {
                let mut input = TextInput::default();
                for ch in reader.viewer.query.chars() {
                    input.insert(ch);
                }
                reader.prompt = Some(Prompt::Search(input));
                return;
            }
            KeyCode::Char('n') => {
                reader.viewer.search(false, false);
                return;
            }
            KeyCode::Char('N') => {
                reader.viewer.search(true, false);
                return;
            }
            KeyCode::Home => {
                view.selected = 1;
                view.top = 1;
            }
            KeyCode::End => {
                reader.viewer.last();
                return;
            }
            KeyCode::Up => view.selected = view.selected.saturating_sub(1).max(1),
            KeyCode::Down => view.selected = view.selected.saturating_add(1),
            KeyCode::PageUp => {
                view.selected = view.selected.saturating_sub(view.height as u64).max(1)
            }
            KeyCode::PageDown => view.selected = view.selected.saturating_add(view.height as u64),
            KeyCode::Left => view.left = view.left.saturating_sub(1),
            KeyCode::Right => view.left = view.left.saturating_add(1),
            _ => return,
        }
        reader.viewer.load(view);
    }

    pub(super) fn draw_reader(&mut self, frame: &mut Frame, area: Rect) {
        let Mode::Read(reader) = &mut self.mode else {
            return;
        };
        let view = reader.viewer.view();
        let total = reader
            .viewer
            .page
            .as_ref()
            .and_then(|page| page.total)
            .map_or("?".to_owned(), |total| total.to_string());
        let status = if reader.viewer.busy {
            reader.viewer.progress()
        } else {
            reader
                .viewer
                .message
                .clone()
                .or_else(|| reader.viewer.syntax_message.clone())
                .unwrap_or_else(|| format!("Line {} / {total}", view.selected))
        };
        let block = Block::bordered()
            .title(reader.viewer.path.display().to_string())
            .title_bottom(status);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let palette = reader.viewer.syntax_theme.as_deref();
        let background = palette.map(crate::syntax::background);
        let base_style = palette.map_or(Style::default(), |theme| {
            let bg = crate::syntax::background(theme);
            let mut style = Style::default().bg(crate::syntax::rgb(bg, bg));
            if let Some(fg) = theme.settings.foreground {
                style = style.fg(crate::syntax::rgb(fg, bg));
            }
            style
        });
        frame.render_widget(Block::default().style(base_style), inner);
        // Keep the page stable when opening or closing a command prompt.
        let [text_area, prompt_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        let last_number = view
            .top
            .saturating_add(u64::from(text_area.height))
            .saturating_sub(1)
            .max(1);
        let number_width = u16::try_from(last_number.to_string().len() + 1).unwrap_or(u16::MAX);
        let [numbers_area, body_area] =
            Layout::horizontal([Constraint::Length(number_width), Constraint::Min(0)])
                .areas(text_area);
        reader.desired = (
            usize::from(text_area.height).max(1),
            usize::from(body_area.width).max(1),
        );
        if let Some(page) = &reader.viewer.page {
            for (index, row) in page
                .rows
                .iter()
                .take(usize::from(text_area.height))
                .enumerate()
            {
                let y = text_area.y + index as u16;
                let current = row.number == page.view.selected;
                let style = if current {
                    palette
                        .and_then(|theme| theme.settings.line_highlight)
                        .zip(background)
                        .map_or(base_style.bg(Color::DarkGray), |(color, bg)| {
                            base_style.bg(crate::syntax::rgb(color, bg))
                        })
                } else {
                    base_style
                };
                frame.render_widget(
                    Block::default().style(style),
                    Rect::new(text_area.x, y, text_area.width, 1),
                );
                frame.render_widget(
                    Paragraph::new(row.number.to_string())
                        .right_aligned()
                        .style(if current {
                            style.fg(Color::Yellow).bold()
                        } else {
                            palette
                                .and_then(|theme| theme.settings.gutter_foreground)
                                .zip(background)
                                .map_or(style.fg(Color::Gray), |(color, bg)| {
                                    style.fg(crate::syntax::rgb(color, bg))
                                })
                        }),
                    Rect::new(numbers_area.x, y, numbers_area.width.saturating_sub(1), 1),
                );
                let ranges = reader
                    .viewer
                    .syntax_page
                    .as_ref()
                    .filter(|syntax| std::sync::Arc::ptr_eq(&syntax.page, page))
                    .and_then(|syntax| syntax.rows.get(index))
                    .map_or(&[][..], Vec::as_slice);
                let mut search = Style::default().fg(Color::Black).bg(Color::Yellow);
                if let (Some(theme), Some(bg)) = (palette, background) {
                    if let Some(color) = theme.settings.find_highlight {
                        search = search.bg(crate::syntax::rgb(color, bg));
                    }
                    if let Some(color) = theme.settings.find_highlight_foreground {
                        search = search.fg(crate::syntax::rgb(color, bg));
                    }
                }
                let line = styled_line(
                    &row.text,
                    ranges,
                    row.highlight.as_ref(),
                    style,
                    search,
                    current,
                );
                frame.render_widget(
                    Paragraph::new(line).style(style),
                    Rect::new(body_area.x, y, body_area.width, 1),
                );
            }
        } else {
            frame.render_widget(Paragraph::new("Loading file..."), body_area);
        }
        if let Some(prompt) = &reader.prompt {
            let (label, input) = match prompt {
                Prompt::Goto(input) => ("Line: ", input),
                Prompt::Search(input) => ("Search: ", input),
            };
            let available = usize::from(prompt_area.width).saturating_sub(label.len());
            let byte = input.byte_index();
            let before = &input.text()[..byte];
            let start = before
                .char_indices()
                .map(|(at, _)| at)
                .chain(std::iter::once(byte))
                .find(|at| before[*at..].width() < available)
                .unwrap_or(byte);
            frame.render_widget(
                Paragraph::new(format!("{label}{}", &input.text()[start..])),
                prompt_area,
            );
            let x = label.len() + before[start..].width();
            if prompt_area.height > 0 && x < usize::from(prompt_area.width) {
                frame.set_cursor_position((prompt_area.x + x as u16, prompt_area.y));
            }
        }
    }
}

fn styled_line<'a>(
    text: &'a str,
    ranges: &[crate::syntax::StyledRange],
    hit: Option<&std::ops::Range<usize>>,
    base: Style,
    search: Style,
    current: bool,
) -> Line<'a> {
    let mut boundaries = vec![0, text.len()];
    for range in ranges {
        boundaries.extend([range.bytes.start, range.bytes.end]);
    }
    if let Some(hit) = hit {
        boundaries.extend([hit.start, hit.end]);
    }
    boundaries.retain(|&at| at <= text.len() && text.is_char_boundary(at));
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut spans = Vec::new();
    let mut index = 0;
    for pair in boundaries.windows(2) {
        while index < ranges.len() && ranges[index].bytes.end <= pair[0] {
            index += 1;
        }
        let mut style = base;
        if let Some(range) = ranges
            .get(index)
            .filter(|range| range.bytes.start <= pair[0])
        {
            style = style.patch(range.style);
        }
        if current {
            style.bg = base.bg;
        }
        if hit.is_some_and(|hit| hit.start < pair[1] && hit.end > pair[0]) {
            style = style.patch(search);
        }
        spans.push(Span::styled(&text[pair[0]..pair[1]], style));
    }
    Line::from(spans)
}
