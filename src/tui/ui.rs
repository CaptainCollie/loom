use super::{App, CreateKind, Mode};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const READ_HINTS: &[(&str, &str)] = &[
    ("Up/Down", "line"),
    ("Left/Right", "scroll"),
    ("PgUp/PgDn", "page"),
    ("Home/End", "first/last"),
    ("g", "go to line"),
    ("/", "search"),
    ("n/N", "next/previous"),
    ("Esc", "back"),
    ("Ctrl+C", "quit"),
];
const PROMPT_HINTS: &[(&str, &str)] = &[
    ("Enter", "submit"),
    ("Esc", "cancel"),
    ("Left/Right", "cursor"),
    ("Backspace", "erase"),
    ("Ctrl+C", "quit"),
];
const BUSY_HINTS: &[(&str, &str)] = &[("Esc", "cancel operation"), ("Ctrl+C", "quit")];

impl App {
    pub(super) fn screen_layout(&self, screen: Rect) -> [Rect; 3] {
        // Loading must not resize the viewport and trigger another page request.
        let hint_rows = if matches!(self.mode, Mode::Read(_)) {
            [READ_HINTS, PROMPT_HINTS, BUSY_HINTS]
                .into_iter()
                .map(|actions| hint_lines(actions, screen.width).len())
                .max()
                .unwrap_or(0)
        } else {
            self.key_hints(screen.width).len()
        };
        let footer_height = u16::try_from(hint_rows).unwrap_or(u16::MAX);
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .areas(screen)
    }

    pub(super) fn delete_dialog(&self, screen: Rect) -> Option<(Rect, Vec<Line<'static>>)> {
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

    pub(super) fn draw(&mut self, frame: &mut Frame) {
        let hints = self.key_hints(frame.area().width);
        let [header_area, content_area, footer_area] = self.screen_layout(frame.area());
        frame.render_widget(Line::from("loom".cyan().bold()).centered(), header_area);
        frame.render_widget(Paragraph::new(hints), footer_area);

        if matches!(&self.mode, Mode::Read(_)) {
            self.draw_reader(frame, content_area);
        } else {
            let input_height = match &self.mode {
                Mode::Create { .. } => 4,
                _ => 0,
            };

            let [tree_area, input_area] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(input_height)])
                    .areas(content_area);

            self.draw_tree(frame, tree_area);
            self.draw_create_input(frame, input_area);
            self.draw_delete_dialog(frame, content_area);
        }
    }

    fn draw_tree(&mut self, frame: &mut Frame, tree_area: Rect) {
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
    }

    fn draw_create_input(&self, frame: &mut Frame, input_area: Rect) {
        if let Mode::Create {
            kind,
            input,
            directory,
        } = &self.mode
        {
            let name = input.text();
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
                let cursor_byte = input.byte_index();
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

    fn draw_delete_dialog(&mut self, frame: &mut Frame, content_area: Rect) {
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
        let actions: &[(&str, &str)] = match &self.mode {
            Mode::Navigation => &[
                ("Up/Down", "move"),
                ("Left/Right", "collapse/expand"),
                ("f", "file"),
                ("d", "directory"),
                ("s", "structure"),
                ("Enter", "read"),
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
            Mode::Read(reader) if reader.prompt.is_some() => PROMPT_HINTS,
            Mode::Read(reader) if reader.viewer.busy => BUSY_HINTS,
            Mode::Read(_) => READ_HINTS,
        };
        hint_lines(actions, width)
    }
}

fn hint_lines(actions: &'static [(&str, &str)], width: u16) -> Vec<Line<'static>> {
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
