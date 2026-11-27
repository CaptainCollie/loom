use anyhow::{Context, Result, bail, ensure};
use std::{collections::HashMap, fs::File, io::Read, path::Path};
use zip::ZipArchive;
const MANIFEST_LIMIT: u64 = 16 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 128 * 1024 * 1024;
const JSON_MAX_DEPTH: usize = 128;
pub(crate) struct Archive {
    zip: ZipArchive<File>,
    names: HashMap<String, usize>,
    pub(crate) remaining: u64,
}
impl Archive {
    pub(crate) fn open(path: &Path) -> Result<Self> {
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
    pub(crate) fn read(&mut self, name: &str, limit: u64) -> Result<Vec<u8>> {
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
    pub(crate) fn json<T: serde::de::DeserializeOwned>(&mut self, name: &str) -> Result<T> {
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
pub(crate) fn resolve(base: &str, relative: &str) -> Result<String> {
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
