use std::{iter::Peekable, str::Chars};

use anyhow::{Result, bail};

const MAX_DEPTH: usize = 128;

#[derive(Debug, PartialEq)]
pub(crate) enum Node {
    File(String),
    Dir { name: String, children: Vec<Node> },
}

pub(crate) fn parse(input: &str) -> Result<Vec<Node>> {
    let mut chars = input.trim().chars().peekable();
    let grouped = chars.next_if_eq(&'{').is_some();
    let nodes = parse_list(&mut chars, 0)?;
    if grouped && chars.next() != Some('}') {
        bail!("Unclosed opening brace '{{'");
    }
    while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
        chars.next();
    }
    if chars.peek() == Some(&'}') {
        bail!("Unexpected closing brace '}}'");
    }
    if chars.peek().is_some() {
        bail!("Unexpected content after root group");
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
