//! Resolve SVG paints without loading external files, images, or fonts.
use ratatui::style::Color;
use std::borrow::Cow;

pub(super) fn primary_color(data: &[u8]) -> Option<Color> {
    let svg = without_variable_paints(std::str::from_utf8(data).ok()?)?;
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        max_depth: 128,
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(&svg, &options).ok()?;
    let mut first = None;
    visit(tree.root(), &mut first).or(first)
}

// usvg cannot resolve CSS variables. Make only those paints transparent so
// its normal CSS cascade still decides which declarations affect visible paths.
fn without_variable_paints(svg: &str) -> Option<Cow<'_, str>> {
    if !svg.contains("var(") {
        return Some(Cow::Borrowed(svg));
    }
    let document = roxmltree::Document::parse(svg).ok()?;
    let mut replacements = Vec::new();
    for node in document.descendants().filter(|node| node.is_element()) {
        for attribute in node.attributes() {
            if let Some(value) = variable_paint(attribute.name(), attribute.value()) {
                replacements.push((
                    attribute.range(),
                    format!("{}=\"{value}\"", attribute.name()),
                ));
            } else if attribute.name() == "style" && attribute.value().contains("var(") {
                let style = simplecss::DeclarationTokenizer::from(attribute.value())
                    .map(|declaration| {
                        let value = variable_paint(declaration.name, declaration.value)
                            .unwrap_or(declaration.value);
                        format!(
                            "{}:{value}{};",
                            declaration.name,
                            if declaration.important {
                                " !important"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<String>();
                replacements.push((
                    attribute.range(),
                    format!("style=\"{}\"", escape_xml(&style, true)),
                ));
            }
        }
        if node.tag_name().name() == "style" {
            let css = node
                .children()
                .filter_map(|node| node.text())
                .collect::<String>();
            if !css.contains("var(") {
                continue;
            }
            let mut sheet = simplecss::StyleSheet::parse(&css);
            let mut changed = false;
            for rule in &mut sheet.rules {
                for declaration in &mut rule.declarations {
                    if let Some(value) = variable_paint(declaration.name, declaration.value) {
                        declaration.value = value;
                        changed = true;
                    }
                }
            }
            if changed {
                replacements.push((
                    node.range(),
                    format!("<style>{}</style>", escape_xml(&sheet.to_string(), false)),
                ));
            }
        }
    }
    if replacements.is_empty() {
        return Some(Cow::Borrowed(svg));
    }
    // Ranges refer to the original XML. Replacing from the end keeps them valid.
    replacements.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut output = svg.to_owned();
    let mut next = svg.len();
    for (range, value) in replacements {
        if range.end <= next {
            next = range.start;
            output.replace_range(range, &value);
        }
    }
    Some(Cow::Owned(output))
}

fn variable_paint(property: &str, value: &str) -> Option<&'static str> {
    if !value.contains("var(") {
        return None;
    }
    match property {
        "fill" | "stroke" => Some("none"),
        "color" | "stop-color" => Some("transparent"),
        _ => None,
    }
}

fn escape_xml(value: &str, attribute: bool) -> String {
    let value = value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    if attribute {
        value.replace('"', "&quot;")
    } else {
        value
    }
}

fn visit(group: &usvg::Group, first: &mut Option<Color>) -> Option<Color> {
    if group.opacity().get() == 0.0 {
        return None;
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(group) => {
                if let Some(color) = visit(group, first) {
                    return Some(color);
                }
            }
            usvg::Node::Path(path) if path.is_visible() => {
                let paints = [
                    path.fill()
                        .filter(|p| p.opacity().get() > 0.0)
                        .map(|p| p.paint()),
                    path.stroke()
                        .filter(|p| p.opacity().get() > 0.0)
                        .map(|p| p.paint()),
                ];
                for paint in paints.into_iter().flatten() {
                    if let Some(color) = paint_color(paint) {
                        let rgb = Color::Rgb(color.red, color.green, color.blue);
                        first.get_or_insert(rgb);
                        if color != usvg::Color::black() && color != usvg::Color::white() {
                            return Some(rgb);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn paint_color(paint: &usvg::Paint) -> Option<usvg::Color> {
    match paint {
        usvg::Paint::Color(color) => Some(*color),
        usvg::Paint::LinearGradient(gradient) => gradient
            .stops()
            .iter()
            .find(|stop| stop.opacity().get() > 0.0)
            .map(|stop| stop.color()),
        usvg::Paint::RadialGradient(gradient) => gradient
            .stops()
            .iter()
            .find(|stop| stop.opacity().get() > 0.0)
            .map(|stop| stop.color()),
        usvg::Paint::Pattern(_) => None,
    }
}
