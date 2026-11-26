use std::{
    fs::{self, OpenOptions},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::parser::Node;

pub(crate) fn create_file(directory: &Path, name: &str) -> Result<PathBuf> {
    let target = single_name_path(directory, name)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .with_context(|| format!("Failed to create file '{}'", target.display()))?;
    Ok(target)
}

pub(crate) fn create_directory(directory: &Path, name: &str) -> Result<PathBuf> {
    let target = single_name_path(directory, name)?;
    fs::create_dir(&target)
        .with_context(|| format!("Failed to create directory '{}'", target.display()))?;
    Ok(target)
}

pub(crate) fn delete_entry(base: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(base)
        .with_context(|| format!("Path is outside the tree: '{}'", target.display()))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!(
            "Cannot delete the tree root or an unsafe path: '{}'",
            target.display()
        );
    }
    // Resolve the parent, not the target: deleting a link must preserve its target.
    let parent = target
        .parent()
        .context("Missing parent directory")?
        .canonicalize()?;
    let root = base.canonicalize()?;
    if !parent.starts_with(&root) {
        bail!(
            "Parent directory is outside the tree: '{}'",
            target.display()
        );
    }

    let metadata = fs::symlink_metadata(target)
        .with_context(|| format!("Failed to inspect '{}'", target.display()))?;
    #[cfg(windows)]
    let is_directory_link = {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        const DIRECTORY: u32 = 0x10;
        let attributes = metadata.file_attributes();
        attributes & REPARSE_POINT != 0 && attributes & DIRECTORY != 0
    };
    #[cfg(not(windows))]
    let is_directory_link = false;

    let result = if is_directory_link {
        fs::remove_dir(target)
    } else if metadata.file_type().is_symlink() || !metadata.is_dir() {
        fs::remove_file(target)
    } else {
        fs::remove_dir_all(target)
    };
    result.with_context(|| format!("Failed to delete '{}'", target.display()))
}

fn single_name_path(directory: &Path, name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() || matches!(name, "." | "..") {
        bail!("Name must not be empty, '.' or '..'");
    }
    if name.contains(['/', '\\', '\0']) {
        bail!("Expected a single name, got {:?}", name);
    }
    safe_join(directory, name)
}

pub(crate) fn validate_tree(base: &Path, nodes: &[Node]) -> Result<()> {
    for node in nodes {
        match node {
            Node::File(name) => {
                safe_join(base, name)?;
            }
            Node::Dir { name, children } => {
                let target = safe_join(base, name)?;
                validate_tree(&target, children)?;
            }
        }
    }

    Ok(())
}

pub(crate) fn apply_tree(base: &Path, nodes: &[Node], dry_run: bool) -> Result<()> {
    for node in nodes {
        create_node(base, node, dry_run)?;
    }
    Ok(())
}

fn create_node(current_dir: &Path, node: &Node, dry_run: bool) -> Result<()> {
    match node {
        Node::File(name) => {
            let target = safe_join(current_dir, name)?;
            if dry_run {
                println!("  [FILE] {}", target.display());
                return Ok(());
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create directory '{}'", parent.display())
                })?;
            }
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&target)
                .with_context(|| format!("Failed to create file '{}'", target.display()))?;
        }
        Node::Dir { name, children } => {
            let target_dir = safe_join(current_dir, name)?;
            if dry_run {
                println!("  [DIR]  {}/", target_dir.display());
            } else {
                fs::create_dir_all(&target_dir).with_context(|| {
                    format!("Failed to create directory '{}'", target_dir.display())
                })?;
            }
            for child in children {
                create_node(&target_dir, child, dry_run)?;
            }
        }
    }
    Ok(())
}

fn safe_join(base: &Path, segment: &str) -> Result<PathBuf> {
    if segment.contains('\\') {
        bail!(
            "Invalid character '\\' in '{}'. Use '/' as separator.",
            segment
        );
    }

    #[cfg(windows)]
    validate_windows_name(segment)?;

    let path = Path::new(segment);
    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            bail!("Unsafe path segment: '{}'", segment);
        }
    }
    Ok(base.join(path))
}

#[cfg(windows)]
fn validate_windows_name(name: &str) -> Result<()> {
    if name
        .chars()
        .any(|c| c <= '\u{001F}' || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        bail!("Invalid character in name: {:?}", name);
    }

    if name.ends_with('.') || name.ends_with(' ') {
        bail!("Name must not end with a dot or space: {:?}", name);
    }

    let stem = name.split('.').next().unwrap_or("");
    let stem = stem.trim_end_matches(' ').to_ascii_uppercase();

    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });

    if reserved {
        bail!("Reserved Windows device name: {:?}", name);
    }

    Ok(())
}
