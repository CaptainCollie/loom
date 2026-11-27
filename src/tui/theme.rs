use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result, ensure};
use ratatui::style::Color;
use serde::Deserialize;

pub(super) const MATERIAL_JSON: &str = include_str!("../../assets/icon-themes/material.json");

#[derive(Debug, Clone)]
pub(super) struct IconTheme {
    pub name: String,
    pub warning: Option<String>,
    imported: Option<super::vsix::IconRules>,
    file_stems: ColorMap,
    file_suffixes: ColorMap,
    directory_colors: ColorMap,
    default_file_color: Color,
    default_directory_color: Color,
}

#[derive(Debug, Clone)]
struct ColorMap {
    exact: HashMap<String, Color>,
    folded: HashMap<String, Color>,
}

impl ColorMap {
    fn new(exact: HashMap<String, Color>) -> Self {
        let mut entries: Vec<_> = exact.iter().collect();
        entries.sort_by_key(|(name, _)| *name);
        let mut folded = HashMap::new();
        for (name, color) in entries {
            let lower = name.to_ascii_lowercase();
            if *name == lower {
                folded.insert(lower, *color);
            } else {
                folded.entry(lower).or_insert(*color);
            }
        }
        Self { exact, folded }
    }

    fn lookup(&self, name: &str) -> Option<Color> {
        self.exact
            .get(name)
            .or_else(|| self.folded.get(&name.to_ascii_lowercase()))
            .copied()
    }
}

#[derive(Deserialize)]
struct ThemeJson {
    version: u32,
    name: String,
    #[serde(default)]
    file_stems: HashMap<String, String>,
    #[serde(default)]
    file_suffixes: HashMap<String, String>,
    #[serde(default)]
    colors: HashMap<String, String>,
    #[serde(default)]
    directory_colors: HashMap<String, String>,
    default_file_color: String,
    default_directory_color: String,
}

impl IconTheme {
    pub(super) fn imported(
        name: String,
        rules: super::vsix::IconRules,
        warning: Option<String>,
    ) -> Self {
        Self {
            name,
            warning,
            imported: Some(rules),
            file_stems: ColorMap::new(HashMap::new()),
            file_suffixes: ColorMap::new(HashMap::new()),
            directory_colors: ColorMap::new(HashMap::new()),
            default_file_color: Color::Rgb(126, 142, 168),
            default_directory_color: Color::Rgb(126, 142, 168),
        }
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let source: ThemeJson = serde_json::from_str(json).context("Invalid theme JSON")?;
        ensure!(
            source.version == 1,
            "Unsupported theme version: {}",
            source.version
        );
        ensure!(
            !source.name.trim().is_empty(),
            "Theme name must not be empty"
        );
        let colors = parse_colors(source.colors)?;
        let resolve = |entries: HashMap<String, String>| -> Result<HashMap<String, Color>> {
            entries
                .into_iter()
                .map(|(key, kind)| {
                    let color = colors
                        .get(&kind)
                        .copied()
                        .with_context(|| format!("Unknown color type {kind:?} for {key:?}"))?;
                    Ok((key, color))
                })
                .collect()
        };
        Ok(Self {
            name: source.name,
            warning: None,
            imported: None,
            file_stems: ColorMap::new(resolve(source.file_stems)?),
            file_suffixes: ColorMap::new(resolve(source.file_suffixes)?),
            directory_colors: ColorMap::new(parse_colors(source.directory_colors)?),
            default_file_color: parse_color(&source.default_file_color)?,
            default_directory_color: parse_color(&source.default_directory_color)?,
        })
    }

    pub fn builtin() -> Result<Self> {
        Self::from_json(MATERIAL_JSON).context("Invalid embedded Material theme")
    }

    pub fn color_for(&self, path: &Path, is_dir: bool) -> Color {
        self.color_for_state(path, is_dir, false, false)
    }

    pub fn color_for_state(&self, path: &Path, is_dir: bool, expanded: bool, root: bool) -> Color {
        if let Some(rules) = &self.imported {
            return rules.color_for(path, is_dir, expanded, root);
        }
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        if is_dir {
            return self
                .directory_colors
                .lookup(&name)
                .unwrap_or(self.default_directory_color);
        }
        if let Some(color) = self.lookup_file(&name) {
            return color;
        }
        for (index, _) in name.match_indices('.') {
            if let Some(color) = self.lookup_file(&name[index + 1..]) {
                return color;
            }
        }
        if let Some((stem, _)) = name.split_once('.')
            && let Some(color) = self.file_stems.lookup(stem)
        {
            return color;
        }
        self.default_file_color
    }

    fn lookup_file(&self, name: &str) -> Option<Color> {
        let lower = name.to_ascii_lowercase();
        self.file_stems
            .exact
            .get(name)
            .or_else(|| self.file_suffixes.exact.get(name))
            .or_else(|| self.file_stems.folded.get(&lower))
            .or_else(|| self.file_suffixes.folded.get(&lower))
            .copied()
    }
}

fn parse_colors(entries: HashMap<String, String>) -> Result<HashMap<String, Color>> {
    entries
        .into_iter()
        .map(|(name, value)| {
            let color =
                parse_color(&value).with_context(|| format!("Invalid color for {name:?}"))?;
            Ok((name, color))
        })
        .collect()
}

fn parse_color(value: &str) -> Result<Color> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit),
        "Expected #RRGGBB color, got {value:?}"
    );
    let rgb = u32::from_str_radix(&value[1..], 16)?;
    Ok(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}
