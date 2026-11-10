//! Seat identities and local coordinates exposed by a rendered regional map.
//!
//! These records describe the current DOM, which may contain only nearby blocks
//! after zooming. They do not establish availability, purchasable quantities,
//! physical adjacency, or complete venue coverage.
use anyhow::{Context, Result, ensure};
use scraper::{ElementRef, Selector};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::map_geometry::{Point, ViewBox};

const MAX_COPIES: usize = 4;
const MAX_SECTIONS: usize = 1000;
const MAX_ROWS: usize = 2000;
const MAX_SEATS: usize = 30_000;
const MAX_COORDINATE: f64 = 1_000_000.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeatShape {
    Circle {
        radius: f64,
    },
    /// Center/radius come from the icon's explicit circle; paths are not copied.
    ResaleIcon {
        scale: f64,
        radius: f64,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SeatPoint {
    pub id: String,
    pub name: String,
    /// Circle center in the map's SVG coordinate space, including icon transform.
    pub position: Point,
    pub shape: SeatShape,
    /// An explicit display attribute, never a claim that this seat is purchasable.
    pub displayed_ticket_type: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SeatRow {
    pub name: String,
    pub seats: Vec<SeatPoint>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SeatSection {
    pub id: String,
    pub name: String,
    pub rows: Vec<SeatRow>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RenderedSeats {
    pub evidence: &'static str,
    pub coordinate_space: &'static str,
    pub view_box: ViewBox,
    pub sections: Vec<SeatSection>,
    pub rendered_copies: usize,
    pub rendered_seat_count: usize,
    pub geometry_fingerprint: String,
    pub coverage: &'static str,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
    pub viewport_coverage_verified: bool,
}

/// The caller must bind the event and map region before reading seat identities.
/// Missing, empty or hidden seat layers do not imply that an event is sold out.
pub fn extract_rendered_seats(region: ElementRef<'_>) -> Result<Option<RenderedSeats>> {
    if crate::parser::hidden_element(region) || blocked_ancestor(region) {
        return Ok(None);
    }
    let roots: Vec<_> = region
        .select(&select("svg[data-component='svg']"))
        .filter(|svg| svg.value().attr("data-ticketwatch-map-painted") == Some("true"))
        .collect();
    ensure!(roots.len() <= MAX_COPIES, "too many painted seat maps");
    let mut result: Option<RenderedSeats> = None;
    let mut total_seats = 0usize;
    for svg in roots {
        if blocked_ancestor(svg) || own_hidden(svg, true) {
            continue;
        }
        ensure!(
            svg.ancestors()
                .filter_map(ElementRef::wrap)
                .all(|e| e.value().name() != "svg"),
            "nested seat map SVG is unsupported"
        );
        no_transform(svg)?;
        let layers: Vec<_> = svg
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|e| e.value().name() == "g" && e.value().classes().any(|c| c == "seats"))
            .collect();
        ensure!(layers.len() <= 1, "ambiguous seat layers");
        let recognized = svg.select(&select("[data-component='svg__seat']")).count();
        if layers.is_empty() {
            ensure!(recognized == 0, "seat elements have no direct seat layer");
            continue;
        }
        let layer = layers[0];
        if own_hidden(layer, false) {
            continue;
        }
        no_transform(layer)?;
        if layer
            .children()
            .filter_map(ElementRef::wrap)
            .next()
            .is_none()
        {
            ensure!(recognized == 0, "seat elements lie outside the seat layer");
            continue;
        }
        let view_box = read_view_box(svg)?;
        let (sections, seat_count) = read_sections(layer, view_box, &mut total_seats)?;
        ensure!(recognized == seat_count, "unbound or nested seat elements");
        if seat_count == 0 {
            continue;
        }
        // Displayed type/icon styling is deliberately excluded from the layout
        // fingerprint: an offer change is not a new seating-layout identity.
        let identity: Vec<_> = sections
            .iter()
            .map(|section| {
                (
                    &section.id,
                    &section.name,
                    section
                        .rows
                        .iter()
                        .map(|row| {
                            (
                                &row.name,
                                row.seats
                                    .iter()
                                    .map(|seat| (&seat.id, &seat.name, seat.position))
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let fingerprint = format!(
            "seat-geometry-{:x}",
            Sha256::digest(serde_json::to_vec(&(view_box, identity))?)
        );
        if let Some(previous) = &mut result {
            ensure!(
                previous.view_box == view_box && previous.sections == sections,
                "conflicting rendered seat maps"
            );
            previous.rendered_copies += 1;
        } else {
            result = Some(RenderedSeats {
                evidence: "rendered_svg_seat_identities",
                coordinate_space: "svg_view_box",
                view_box,
                sections,
                rendered_copies: 1,
                rendered_seat_count: seat_count,
                geometry_fingerprint: fingerprint,
                coverage: "partial_rendered_dom",
                stock_verified: false,
                adjacency_verified: false,
                viewport_coverage_verified: false,
            });
        }
    }
    Ok(result)
}

fn read_sections(
    layer: ElementRef<'_>,
    bounds: ViewBox,
    total_seats: &mut usize,
) -> Result<(Vec<SeatSection>, usize)> {
    let mut sections = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut row_count = 0usize;
    for block in layer.children().filter_map(ElementRef::wrap) {
        ensure!(
            block.value().name() == "g"
                && block.value().attr("data-component") == Some("svg__block")
                && !own_hidden(block, false),
            "unsupported or hidden seat block"
        );
        no_transform(block)?;
        ensure!(sections.len() < MAX_SECTIONS, "too many seat sections");
        let id = identifier(
            block
                .value()
                .attr("data-section-id")
                .context("seat block lacks ID")?,
        )?;
        let name = label(
            block
                .value()
                .attr("data-section-name")
                .context("seat block lacks name")?,
        )?;
        let mut rows = BTreeMap::new();
        for row in block.children().filter_map(ElementRef::wrap) {
            ensure!(
                row.value().name() == "g" && !own_hidden(row, false),
                "unsupported or hidden seat row"
            );
            no_transform(row)?;
            let row_name = label(
                row.value()
                    .attr("data-row-name")
                    .context("seat row lacks name")?,
            )?;
            row_count += 1;
            ensure!(row_count <= MAX_ROWS, "too many seat rows");
            let mut seats = BTreeMap::new();
            let mut names = BTreeSet::new();
            for mark in row.children().filter_map(ElementRef::wrap) {
                ensure!(
                    mark.value().attr("data-component") == Some("svg__seat")
                        && !own_hidden(mark, false),
                    "unsupported or hidden seat mark"
                );
                *total_seats += 1;
                ensure!(*total_seats <= MAX_SEATS, "too many rendered seats");
                let seat = read_seat(mark, bounds)?;
                ensure!(ids.insert(seat.id.clone()), "duplicate rendered seat ID");
                ensure!(
                    names.insert(seat.name.clone()),
                    "duplicate seat name in row"
                );
                seats.insert(seat.id.clone(), seat);
            }
            ensure!(!seats.is_empty(), "empty rendered seat row");
            ensure!(
                rows.insert(
                    row_name.clone(),
                    SeatRow {
                        name: row_name,
                        seats: seats.into_values().collect()
                    }
                )
                .is_none(),
                "duplicate rendered seat row"
            );
        }
        ensure!(!rows.is_empty(), "empty rendered seat block");
        ensure!(
            sections
                .insert(
                    id.clone(),
                    SeatSection {
                        id,
                        name,
                        rows: rows.into_values().collect()
                    }
                )
                .is_none(),
            "duplicate rendered seat section"
        );
    }
    Ok((sections.into_values().collect(), ids.len()))
}

fn read_seat(mark: ElementRef<'_>, bounds: ViewBox) -> Result<SeatPoint> {
    no_style_override(mark)?;
    let e = mark.value();
    let id = identifier(e.attr("id").context("seat mark lacks ID")?)?;
    let name = label(e.attr("data-seat-name").context("seat mark lacks name")?)?;
    let (position, shape, displayed_ticket_type) = match e.name() {
        "circle" => {
            no_transform(mark)?;
            ensure!(
                mark.children()
                    .filter_map(ElementRef::wrap)
                    .next()
                    .is_none(),
                "nested seat circle content"
            );
            ensure!(e.attr("type").is_none(), "unsupported circle ticket type");
            let position = point(
                scalar(e.attr("cx").context("seat circle lacks x")?)?,
                scalar(e.attr("cy").context("seat circle lacks y")?)?,
                bounds,
            )?;
            let radius = scalar(e.attr("r").context("seat circle lacks radius")?)?;
            ensure!(
                radius > 0.0 && radius <= bounds.width.min(bounds.height),
                "invalid seat circle radius"
            );
            (position, SeatShape::Circle { radius }, None)
        }
        "g" => {
            ensure!(
                e.attr("type") == Some("resale"),
                "unsupported seat icon type"
            );
            let children: Vec<_> = mark.children().filter_map(ElementRef::wrap).collect();
            ensure!(children.len() == 2, "unsupported seat icon composition");
            let circles: Vec<_> = children
                .iter()
                .copied()
                .filter(|c| c.value().name() == "circle")
                .collect();
            let paths: Vec<_> = children
                .iter()
                .copied()
                .filter(|c| c.value().name() == "path")
                .collect();
            ensure!(
                circles.len() == 1 && paths.len() == 1,
                "unsupported seat icon composition"
            );
            for child in &children {
                ensure!(
                    !own_hidden(*child, false)
                        && child
                            .children()
                            .filter_map(ElementRef::wrap)
                            .next()
                            .is_none(),
                    "hidden or nested seat icon content"
                );
                no_transform(*child)?;
            }
            let circle = circles[0].value();
            let local_x = scalar(circle.attr("cx").context("seat icon circle lacks x")?)?;
            let local_y = scalar(circle.attr("cy").context("seat icon circle lacks y")?)?;
            let local_radius = scalar(circle.attr("r").context("seat icon circle lacks radius")?)?;
            ensure!(local_radius > 0.0, "invalid seat icon radius");
            let transform = e.attr("transform").context("seat icon lacks position")?;
            let inner = transform
                .trim()
                .strip_prefix("translate(")
                .context("unsupported seat icon transform")?;
            let (translation, scale) = inner
                .split_once(')')
                .context("invalid seat icon translation")?;
            let translation = number_list(translation)?;
            ensure!(translation.len() == 2, "invalid seat icon translation");
            let scale = scalar(
                scale
                    .trim()
                    .strip_prefix("scale(")
                    .and_then(|s| s.strip_suffix(')'))
                    .context("unsupported seat icon scale")?,
            )?;
            ensure!(scale > 0.0 && scale <= 100.0, "invalid seat icon scale");
            let radius = local_radius * scale;
            ensure!(
                radius.is_finite() && radius > 0.0 && radius <= bounds.width.min(bounds.height),
                "invalid seat icon radius"
            );
            (
                point(
                    translation[0] + local_x * scale,
                    translation[1] + local_y * scale,
                    bounds,
                )?,
                SeatShape::ResaleIcon { scale, radius },
                Some("resale"),
            )
        }
        _ => anyhow::bail!("unsupported rendered seat shape"),
    };
    Ok(SeatPoint {
        id,
        name,
        position,
        shape,
        displayed_ticket_type,
    })
}

fn read_view_box(svg: ElementRef<'_>) -> Result<ViewBox> {
    let values = number_list(
        svg.value()
            .attr("viewBox")
            .or_else(|| svg.value().attr("viewbox"))
            .context("seat map lacks viewBox")?,
    )?;
    ensure!(values.len() == 4, "invalid seat map viewBox");
    let bounds = ViewBox {
        x: values[0],
        y: values[1],
        width: values[2],
        height: values[3],
    };
    ensure!(
        bounds.width > 0.0
            && bounds.height > 0.0
            && (bounds.x + bounds.width).abs() <= MAX_COORDINATE
            && (bounds.y + bounds.height).abs() <= MAX_COORDINATE,
        "invalid seat map bounds"
    );
    Ok(bounds)
}

fn point(x: f64, y: f64, bounds: ViewBox) -> Result<Point> {
    ensure!(
        (bounds.x..=bounds.x + bounds.width).contains(&x)
            && (bounds.y..=bounds.y + bounds.height).contains(&y),
        "seat position lies outside viewBox"
    );
    Ok(Point {
        x: if x == 0.0 { 0.0 } else { x },
        y: if y == 0.0 { 0.0 } else { y },
    })
}

fn blocked_ancestor(element: ElementRef<'_>) -> bool {
    element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .any(|e| own_hidden(e, false))
}

fn own_hidden(element: ElementRef<'_>, painted_root: bool) -> bool {
    let e = element.value();
    e.attr("hidden").is_some()
        || e.attr("inert").is_some()
        || e.attr("data-ticketwatch-native-modal") == Some("open")
        || (!painted_root && e.attr("aria-hidden") == Some("true"))
        || e.attr("style").is_some_and(crate::parser::inline_hidden)
}

fn no_transform(element: ElementRef<'_>) -> Result<()> {
    ensure!(
        element
            .value()
            .attr("transform")
            .is_none_or(|s| s.trim().is_empty()),
        "transformed seat layout is unsupported"
    );
    no_style_override(element)
}

fn no_style_override(element: ElementRef<'_>) -> Result<()> {
    let style = element.value().attr("style").unwrap_or("");
    ensure!(
        !style
            .split(';')
            .filter_map(|s| s.split_once(':'))
            .any(|(name, _)| matches!(
                name.trim().to_ascii_lowercase().as_str(),
                "transform"
                    | "transform-origin"
                    | "translate"
                    | "rotate"
                    | "scale"
                    | "offset-path"
                    | "offset-distance"
                    | "offset-rotate"
                    | "offset-anchor"
                    | "x"
                    | "y"
                    | "cx"
                    | "cy"
                    | "r"
                    | "d"
            )),
        "CSS seat geometry overrides are unsupported"
    );
    Ok(())
}

fn identifier(raw: &str) -> Result<String> {
    ensure!(
        !raw.is_empty()
            && raw.len() <= 128
            && raw
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')),
        "invalid seat identifier"
    );
    Ok(raw.to_owned())
}

fn label(raw: &str) -> Result<String> {
    ensure!(
        raw.len() <= 512 && !raw.chars().any(char::is_control),
        "invalid seat label"
    );
    let value = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(
        !value.is_empty() && value.len() <= 128,
        "invalid seat label length"
    );
    Ok(value)
}

fn scalar(raw: &str) -> Result<f64> {
    ensure!(
        raw.len() <= 64 && !raw.trim().is_empty(),
        "invalid seat scalar"
    );
    let value = raw.trim().parse::<f64>().context("invalid seat scalar")?;
    ensure!(
        value.is_finite() && value.abs() <= MAX_COORDINATE,
        "seat scalar exceeds bounds"
    );
    Ok(if value == 0.0 { 0.0 } else { value })
}

fn number_list(raw: &str) -> Result<Vec<f64>> {
    ensure!(
        raw.len() <= 256 && !raw.split(',').any(|s| s.trim().is_empty()),
        "invalid seat number list"
    );
    raw.split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .map(scalar)
        .collect()
}

fn select(value: &str) -> Selector {
    Selector::parse(value).expect("constant seat geometry selector")
}
