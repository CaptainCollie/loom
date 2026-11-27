use crate::theme_archive::{Archive, resolve};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use syntect::highlighting::{
    Color, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSet,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub(crate) enum Selection {
    Builtin,
    Disabled,
    Vsix { archive: PathBuf, id: String },
}

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub selection: Selection,
    pub label: String,
    pub theme: Result<Option<Arc<Theme>>, String>,
}

pub(crate) fn builtin() -> Arc<Theme> {
    static DEFAULT: OnceLock<Arc<Theme>> = OnceLock::new();
    DEFAULT
        .get_or_init(|| {
            let mut themes = ThemeSet::load_defaults();
            Arc::new(
                themes
                    .themes
                    .remove("base16-ocean.dark")
                    .unwrap_or_default(),
            )
        })
        .clone()
}

pub(crate) fn read_selection(directory: &Path) -> Result<Selection> {
    let path = directory.join(".selected-syntax-theme.json");
    if !path.exists() {
        return Ok(Selection::Builtin);
    }
    let data = fs::read(&path)?;
    ensure!(data.len() <= 16384, "Invalid syntax theme selection");
    Ok(serde_json::from_slice(&data)?)
}

pub(crate) fn save_selection(directory: &Path, selection: &Selection) -> Result<()> {
    use std::io::Write;
    static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = directory.join(format!(
        ".syntax-selection-{}-{}.tmp",
        std::process::id(),
        ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(selection)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, directory.join(".selected-syntax-theme.json"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn entries(directory: &Path, cancelled: &dyn Fn() -> bool) -> Result<Vec<Entry>> {
    let mut entries = vec![
        Entry {
            selection: Selection::Builtin,
            label: "base16-ocean.dark — built-in".into(),
            theme: Ok(Some(builtin())),
        },
        Entry {
            selection: Selection::Disabled,
            label: "No highlighting".into(),
            theme: Ok(None),
        },
    ];
    for file in fs::read_dir(directory)? {
        ensure!(!cancelled(), "Cancelled");
        let file = file?;
        if !file.file_type()?.is_file()
            || file
                .path()
                .extension()
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("vsix"))
        {
            continue;
        }
        let name = PathBuf::from(file.file_name());
        match package(&file.path(), cancelled) {
            Ok(imported) => entries.extend(imported),
            Err(error) => entries.push(Entry {
                selection: Selection::Vsix {
                    archive: name.clone(),
                    id: String::new(),
                },
                label: format!("{} — invalid package", name.display()),
                theme: Err(format!("{error:#}")),
            }),
        }
    }
    entries.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(entries)
}

fn package(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<Vec<Entry>> {
    let mut archive = Archive::open(path)?;
    let package: Value = archive.json("extension/package.json")?;
    let Some(contributions) = package
        .pointer("/contributes/themes")
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    let mut ids = HashSet::new();
    for contribution in contributions {
        ensure!(!cancelled(), "Cancelled");
        archive.remaining = 128 * 1024 * 1024;
        let theme_path = contribution
            .get("path")
            .and_then(Value::as_str)
            .context("Missing color theme path")?;
        let id = contribution
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(theme_path)
            .to_owned();
        let label = contribution
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        let theme = (|| -> Result<Arc<Theme>> {
            ensure!(ids.insert(id.clone()), "Duplicate color theme identifier");
            let path = resolve("extension", theme_path)?;
            let mut theme = (*builtin()).clone();
            // Only UI defaults are fallback values. Ocean's scope selectors must
            // not override the selected theme through greater specificity.
            theme.scopes.clear();
            apply(
                &mut archive,
                &path,
                &mut theme,
                &mut HashSet::new(),
                0,
                cancelled,
            )?;
            // For equally specific selectors syntect keeps the first value;
            // VS Code gives later rules (including child overrides) priority.
            theme.scopes.reverse();
            theme.name = Some(label.clone());
            Ok(Arc::new(theme))
        })()
        .map_err(|error| format!("{error:#}"));
        entries.push(Entry {
            selection: Selection::Vsix {
                archive: path.file_name().context("Invalid archive name")?.into(),
                id,
            },
            label: format!(
                "{label} — {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            theme: theme.map(Some),
        });
    }
    Ok(entries)
}

fn apply(
    archive: &mut Archive,
    path: &str,
    theme: &mut Theme,
    visited: &mut HashSet<String>,
    depth: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    ensure!(!cancelled(), "Cancelled");
    ensure!(
        depth < 16 && visited.insert(path.into()),
        "Cyclic or too deep theme include"
    );
    let base = path.rsplit_once('/').context("Invalid theme path")?.0;
    if path.to_ascii_lowercase().ends_with(".tmtheme") {
        let data = archive.read(path, 16 * 1024 * 1024)?;
        // Bound XML depth before handing a property list to syntect's loader.
        let xml = std::str::from_utf8(&data)?;
        let doc = roxmltree::Document::parse_with_options(
            xml,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )?;
        ensure!(
            doc.descendants()
                .all(|node| node.ancestors().take(130).count() <= 128),
            "Theme XML nesting exceeds 128 levels"
        );
        let loaded = ThemeSet::load_from_reader(&mut Cursor::new(data))?;
        theme.scopes.extend(loaded.scopes);
        if loaded.settings.foreground.is_some() {
            theme.settings.foreground = loaded.settings.foreground;
        }
        if loaded.settings.background.is_some() {
            theme.settings.background = loaded.settings.background;
        }
        if loaded.settings.gutter_foreground.is_some() {
            theme.settings.gutter_foreground = loaded.settings.gutter_foreground;
        }
        if loaded.settings.line_highlight.is_some() {
            theme.settings.line_highlight = loaded.settings.line_highlight;
        }
        if loaded.settings.find_highlight.is_some() {
            theme.settings.find_highlight = loaded.settings.find_highlight;
        }
        if loaded.settings.find_highlight_foreground.is_some() {
            theme.settings.find_highlight_foreground = loaded.settings.find_highlight_foreground;
        }
        visited.remove(path);
        return Ok(());
    }
    let json: Value = archive.json(path)?;
    if let Some(include) = json.get("include").and_then(Value::as_str) {
        apply(
            archive,
            &resolve(base, include)?,
            theme,
            visited,
            depth + 1,
            cancelled,
        )?;
    }
    if let Some(tokens) = json.get("tokenColors") {
        if let Some(path) = tokens.as_str() {
            apply(
                archive,
                &resolve(base, path)?,
                theme,
                visited,
                depth + 1,
                cancelled,
            )?;
        } else if let Some(rules) = tokens.as_array() {
            for rule in rules {
                let settings = rule.get("settings").context("Missing token settings")?;
                let modifier = modifier(settings)?;
                let scopes = match rule.get("scope") {
                    Some(Value::String(value)) => vec![value.as_str()],
                    Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
                    None => {
                        if modifier.foreground.is_some() {
                            theme.settings.foreground = modifier.foreground;
                        }
                        if modifier.background.is_some() {
                            theme.settings.background = modifier.background;
                        }
                        continue;
                    }
                    _ => anyhow::bail!("Invalid TextMate scope"),
                };
                for scope in scopes {
                    theme.scopes.push(ThemeItem {
                        scope: scope.parse::<ScopeSelectors>()?,
                        style: modifier,
                    });
                }
            }
        }
    }
    if let Some(colors) = json.get("colors").and_then(Value::as_object) {
        for (key, target) in [
            ("editor.foreground", &mut theme.settings.foreground),
            ("editor.background", &mut theme.settings.background),
            (
                "editorLineNumber.foreground",
                &mut theme.settings.gutter_foreground,
            ),
            (
                "editor.lineHighlightBackground",
                &mut theme.settings.line_highlight,
            ),
            (
                "editor.findMatchBackground",
                &mut theme.settings.find_highlight,
            ),
            (
                "editor.findMatchForeground",
                &mut theme.settings.find_highlight_foreground,
            ),
        ] {
            if let Some(value) = colors.get(key).and_then(Value::as_str) {
                *target = Some(color(value)?);
            }
        }
    }
    visited.remove(path);
    Ok(())
}

fn modifier(settings: &Value) -> Result<StyleModifier> {
    let mut result = StyleModifier::default();
    if let Some(value) = settings.get("foreground").and_then(Value::as_str) {
        result.foreground = Some(color(value)?);
    }
    if let Some(value) = settings.get("background").and_then(Value::as_str) {
        result.background = Some(color(value)?);
    }
    if let Some(value) = settings.get("fontStyle").and_then(Value::as_str) {
        let mut style = FontStyle::empty();
        for name in value.split_whitespace() {
            match name {
                "bold" => style |= FontStyle::BOLD,
                "italic" => style |= FontStyle::ITALIC,
                "underline" => style |= FontStyle::UNDERLINE,
                _ => {}
            }
        }
        result.font_style = Some(style);
    }
    Ok(result)
}

fn color(value: &str) -> Result<Color> {
    let hex = value
        .strip_prefix('#')
        .context("Expected hexadecimal color")?;
    ensure!(
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid theme color: {value}"
    );
    let hex = if hex.len() <= 4 {
        hex.chars().flat_map(|ch| [ch, ch]).collect::<String>()
    } else {
        hex.to_owned()
    };
    Ok(Color {
        r: u8::from_str_radix(&hex[..2], 16)?,
        g: u8::from_str_radix(&hex[2..4], 16)?,
        b: u8::from_str_radix(&hex[4..6], 16)?,
        a: if hex.len() == 8 {
            u8::from_str_radix(&hex[6..], 16)?
        } else {
            255
        },
    })
}
