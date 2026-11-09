//! Explicit section outlines from a painted regional SVG map.
//!
//! The caller binds the surrounding page to its event. Coordinates are SVG-local,
//! not screen coordinates or seat topology. Colors, active flags and empty seat
//! layers never supply availability, seat counts or adjacency evidence.
use anyhow::{Context, Result, ensure};
use scraper::{ElementRef, Selector};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_SECTIONS: usize = 1000;
const MAX_COPIES: usize = 4;
const MAX_PATH_BYTES: usize = 16 * 1024;
const MAX_TOTAL_PATH_BYTES: usize = 1024 * 1024;
const MAX_COMMANDS: usize = 512;
const MAX_COORDINATE: f64 = 1_000_000.0;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct ViewBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum PathCommand {
    MoveTo {
        point: Point,
    },
    LineTo {
        point: Point,
    },
    CubicTo {
        control1: Point,
        control2: Point,
        point: Point,
    },
    Close,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct LabelRotation {
    pub degrees: f64,
    pub cx: f64,
    pub cy: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LabelLine {
    pub text: String,
    /// Declared SVG tspan anchor before the optional rotation and em offset.
    pub anchor: Point,
    pub dy_em: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SectionLabel {
    pub lines: Vec<LabelLine>,
    pub font_size: f64,
    pub rotation: Option<LabelRotation>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SectionGeometry {
    pub id: String,
    pub name: String,
    pub path: Vec<PathCommand>,
    pub label: Option<SectionLabel>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MapGeometry {
    pub evidence: &'static str,
    pub coordinate_space: &'static str,
    pub view_box: ViewBox,
    pub sections: Vec<SectionGeometry>,
    pub rendered_copies: usize,
    pub geometry_fingerprint: String,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
    pub individual_seats_verified: bool,
}

/// The surrounding event and map region must already be validated by seat_map.
/// A legacy capture without computed paint provenance supplies no geometry.
pub fn extract_svg_geometry(region: ElementRef<'_>) -> Result<Option<MapGeometry>> {
    if crate::parser::hidden_element(region) || blocked_ancestor(region) {
        return Ok(None);
    }
    let roots: Vec<_> = region
        .select(&select("svg[data-component='svg']"))
        .filter(|svg| svg.value().attr("data-ticketwatch-map-painted") == Some("true"))
        .collect();
    ensure!(roots.len() <= MAX_COPIES, "too many painted map SVGs");
    let mut result: Option<MapGeometry> = None;
    let mut total_bytes = 0usize;
    for svg in roots {
        // Only the root's accessibility-hidden attribute may be bypassed by
        // provenance. Native hidden and parent or descendant hiding still wins.
        if blocked_ancestor(svg) || own_hidden(svg, true) {
            continue;
        }
        ensure!(
            svg.ancestors()
                .filter_map(ElementRef::wrap)
                .all(|e| e.value().name() != "svg"),
            "nested map SVG is unsupported"
        );
        no_transform(svg)?;
        let view_box = parse_view_box(
            svg.value()
                .attr("viewBox")
                .or_else(|| svg.value().attr("viewbox"))
                .context("map SVG lacks a viewBox")?,
        )?;
        let sections = read_sections(svg, view_box, &mut total_bytes)?;
        let fingerprint = format!(
            "geometry-{:x}",
            Sha256::digest(serde_json::to_vec(&(view_box, &sections))?)
        );
        if let Some(previous) = &mut result {
            ensure!(
                previous.view_box == view_box && previous.sections == sections,
                "conflicting painted map geometries"
            );
            previous.rendered_copies += 1;
        } else {
            result = Some(MapGeometry {
                evidence: "rendered_svg_sections",
                coordinate_space: "svg_view_box",
                view_box,
                sections,
                rendered_copies: 1,
                geometry_fingerprint: fingerprint,
                stock_verified: false,
                adjacency_verified: false,
                individual_seats_verified: false,
            });
        }
    }
    Ok(result)
}

fn read_sections(
    svg: ElementRef<'_>,
    bounds: ViewBox,
    total_bytes: &mut usize,
) -> Result<Vec<SectionGeometry>> {
    ensure!(
        svg.select(&select("svg, foreignObject, use"))
            .next()
            .is_none(),
        "unsupported map SVG composition"
    );
    let groups: Vec<_> = svg
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().name() == "g" && e.value().classes().any(|c| c == "polygons"))
        .collect();
    ensure!(
        groups.len() == 1 && !own_hidden(groups[0], false),
        "missing or hidden section polygon layer"
    );
    no_transform(groups[0])?;
    let mut sections = BTreeMap::new();
    for element in groups[0].children().filter_map(ElementRef::wrap) {
        ensure!(
            element.value().name() == "path"
                && element.value().attr("data-component") == Some("svg__section"),
            "unsupported map section element"
        );
        ensure!(!own_hidden(element, false), "hidden map section geometry");
        no_transform(element)?;
        ensure!(sections.len() < MAX_SECTIONS, "too many map sections");
        let id = identifier(
            element
                .value()
                .attr("data-section-id")
                .context("missing map section ID")?,
        )?;
        let name = label(
            element
                .value()
                .attr("data-section-name")
                .context("missing map section name")?,
        )?;
        let raw_path = element
            .value()
            .attr("d")
            .context("missing map section path")?;
        *total_bytes = total_bytes
            .checked_add(raw_path.len())
            .context("map path size overflow")?;
        ensure!(
            *total_bytes <= MAX_TOTAL_PATH_BYTES,
            "map geometry exceeds size limit"
        );
        let path = parse_path(raw_path, bounds)?;
        ensure!(
            sections
                .insert(
                    id.clone(),
                    SectionGeometry {
                        id,
                        name,
                        path,
                        label: None
                    }
                )
                .is_none(),
            "duplicate map section ID"
        );
    }
    ensure!(
        !sections.is_empty(),
        "painted map has no explicit section paths"
    );
    // Reject recognized paths outside the parsed direct polygon layer rather
    // than silently publishing an apparently complete subset.
    ensure!(
        svg.select(&select("[data-component='svg__section']"))
            .count()
            == sections.len(),
        "unbound or nested map section geometry"
    );
    let label_groups: Vec<_> = svg
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().name() == "g" && e.value().classes().any(|c| c == "labels"))
        .collect();
    ensure!(label_groups.len() <= 1, "ambiguous map label layers");
    if let Some(group) = label_groups.first() {
        ensure!(!own_hidden(*group, false), "hidden map labels");
        no_transform(*group)?;
        for text in group.children().filter_map(ElementRef::wrap) {
            ensure!(
                text.value().name() == "text"
                    && text.value().attr("data-component") == Some("svg__label")
                    && !own_hidden(text, false),
                "unsupported or hidden map label"
            );
            let id = identifier(
                text.value()
                    .attr("data-label-id")
                    .context("missing map label ID")?,
            )?;
            let section = sections
                .get_mut(&id)
                .context("map label has no matching section")?;
            ensure!(section.label.is_none(), "duplicate map section label");
            let value = read_label(text, bounds)?;
            let rendered_name = value
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            ensure!(
                rendered_name == section.name,
                "map label and section name disagree"
            );
            section.label = Some(value);
        }
    }
    ensure!(
        svg.select(&select("[data-component='svg__label']")).count()
            == sections
                .values()
                .filter(|section| section.label.is_some())
                .count(),
        "unbound or nested map section labels"
    );
    Ok(sections.into_values().collect())
}

fn read_label(text: ElementRef<'_>, bounds: ViewBox) -> Result<SectionLabel> {
    no_style_override(text)?;
    ensure!(
        text.children().all(|node| node
            .value()
            .as_text()
            .is_none_or(|value| value.trim().is_empty())),
        "unsupported direct map label text"
    );
    let font_size = scalar(
        text.value()
            .attr("font-size")
            .context("missing map label font size")?,
    )?;
    ensure!(
        font_size > 0.0 && font_size <= bounds.width.max(bounds.height),
        "invalid map label font size"
    );
    let rotation = text
        .value()
        .attr("transform")
        .map(|raw| {
            let inner = raw
                .trim()
                .strip_prefix("rotate(")
                .and_then(|s| s.strip_suffix(')'))
                .context("unsupported map label transform")?;
            let numbers = number_list(inner)?;
            ensure!(
                numbers.len() == 3 && numbers[0].abs() <= 360.0,
                "invalid map label rotation"
            );
            point(numbers[1], numbers[2], bounds)?;
            Ok(LabelRotation {
                degrees: numbers[0],
                cx: numbers[1],
                cy: numbers[2],
            })
        })
        .transpose()?;
    let mut lines = Vec::new();
    let children: Vec<_> = text.children().filter_map(ElementRef::wrap).collect();
    ensure!(children.len() <= 8, "too many map label lines");
    for span in children {
        ensure!(
            span.value().name() == "tspan"
                && !own_hidden(span, false)
                && span
                    .children()
                    .all(|node| node.value().as_element().is_none()),
            "unsupported map label line"
        );
        no_transform(span)?;
        let raw = span.text().collect::<Vec<_>>().join(" ");
        if raw.trim().is_empty() {
            continue;
        }
        let value = label(&raw)?;
        let anchor = point(
            scalar(span.value().attr("x").context("missing label x")?)?,
            scalar(span.value().attr("y").context("missing label y")?)?,
            bounds,
        )?;
        let dy_em = scalar(
            span.value()
                .attr("dy")
                .and_then(|s| s.strip_suffix("em"))
                .context("unsupported map label offset")?,
        )?;
        ensure!(
            (-8.0..=8.0).contains(&dy_em),
            "map label offset exceeds limit"
        );
        lines.push(LabelLine {
            text: value,
            anchor,
            dy_em,
        });
    }
    ensure!(!lines.is_empty(), "empty map section label");
    Ok(SectionLabel {
        lines,
        font_size,
        rotation,
    })
}

fn blocked_ancestor(element: ElementRef<'_>) -> bool {
    element.ancestors().filter_map(ElementRef::wrap).any(|e| {
        own_hidden(e, false) || e.value().attr("data-ticketwatch-native-modal") == Some("open")
    })
}

fn own_hidden(element: ElementRef<'_>, painted_root: bool) -> bool {
    let e = element.value();
    e.attr("inert").is_some()
        || e.attr("data-ticketwatch-native-modal") == Some("open")
        || e.attr("hidden").is_some()
        || (!painted_root && e.attr("aria-hidden") == Some("true"))
        || e.attr("style").is_some_and(crate::parser::inline_hidden)
}

fn no_transform(element: ElementRef<'_>) -> Result<()> {
    ensure!(
        element.value().attr("transform").is_none(),
        "transformed map geometry is unsupported"
    );
    no_style_override(element)?;
    Ok(())
}

fn no_style_override(element: ElementRef<'_>) -> Result<()> {
    let style = element.value().attr("style").unwrap_or("");
    ensure!(
        !style
            .split(';')
            .filter_map(|value| value.split_once(':'))
            .any(|(name, _)| {
                matches!(
                    name.trim().to_ascii_lowercase().as_str(),
                    "transform"
                        | "transform-origin"
                        | "d"
                        | "x"
                        | "y"
                        | "cx"
                        | "cy"
                        | "r"
                        | "font-size"
                        | "text-anchor"
                )
            }),
        "CSS geometry overrides are unsupported"
    );
    Ok(())
}

fn select(value: &str) -> Selector {
    Selector::parse(value).expect("constant geometry selector")
}
fn identifier(value: &str) -> Result<String> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')),
        "invalid map section identifier"
    );
    Ok(value.to_owned())
}
fn label(value: &str) -> Result<String> {
    ensure!(
        !value.chars().any(|c| c.is_control()) && value.len() <= 512,
        "invalid map section label"
    );
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(
        !value.is_empty() && value.len() <= 128,
        "invalid map section label length"
    );
    Ok(value)
}
fn scalar(raw: &str) -> Result<f64> {
    let tokens = tokens(raw)?;
    ensure!(tokens.len() == 1, "invalid map scalar");
    match tokens[0] {
        Token::Number(value) => Ok(value),
        _ => anyhow::bail!("invalid map scalar"),
    }
}
fn number_list(raw: &str) -> Result<Vec<f64>> {
    tokens(raw)?
        .into_iter()
        .map(|token| match token {
            Token::Number(value) => Ok(value),
            _ => anyhow::bail!("invalid numeric map list"),
        })
        .collect()
}
fn parse_view_box(raw: &str) -> Result<ViewBox> {
    let values = number_list(raw)?;
    ensure!(values.len() == 4, "invalid map viewBox");
    let result = ViewBox {
        x: values[0],
        y: values[1],
        width: values[2],
        height: values[3],
    };
    validate_view_box(result)?;
    Ok(result)
}
fn validate_view_box(value: ViewBox) -> Result<()> {
    ensure!(
        [value.x, value.y, value.width, value.height]
            .iter()
            .all(|n| n.is_finite() && n.abs() <= MAX_COORDINATE)
            && value.width > 0.0
            && value.height > 0.0
            && (value.x + value.width).abs() <= MAX_COORDINATE
            && (value.y + value.height).abs() <= MAX_COORDINATE,
        "invalid map viewBox bounds"
    );
    Ok(())
}
fn point(x: f64, y: f64, bounds: ViewBox) -> Result<Point> {
    ensure!(
        x.is_finite()
            && y.is_finite()
            && (bounds.x..=bounds.x + bounds.width).contains(&x)
            && (bounds.y..=bounds.y + bounds.height).contains(&y),
        "map coordinate lies outside viewBox"
    );
    Ok(Point {
        x: if x == 0.0 { 0.0 } else { x },
        y: if y == 0.0 { 0.0 } else { y },
    })
}

#[derive(Clone, Copy)]
enum Token {
    Command(u8),
    Number(f64),
}

/// A strict bounded subset of SVG grammar: absolute M, L, C and Z only.
fn tokens(raw: &str) -> Result<Vec<Token>> {
    ensure!(
        raw.len() <= MAX_PATH_BYTES && raw.is_ascii(),
        "map numeric input exceeds bounds"
    );
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut result = Vec::new();
    let mut previous_number = false;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index] == b',' {
            ensure!(previous_number, "invalid SVG separator");
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            ensure!(
                index < bytes.len() && !bytes[index].is_ascii_alphabetic() && bytes[index] != b',',
                "invalid SVG separator"
            );
        }
        if bytes[index].is_ascii_alphabetic() {
            ensure!(
                matches!(bytes[index], b'M' | b'L' | b'C' | b'Z'),
                "unsupported SVG path command"
            );
            result.push(Token::Command(bytes[index]));
            index += 1;
            previous_number = false;
        } else {
            let start = index;
            if matches!(bytes[index], b'+' | b'-') {
                index += 1;
            }
            let mut digits = 0;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                digits += 1;
                index += 1;
            }
            if bytes.get(index) == Some(&b'.') {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    digits += 1;
                    index += 1;
                }
            }
            ensure!(digits > 0, "invalid SVG number");
            if matches!(bytes.get(index), Some(b'e' | b'E')) {
                index += 1;
                if matches!(bytes.get(index), Some(b'+' | b'-')) {
                    index += 1;
                }
                let exp = index;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                ensure!(index > exp, "invalid SVG exponent");
            }
            let value: f64 = raw[start..index]
                .parse()
                .context("invalid SVG numeric value")?;
            ensure!(
                value.is_finite() && value.abs() <= MAX_COORDINATE,
                "SVG number exceeds bounds"
            );
            if index < bytes.len() {
                ensure!(
                    bytes[index].is_ascii_whitespace()
                        || bytes[index] == b','
                        || bytes[index].is_ascii_alphabetic()
                        || matches!(bytes[index], b'+' | b'-'),
                    "invalid SVG numeric separation"
                );
            }
            result.push(Token::Number(if value == 0.0 { 0.0 } else { value }));
            previous_number = true;
        }
        ensure!(result.len() <= MAX_COMMANDS * 7, "too many SVG path values");
    }
    Ok(result)
}

fn parse_path(raw: &str, bounds: ViewBox) -> Result<Vec<PathCommand>> {
    let input = tokens(raw)?;
    let mut index = 0;
    let mut path = Vec::new();
    while index < input.len() {
        let Token::Command(command) = input[index] else {
            anyhow::bail!("SVG path requires explicit commands");
        };
        index += 1;
        if command == b'Z' {
            ensure!(
                path.len() >= 3 && index == input.len(),
                "SVG outline must end with one close command"
            );
            path.push(PathCommand::Close);
            break;
        }
        ensure!(
            if path.is_empty() {
                command == b'M'
            } else {
                command != b'M'
            },
            "SVG outline requires one initial move"
        );
        let arity = if command == b'C' { 6 } else { 2 };
        let mut values = Vec::with_capacity(arity);
        for _ in 0..arity {
            let Some(Token::Number(value)) = input.get(index) else {
                anyhow::bail!("incomplete SVG path command");
            };
            values.push(*value);
            index += 1;
        }
        path.push(match command {
            b'M' => PathCommand::MoveTo {
                point: point(values[0], values[1], bounds)?,
            },
            b'L' => PathCommand::LineTo {
                point: point(values[0], values[1], bounds)?,
            },
            b'C' => PathCommand::CubicTo {
                control1: point(values[0], values[1], bounds)?,
                control2: point(values[2], values[3], bounds)?,
                point: point(values[4], values[5], bounds)?,
            },
            _ => anyhow::bail!("unsupported SVG outline command"),
        });
        ensure!(path.len() < MAX_COMMANDS, "too many SVG outline commands");
    }
    ensure!(
        matches!(path.last(), Some(PathCommand::Close)),
        "open SVG outline is unsupported"
    );
    Ok(path)
}

/// Build a standalone reference drawing from validated typed coordinates only.
/// Source styles, colors, URLs, SVG markup and interaction handlers are omitted.
pub fn render_svg(geometry: &MapGeometry) -> Result<String> {
    use std::fmt::Write;
    validate_view_box(geometry.view_box)?;
    ensure!(
        !geometry.sections.is_empty() && geometry.sections.len() <= MAX_SECTIONS,
        "invalid geometry section count"
    );
    let b = geometry.view_box;
    let mut output = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\" role=\"img\" aria-label=\"Section layout reference\"><title>Section layout reference</title><desc>Section outlines only. No live availability, seat counts, or adjacency is established.</desc><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"white\"/>",
        b.x, b.y, b.width, b.height, b.x, b.y, b.width, b.height
    );
    let mut ids = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for section in &geometry.sections {
        identifier(&section.id)?;
        label(&section.name)?;
        ensure!(ids.insert(&section.id), "duplicate geometry section ID");
        let path = path_string(&section.path, b)?;
        total += path.len();
        ensure!(
            total <= MAX_TOTAL_PATH_BYTES,
            "geometry export exceeds size limit"
        );
        write!(
            output,
            "<path d=\"{}\" fill=\"#e6e9ed\" stroke=\"#334155\" stroke-width=\"{}\"><title>{}</title></path>",
            path,
            b.width.max(b.height) / 3000.0,
            xml(&section.name)
        )?;
        if let Some(text) = &section.label {
            ensure!(
                !text.lines.is_empty()
                    && text.lines.len() <= 8
                    && text.font_size.is_finite()
                    && text.font_size > 0.0
                    && text.font_size <= b.width.max(b.height),
                "invalid geometry label"
            );
            ensure!(
                text.lines
                    .iter()
                    .map(|line| line.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
                    == section.name,
                "geometry label and section name disagree"
            );
            write!(
                output,
                "<text fill=\"#111827\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"{}\"",
                text.font_size
            )?;
            if let Some(r) = text.rotation {
                ensure!(
                    r.degrees.is_finite() && r.degrees.abs() <= 360.0,
                    "invalid geometry label rotation"
                );
                point(r.cx, r.cy, b)?;
                write!(
                    output,
                    " transform=\"rotate({},{},{})\"",
                    r.degrees, r.cx, r.cy
                )?;
            }
            output.push('>');
            for line in &text.lines {
                label(&line.text)?;
                point(line.anchor.x, line.anchor.y, b)?;
                ensure!(
                    line.dy_em.is_finite() && (-8.0..=8.0).contains(&line.dy_em),
                    "invalid geometry label offset"
                );
                write!(
                    output,
                    "<tspan x=\"{}\" y=\"{}\" dy=\"{}em\">{}</tspan>",
                    line.anchor.x,
                    line.anchor.y,
                    line.dy_em,
                    xml(&line.text)
                )?;
            }
            output.push_str("</text>");
        }
    }
    output.push_str("</svg>\n");
    ensure!(
        output.len() <= 2 * MAX_TOTAL_PATH_BYTES,
        "geometry export exceeds size limit"
    );
    Ok(output)
}

fn path_string(path: &[PathCommand], bounds: ViewBox) -> Result<String> {
    use std::fmt::Write;
    ensure!(
        path.len() >= 4 && path.len() <= MAX_COMMANDS,
        "invalid geometry outline length"
    );
    let mut result = String::new();
    for (index, command) in path.iter().enumerate() {
        match command {
            PathCommand::MoveTo { point: p } => {
                ensure!(index == 0, "invalid geometry move");
                point(p.x, p.y, bounds)?;
                write!(result, "M{},{}", p.x, p.y)?;
            }
            PathCommand::LineTo { point: p } => {
                ensure!(index > 0 && index + 1 < path.len(), "invalid geometry line");
                point(p.x, p.y, bounds)?;
                write!(result, "L{},{}", p.x, p.y)?;
            }
            PathCommand::CubicTo {
                control1: a,
                control2: b,
                point: c,
            } => {
                ensure!(
                    index > 0 && index + 1 < path.len(),
                    "invalid geometry curve"
                );
                for p in [a, b, c] {
                    point(p.x, p.y, bounds)?;
                }
                write!(result, "C{},{},{},{},{},{}", a.x, a.y, b.x, b.y, c.x, c.y)?;
            }
            PathCommand::Close => {
                ensure!(index + 1 == path.len(), "invalid geometry close");
                result.push('Z');
            }
        }
    }
    ensure!(
        matches!(path.first(), Some(PathCommand::MoveTo { .. }))
            && matches!(path.last(), Some(PathCommand::Close)),
        "unclosed geometry outline"
    );
    Ok(result)
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
