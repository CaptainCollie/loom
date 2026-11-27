pub(crate) mod themes;
mod worker;

use ratatui::style::{Color, Modifier, Style};
use std::{ops::Range, sync::Arc};
use syntect::highlighting::{FontStyle, Theme};

pub(crate) use worker::Client;

#[derive(Debug, Clone)]
pub(crate) struct StyledRange {
    pub bytes: Range<usize>,
    pub style: Style,
}

#[derive(Debug)]
pub(crate) struct HighlightedPage {
    pub page: Arc<crate::viewer::Page>,
    pub rows: Vec<Vec<StyledRange>>,
    pub notice: Option<String>,
}

pub(crate) fn rgb(
    color: syntect::highlighting::Color,
    background: syntect::highlighting::Color,
) -> Color {
    let blend = |value: u8, base: u8| {
        ((u32::from(value) * u32::from(color.a) + u32::from(base) * (255 - u32::from(color.a)))
            / 255) as u8
    };
    Color::Rgb(
        blend(color.r, background.r),
        blend(color.g, background.g),
        blend(color.b, background.b),
    )
}

pub(crate) fn background(theme: &Theme) -> syntect::highlighting::Color {
    let base = syntect::highlighting::Color {
        r: 30,
        g: 30,
        b: 30,
        a: 255,
    };
    let color = theme.settings.background.unwrap_or(base);
    let Color::Rgb(r, g, b) = rgb(color, base) else {
        return base;
    };
    syntect::highlighting::Color { r, g, b, a: 255 }
}

pub(crate) fn style(style: syntect::highlighting::Style, theme: &Theme) -> Style {
    let bg = background(theme);
    let mut modifiers = Modifier::empty();
    if style.font_style.contains(FontStyle::BOLD) {
        modifiers |= Modifier::BOLD;
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        modifiers |= Modifier::ITALIC;
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        modifiers |= Modifier::UNDERLINED;
    }
    Style::default()
        .fg(rgb(style.foreground, bg))
        .bg(rgb(style.background, bg))
        .add_modifier(modifiers)
}
