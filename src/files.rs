use std::{
    fs::{self, OpenOptions},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::parser::Node;

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
