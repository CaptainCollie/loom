use std::path::Path;

use devicons::icon_for_file;
use ratatui::style::Color;

use super::{folder_icons, theme::IconTheme};

pub(super) struct Icon {
    pub(super) glyph: char,
    pub(super) color: Color,
}

pub(super) fn icon_for(
    path: &Path,
    is_dir: bool,
    expanded: bool,
    root: bool,
    theme: &IconTheme,
) -> Icon {
    let glyph = if is_dir {
        path.file_name().map_or('\u{f115}', |name| {
            folder_icons::glyph_for(&name.to_string_lossy())
        })
    } else {
        icon_for_file(path, &Some(devicons::Theme::Dark)).icon
    };
    Icon {
        glyph,
        color: if expanded || root {
            theme.color_for_state(path, is_dir, expanded, root)
        } else {
            theme.color_for(path, is_dir)
        },
    }
}
