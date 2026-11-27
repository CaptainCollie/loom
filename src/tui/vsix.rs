//! VS Code icon themes are data: extension code is never executed.
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use ratatui::style::Color;
use serde::Deserialize;
use zip::ZipArchive;

use super::{svg_color, theme::IconTheme};

const MANIFEST_LIMIT: u64 = 16 * 1024 * 1024;
const SVG_LIMIT: u64 = 2 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 128 * 1024 * 1024;
const JSON_MAX_DEPTH: usize = 128;
const NEUTRAL: Color = Color::Rgb(126, 142, 168);

#[derive(Debug, Clone, Default)]
pub(super) struct IconRules {
    file: Option<Color>,
    folder: Option<Color>,
    folder_expanded: Option<Color>,
    root_folder: Option<Color>,
    root_folder_expanded: Option<Color>,
    file_names: HashMap<String, Color>,
    extensions: HashMap<String, Color>,
    folder_names: HashMap<String, Color>,
    folder_names_expanded: HashMap<String, Color>,
    root_names: HashMap<String, Color>,
    root_names_expanded: HashMap<String, Color>,
    language_names: HashMap<String, Color>,
    language_extensions: HashMap<String, Color>,
}

impl IconRules {
    pub fn color_for(&self, path: &Path, directory: bool, expanded: bool, root: bool) -> Color {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .to_lowercase();
        let parent = path
            .parent()
            .and_then(Path::file_name)
            .map(|p| p.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if directory {
            if root {
                return expanded
                    .then(|| self.root_names_expanded.get(&name).copied())
                    .flatten()
                    .or_else(|| self.root_names.get(&name).copied())
                    .or(if expanded {
                        self.root_folder_expanded
                    } else {
                        None
                    })
                    .or(self.root_folder)
                    .or(if expanded { self.folder_expanded } else { None })
                    .or(self.folder)
                    .unwrap_or(NEUTRAL);
            }
            return expanded
                .then(|| lookup(&self.folder_names_expanded, &parent, &name))
                .flatten()
                .or_else(|| lookup(&self.folder_names, &parent, &name))
                .or(if expanded { self.folder_expanded } else { None })
                .or(self.folder)
                .unwrap_or(NEUTRAL);
        }
        if let Some(color) = lookup(&self.file_names, &parent, &name) {
            return color;
        }
        // Parent-qualified extensions outrank unqualified ones, then longest first.
        for qualified in [true, false] {
            for (offset, _) in name.match_indices('.') {
                let suffix = &name[offset + 1..];
                let key = if qualified {
                    format!("{parent}/{suffix}")
                } else {
                    suffix.to_owned()
                };
                if let Some(color) = self.extensions.get(&key) {
                    return *color;
                }
            }
        }
        if let Some(color) = self.language_names.get(&name) {
            return *color;
        }
        for (offset, _) in name.match_indices('.') {
            if let Some(color) = self.language_extensions.get(&name[offset + 1..]) {
                return *color;
            }
        }
        self.file.unwrap_or(NEUTRAL)
    }
}

fn lookup(map: &HashMap<String, Color>, parent: &str, name: &str) -> Option<Color> {
    map.get(&format!("{parent}/{name}"))
        .or_else(|| map.get(name))
        .copied()
}

#[derive(Deserialize)]
struct Package {
    #[serde(default)]
    contributes: Contributions,
}
#[derive(Default, Deserialize)]
struct Contributions {
    #[serde(default, rename = "iconThemes")]
    themes: Vec<Contribution>,
    #[serde(default)]
    languages: Vec<Language>,
}
#[derive(Deserialize)]
struct Contribution {
    id: String,
    label: String,
    path: String,
}
#[derive(Deserialize)]
struct Language {
    id: String,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    filenames: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    icon_definitions: HashMap<String, Definition>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    folder_expanded: Option<String>,
    #[serde(default)]
    root_folder: Option<String>,
    #[serde(default)]
    root_folder_expanded: Option<String>,
    #[serde(default)]
    file_names: HashMap<String, String>,
    #[serde(default)]
    file_extensions: HashMap<String, String>,
    #[serde(default)]
    folder_names: HashMap<String, String>,
    #[serde(default)]
    folder_names_expanded: HashMap<String, String>,
    #[serde(default)]
    root_folder_names: HashMap<String, String>,
    #[serde(default)]
    root_folder_names_expanded: HashMap<String, String>,
    #[serde(default)]
    language_ids: HashMap<String, String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Definition {
    icon_path: Option<String>,
    font_color: Option<String>,
}

pub(super) struct ImportedTheme {
    pub id: String,
    pub label: String,
    pub theme: Result<IconTheme, String>,
}

pub(super) fn read_package(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<ImportedTheme>> {
    let mut archive = Archive::open(path)?;
    let package: Package = archive.json("extension/package.json")?;
    ensure!(
        !package.contributes.themes.is_empty(),
        "Package does not contribute any file icon themes"
    );
    let mut ids = HashSet::new();
    let mut themes = Vec::new();
    for contribution in package.contributes.themes {
        ensure!(!cancelled(), "Theme import cancelled");
        archive.remaining = TOTAL_LIMIT;
        let theme = if contribution.id.trim().is_empty() || !ids.insert(contribution.id.clone()) {
            Err("Empty or duplicate icon theme identifier".to_owned())
        } else {
            import(
                &mut archive,
                &contribution,
                &package.contributes.languages,
                cancelled,
            )
            .map_err(|error| format!("{error:#}"))
        };
        themes.push(ImportedTheme {
            id: contribution.id,
            label: contribution.label,
            theme,
        });
    }
    Ok(themes)
}

fn import(
    archive: &mut Archive,
    contribution: &Contribution,
    languages: &[Language],
    cancelled: &dyn Fn() -> bool,
) -> Result<IconTheme> {
    let path = resolve("extension", &contribution.path)?;
    let manifest: Manifest = archive.json(&path)?;
    let base = path.rsplit_once('/').context("Invalid theme path")?.0;
    let mut colors = HashMap::new();
    let mut assets = HashMap::new();
    let mut unsupported = 0;
    // Stable order also makes case-insensitive collisions deterministic.
    let mut definitions: Vec<_> = manifest.icon_definitions.iter().collect();
    definitions.sort_by_key(|(id, _)| *id);
    for (id, definition) in definitions {
        ensure!(!cancelled(), "Theme import cancelled");
        let color = if let Some(relative) = &definition.icon_path {
            let asset = resolve(base, relative)?;
            if let Some(color) = assets.get(&asset) {
                *color
            } else {
                let color = if asset.to_ascii_lowercase().ends_with(".svg") {
                    let data = archive.read(&asset, SVG_LIMIT)?;
                    svg_color::primary_color(&data)
                } else {
                    None
                };
                assets.insert(asset, color);
                color
            }
        } else {
            definition.font_color.as_deref().and_then(font_color)
        };
        if color.is_none() {
            unsupported += 1;
        }
        colors.insert(id.clone(), color);
    }
    let color = |id: &Option<String>| id.as_ref().and_then(|id| colors.get(id).copied().flatten());
    let file = color(&manifest.file);
    let folder = color(&manifest.folder);
    let resolve_map = |map: &HashMap<String, String>, fallback: Option<Color>| {
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_by_key(|(key, _)| *key);
        entries
            .into_iter()
            .map(|(key, id)| {
                (
                    key.to_lowercase(),
                    colors
                        .get(id)
                        .copied()
                        .flatten()
                        .or(fallback)
                        .unwrap_or(NEUTRAL),
                )
            })
            .collect()
    };
    let mut rules = IconRules {
        file,
        folder,
        folder_expanded: color(&manifest.folder_expanded),
        root_folder: color(&manifest.root_folder),
        root_folder_expanded: color(&manifest.root_folder_expanded),
        file_names: resolve_map(&manifest.file_names, file),
        extensions: resolve_map(&manifest.file_extensions, file),
        folder_names: resolve_map(&manifest.folder_names, folder),
        folder_names_expanded: resolve_map(
            &manifest.folder_names_expanded,
            color(&manifest.folder_expanded).or(folder),
        ),
        root_names: resolve_map(
            &manifest.root_folder_names,
            color(&manifest.root_folder).or(folder),
        ),
        root_names_expanded: resolve_map(
            &manifest.root_folder_names_expanded,
            color(&manifest.root_folder_expanded)
                .or(color(&manifest.root_folder))
                .or(folder),
        ),
        ..Default::default()
    };
    for language in languages {
        if let Some(id) = manifest.language_ids.get(&language.id) {
            let color = colors
                .get(id)
                .copied()
                .flatten()
                .or(file)
                .unwrap_or(NEUTRAL);
            for name in &language.filenames {
                rules.language_names.insert(name.to_lowercase(), color);
            }
            for suffix in &language.extensions {
                rules
                    .language_extensions
                    .insert(suffix.trim_start_matches('.').to_lowercase(), color);
            }
        }
    }
    let unresolved = manifest
        .language_ids
        .keys()
        .filter(|id| !languages.iter().any(|language| &language.id == *id))
        .count();
    let missing = [
        &manifest.file_names,
        &manifest.file_extensions,
        &manifest.folder_names,
        &manifest.folder_names_expanded,
        &manifest.root_folder_names,
        &manifest.root_folder_names_expanded,
        &manifest.language_ids,
    ]
    .into_iter()
    .flat_map(|map| map.values())
    .chain(
        [
            &manifest.file,
            &manifest.folder,
            &manifest.folder_expanded,
            &manifest.root_folder,
            &manifest.root_folder_expanded,
        ]
        .into_iter()
        .flatten(),
    )
    .filter(|id| !colors.contains_key(*id))
    .count();
    let warning = (unsupported + unresolved + missing > 0).then(|| format!(
            "Theme uses fallback colors: {unsupported} unsupported icon definitions, {missing} missing references; {unresolved} language associations have no filename mapping in this package."
        ));
    Ok(IconTheme::imported(
        contribution.label.clone(),
        rules,
        warning,
    ))
}

fn font_color(value: &str) -> Option<Color> {
    let value = value.strip_prefix('#')?;
    if !matches!(value.len(), 3 | 4 | 6 | 8) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let expanded;
    let value = if value.len() <= 4 {
        expanded = value.chars().flat_map(|c| [c, c]).collect::<String>();
        expanded.as_str()
    } else {
        value
    };
    if value.len() == 8 && u8::from_str_radix(&value[6..], 16).ok()? == 0 {
        return None;
    }
    let rgb = u32::from_str_radix(&value[..6], 16).ok()?;
    Some(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}

struct Archive {
    zip: ZipArchive<File>,
    names: HashMap<String, usize>,
    remaining: u64,
}
impl Archive {
    fn open(path: &Path) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        ensure!(
            metadata.file_type().is_file(),
            "VSIX must be a regular file"
        );
        let mut zip = ZipArchive::new(File::open(path)?).context("Invalid VSIX/ZIP archive")?;
        ensure!(
            !zip.has_duplicate_names(),
            "Duplicate names in VSIX archive"
        );
        ensure!(zip.len() <= 100_000, "VSIX has too many entries");
        let mut names = HashMap::new();
        for index in 0..zip.len() {
            let entry = zip.by_index(index)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name()?;
            ensure!(
                !name.contains(['\\', ':']) && !name.starts_with('/'),
                "Invalid archive path: {name}"
            );
            let normalized = normalize(&name)?;
            ensure!(
                names.insert(normalized.clone(), index).is_none(),
                "Duplicate archive entry: {normalized}"
            );
        }
        Ok(Self {
            zip,
            names,
            remaining: TOTAL_LIMIT,
        })
    }
    fn read(&mut self, name: &str, limit: u64) -> Result<Vec<u8>> {
        let index = *self
            .names
            .get(name)
            .with_context(|| format!("Missing archive entry: {name}"))?;
        let mut entry = self.zip.by_index(index)?;
        ensure!(
            !entry.is_symlink(),
            "Archive links are not supported: {name}"
        );
        ensure!(
            entry.size() <= limit && entry.size() <= self.remaining,
            "Theme data exceeds import size limit: {name}"
        );
        let mut data = Vec::new();
        (&mut entry)
            .take(limit.min(self.remaining) + 1)
            .read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 <= limit.min(self.remaining),
            "Theme data exceeds import size limit: {name}"
        );
        self.remaining -= data.len() as u64;
        Ok(data)
    }
    fn json<T: serde::de::DeserializeOwned>(&mut self, name: &str) -> Result<T> {
        let data = self.read(name, MANIFEST_LIMIT)?;
        let json = std::str::from_utf8(&data)
            .context("Manifest is not UTF-8")?
            .trim_start_matches('\u{feff}');
        validate_json_depth(json)?;
        json5::from_str(json).with_context(|| format!("Invalid JSON manifest: {name}"))
    }
}

// JSON5's recursive deserializer has no depth limit, even for ignored fields.
// This pass only bounds nesting; the deserializer still validates the syntax.
fn validate_json_depth(json: &str) -> Result<()> {
    let mut chars = json.chars().peekable();
    let mut depth = 0usize;
    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' => {
                while let Some(next) = chars.next() {
                    if next == '\\' {
                        chars.next();
                    } else if next == ch {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                for next in chars.by_ref() {
                    if matches!(next, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
            }
            '{' | '[' => {
                depth += 1;
                ensure!(
                    depth <= JSON_MAX_DEPTH,
                    "JSON nesting exceeds {JSON_MAX_DEPTH} levels"
                );
            }
            '}' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

fn normalize(path: &str) -> Result<String> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                ensure!(parts.pop().is_some(), "Path escapes archive");
            }
            part => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}
fn resolve(base: &str, relative: &str) -> Result<String> {
    ensure!(
        !relative.starts_with('/') && !relative.contains(['\\', ':']),
        "Invalid resource path: {relative}"
    );
    let name = normalize(&format!("{base}/{relative}"))?;
    if !name.starts_with("extension/") {
        bail!("Resource path escapes extension: {relative}");
    }
    Ok(name)
}
