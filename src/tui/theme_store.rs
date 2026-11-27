use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{theme::IconTheme, vsix};

const SELECTION: &str = ".selected-theme.json";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub(super) enum ThemeId {
    Builtin,
    Vsix { archive: PathBuf, theme_id: String },
}

#[derive(Debug)]
pub(super) struct ThemeStore {
    directory: PathBuf,
    requests: Sender<u64>,
    results: Receiver<ScanResult>,
    generation: Arc<AtomicU64>,
}

#[derive(Debug)]
pub(super) struct ThemeStartup {
    pub store: ThemeStore,
    pub theme: IconTheme,
    pub selected: Option<ThemeId>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ThemeEntry {
    pub id: ThemeId,
    pub label: String,
    pub theme: Result<Arc<IconTheme>, String>,
}

#[derive(Debug)]
pub(super) struct ScanResult {
    pub generation: u64,
    pub entries: Result<Vec<ThemeEntry>, String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SavedSelection {
    Current(ThemeId),
    Legacy { file: PathBuf },
}

impl ThemeStore {
    pub fn open() -> Result<ThemeStartup> {
        let directory = dirs::config_dir()
            .context("Cannot locate the configuration directory")?
            .join("loom")
            .join("themes");
        Self::open_in(directory)
    }

    pub fn open_in(directory: PathBuf) -> Result<ThemeStartup> {
        fs::create_dir_all(&directory)
            .with_context(|| format!("Cannot create {}", directory.display()))?;
        fs::read_dir(&directory).context("Cannot read theme directory")?;
        let load = || -> Result<(IconTheme, ThemeId)> {
            let id = match fs::read_to_string(directory.join(SELECTION)) {
                Ok(json) => match serde_json::from_str::<SavedSelection>(&json)
                    .context("Invalid theme selection")?
                {
                    SavedSelection::Current(id) => id,
                    SavedSelection::Legacy { file } if file == Path::new("material.json") => {
                        ThemeId::Builtin
                    }
                    SavedSelection::Legacy { .. } => anyhow::bail!(
                        "Legacy JSON themes are no longer supported; add a VSIX package"
                    ),
                },
                Err(error) if error.kind() == ErrorKind::NotFound => ThemeId::Builtin,
                Err(error) => return Err(error).context("Cannot read theme selection"),
            };
            let theme = match &id {
                ThemeId::Builtin => IconTheme::builtin()?,
                ThemeId::Vsix { archive, theme_id } => {
                    validate_name(archive)?;
                    let theme = vsix::read_package(&directory.join(archive), &|| false)?
                        .into_iter()
                        .find(|entry| &entry.id == theme_id)
                        .context("Saved icon theme no longer exists")?;
                    theme.theme.map_err(anyhow::Error::msg)?
                }
            };
            Ok((theme, id))
        };
        let (theme, selected, warning) = match load() {
            Ok((theme, id)) => {
                let warning = theme.warning.clone();
                (theme, Some(id), warning)
            }
            Err(error) => (
                IconTheme::builtin()?,
                Some(ThemeId::Builtin),
                Some(format!("Using built-in Material theme: {error:#}")),
            ),
        };
        let (requests, receiver) = mpsc::channel();
        let (sender, results) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = generation.clone();
        let worker_directory = directory.clone();
        std::thread::Builder::new()
            .name("loom-icon-themes".into())
            .spawn(move || {
                let mut cache = HashMap::new();
                while let Ok(request) = receiver.recv() {
                    let cancelled = || worker_generation.load(Ordering::Relaxed) != request;
                    if cancelled() {
                        continue;
                    }
                    let entries = scan(&worker_directory, &mut cache, &cancelled)
                        .map_err(|error| format!("{error:#}"));
                    if !cancelled()
                        && sender
                            .send(ScanResult {
                                generation: request,
                                entries,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            })
            .context("Cannot start icon theme worker")?;
        Ok(ThemeStartup {
            store: Self {
                directory,
                requests,
                results,
                generation,
            },
            theme,
            selected,
            warning,
        })
    }

    pub fn request_scan(&self) -> Result<u64> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.requests
            .send(generation)
            .context("Icon theme worker stopped")?;
        Ok(generation)
    }

    pub fn poll(&self) -> Option<ScanResult> {
        self.results.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn save_selection(&self, id: &ThemeId) -> Result<()> {
        if let ThemeId::Vsix { archive, theme_id } = id {
            validate_name(archive)?;
            ensure!(!theme_id.is_empty(), "Empty theme identifier");
        }
        let json = serde_json::to_vec(id)?;
        let (temporary, mut file) = loop {
            let path = self.directory.join(format!(
                ".selection-{}-{}.tmp",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => break (path, file),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error).context("Cannot create temporary theme selection"),
            }
        };
        let result = (|| -> Result<()> {
            file.write_all(&json)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, self.directory.join(SELECTION))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.context("Cannot save theme selection")
    }
}

impl Drop for ThemeStore {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Cached {
    length: u64,
    modified: SystemTime,
    entries: Vec<ThemeEntry>,
}

fn scan(
    directory: &Path,
    cache: &mut HashMap<PathBuf, Cached>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<ThemeEntry>> {
    let builtin = Arc::new(IconTheme::builtin()?);
    let mut entries = vec![ThemeEntry {
        id: ThemeId::Builtin,
        label: format!("{} — built-in", builtin.name),
        theme: Ok(builtin),
    }];
    let mut present = std::collections::HashSet::new();
    for entry in fs::read_dir(directory).context("Cannot list themes")? {
        ensure!(!cancelled(), "Theme import cancelled");
        let entry = entry?;
        let name = PathBuf::from(entry.file_name());
        if name
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("vsix"))
            || !entry.file_type()?.is_file()
        {
            continue;
        }
        present.insert(name.clone());
        let metadata = entry.metadata()?;
        let modified = metadata.modified()?;
        if let Some(cached) = cache
            .get(&name)
            .filter(|cached| cached.length == metadata.len() && cached.modified == modified)
        {
            entries.extend(cached.entries.clone());
            continue;
        }
        let imported = match vsix::read_package(&entry.path(), cancelled) {
            Ok(themes) => themes
                .into_iter()
                .map(|theme| ThemeEntry {
                    id: ThemeId::Vsix {
                        archive: name.clone(),
                        theme_id: theme.id,
                    },
                    label: format!("{} — {}", theme.label, name.display()),
                    theme: theme.theme.map(Arc::new),
                })
                .collect::<Vec<_>>(),
            Err(error) => vec![ThemeEntry {
                id: ThemeId::Vsix {
                    archive: name.clone(),
                    theme_id: String::new(),
                },
                label: format!("{} — invalid package", name.display()),
                theme: Err(format!("{error:#}")),
            }],
        };
        ensure!(!cancelled(), "Theme import cancelled");
        cache.insert(
            name,
            Cached {
                length: metadata.len(),
                modified,
                entries: imported.clone(),
            },
        );
        entries.extend(imported);
    }
    cache.retain(|name, _| present.contains(name));
    entries.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(entries)
}

fn validate_name(name: &Path) -> Result<()> {
    let mut parts = name.components();
    ensure!(
        matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none()
            && !name.to_string_lossy().contains([':', '/', '\\'])
            && name
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vsix")),
        "Theme selection must be a single VSIX file name"
    );
    Ok(())
}
