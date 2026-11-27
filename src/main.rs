use std::{path::Path, process};

use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches, Command, crate_description, crate_version};

mod files;
mod parser;
mod syntax;
mod theme_archive;
mod tui;
mod viewer;

use parser::parse;

use crate::files::{apply_tree, validate_tree};

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
        let mut app = tui::App::new(std::env::current_dir()?)?;
        ratatui::run(|terminal| app.run(terminal))?;
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
                .requires("pattern")
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
