use std::{
    fs::{self, OpenOptions},
    iter::Peekable,
    path::{Component, Path, PathBuf},
    process,
    str::Chars,
};

use anyhow::{Context, Result, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, crate_description, crate_version};

const MAX_DEPTH: usize = 128;

#[derive(Debug, PartialEq)]
pub enum Node {
    File(String),
    Dir { name: String, children: Vec<Node> },
}

fn create_structure<P: AsRef<Path>>(base_dir: P, pattern: &str, dry_run: bool) -> Result<()> {
    let nodes = parse(pattern)?;

    validate_tree(base_dir.as_ref(), &nodes)?;
    apply_tree(base_dir.as_ref(), &nodes, dry_run)?;
    Ok(())
}

fn run(matches: &ArgMatches) -> Result<()> {
    let dry_run = matches.get_flag("dry-run");
    if let Some(pattern) = matches.get_one::<String>("pattern") {
        create_structure(".", pattern, dry_run)?;
    } else {
        println!("No pattern provided. Interactive TUI mode coming soon (try --help).");
    }

    Ok(())
}

fn main() {
    let matches = Command::new("loom")
        .version(crate_version!())
        .about(crate_description!())
        .arg(
            Arg::new("pattern").help("Structure pattern, e.g. 'dir/{file1, file2, dir 2/{file3}}'"),
        )
        .arg(
            Arg::new("dry-run")
                .short('d')
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Preview the structure without creating files"),
        )
        .get_matches();

    if let Err(e) = run(&matches) {
        eprintln!("Error: {:#}", e);
        process::exit(1);
    }
}

fn validate_tree(base: &Path, nodes: &[Node]) -> Result<()> {
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

// ----------------- PARSER -----------------

fn parse(input: &str) -> Result<Vec<Node>> {
    let mut chars = input.chars().peekable();
    let nodes = parse_list(&mut chars, 0)?;
    if chars.peek().is_some() {
        bail!("Unexpected closing brace '}}'");
    }
    Ok(nodes)
}

fn parse_list(chars: &mut Peekable<Chars>, depth: usize) -> Result<Vec<Node>> {
    if depth > MAX_DEPTH {
        bail!("Maximum nesting depth ({MAX_DEPTH}) exceeded");
    }

    let mut nodes = Vec::new();

    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() || ch == ',' {
            chars.next();
            continue;
        }

        if ch == '}' {
            break;
        }

        nodes.push(parse_item(chars, depth)?);
    }

    Ok(nodes)
}

fn parse_item(chars: &mut Peekable<Chars>, depth: usize) -> Result<Node> {
    if depth > MAX_DEPTH {
        bail!("Maximum nesting depth ({MAX_DEPTH}) exceeded");
    }

    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if matches!(c, '/' | '{' | '}' | ',') {
            break;
        }
        name.push(c);
        chars.next();
    }
    let name = name.trim().to_string();
    if name.is_empty() {
        bail!("Empty file or directory name");
    }

    if chars.peek() == Some(&'/') {
        chars.next();
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }

        let children = match chars.peek() {
            Some(&'{') => {
                chars.next();
                let inner = parse_list(chars, depth + 1)?;
                if chars.next() != Some('}') {
                    bail!("Unclosed opening brace '{{'");
                }
                inner
            }
            Some(&',' | &'}') | None => {
                vec![]
            }
            Some(_) => {
                vec![parse_item(chars, depth + 1)?]
            }
        };

        Ok(Node::Dir { name, children })
    } else if chars.peek() == Some(&'{') {
        chars.next();
        let children = parse_list(chars, depth + 1)?;
        if chars.next() != Some('}') {
            bail!("Unclosed opening brace '{{'");
        }
        Ok(Node::Dir { name, children })
    } else {
        Ok(Node::File(name))
    }
}

// ----------------- FILE SYSTEM -----------------

fn apply_tree(base: &Path, nodes: &[Node], dry_run: bool) -> Result<()> {
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
