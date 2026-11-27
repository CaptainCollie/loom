use super::App;
use ratatui::{
    style::Stylize,
    text::{Line, Span},
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
#[derive(Debug)]
pub(super) struct TreeEntry {
    pub(super) path: PathBuf,
    pub(super) display_line: Line<'static>,
    pub(super) is_dir: bool,
}

impl App {
    pub(super) fn rebuild_visible(&mut self) -> io::Result<()> {
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

    pub(super) fn selected_entry(&self) -> Option<&TreeEntry> {
        self.list_state
            .selected()
            .and_then(|index| self.visible.get(index))
    }

    pub(super) fn refresh_tree(&mut self) {
        self.refresh_error = self.rebuild_visible().err().map(|error| error.to_string());
    }
}
