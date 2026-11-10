//! Event-bound section/seat hover observations, separate from confirmed stock.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MapInspection {
    pub section_id: String,
    pub section_name: String,
    pub seat_id: Option<String>,
    pub coverage: String,
    pub tooltip: Option<SeatTooltip>,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatTooltip {
    pub zone: String,
    pub section: String,
    pub row: String,
    pub seat: String,
    pub ticket_type: String,
    pub restriction: Option<String>,
    pub price_text: String,
}

#[derive(Debug, Serialize)]
pub struct SectionInspection {
    pub section_id: String,
    pub section_name: String,
    pub coverage: &'static str,
    pub rendered_seat_count: usize,
    pub hovered_seat_id: Option<String>,
    pub displayed_quote: Option<DisplayedSeatQuote>,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DisplayedSeatQuote {
    pub evidence: String,
    pub seat_id: String,
    pub zone: String,
    pub section: String,
    pub row: String,
    pub seat: String,
    pub ticket_type: String,
    pub restriction: Option<String>,
    pub price_minor: i64,
    pub currency: String,
    pub price_basis: String,
    pub price_text: String,
    pub fees_complete: bool,
    pub available_quantity: Option<u32>,
}

pub fn validate_inspection(
    snapshot: &crate::seat_map::SeatMapSnapshot,
    evidence: &MapInspection,
) -> Result<SectionInspection> {
    ensure!(
        snapshot.site_host == "ticketmaster.cz",
        "section inspection supports Czech pages only"
    );
    ensure!(
        evidence.coverage == "section_only"
            && !evidence.stock_verified
            && !evidence.adjacency_verified,
        "unsupported section inspection claims"
    );
    identifier(&evidence.section_id)?;
    ensure!(
        !evidence.section_name.is_empty()
            && evidence.section_name.len() <= 12
            && evidence.section_name.bytes().all(|b| b.is_ascii_digit()),
        "section inspection requires a numbered seating section"
    );
    let overview = snapshot
        .rendered_geometry
        .as_ref()
        .context("section overview missing")?;
    ensure!(
        overview
            .sections
            .iter()
            .any(|s| s.id == evidence.section_id && s.name == evidence.section_name),
        "section inspection does not match map overview"
    );
    let layout = snapshot
        .rendered_seats
        .as_ref()
        .context("rendered seat layout missing")?;
    let section = layout
        .sections
        .iter()
        .find(|s| s.id == evidence.section_id)
        .context("requested section has no rendered seat block")?;
    let mut hovered = None;
    if let Some(seat_id) = &evidence.seat_id {
        identifier(seat_id)?;
        for row in &section.rows {
            for seat in &row.seats {
                if seat.id == *seat_id {
                    ensure!(hovered.is_none(), "ambiguous hovered seat");
                    hovered = Some((row, seat));
                }
            }
        }
        ensure!(
            hovered.is_some(),
            "hovered seat is not in the requested section"
        );
    }
    let quote = evidence
        .tooltip
        .as_ref()
        .map(|tip| -> Result<_> {
            let (row, seat) = hovered.context("tooltip has no bound hovered seat")?;
            for value in [
                &tip.zone,
                &tip.section,
                &tip.row,
                &tip.seat,
                &tip.ticket_type,
                &tip.price_text,
            ] {
                bounded_text(value)?;
            }
            if let Some(restriction) = &tip.restriction {
                bounded_text(restriction)?;
            }
            ensure!(
                tip.section == evidence.section_name
                    && tip.row == row.name
                    && tip.seat == seat.name,
                "tooltip does not match hovered seat labels"
            );
            ensure!(
                seat.displayed_ticket_type == Some("resale")
                    && tip.ticket_type == "Ověřené přeprodávané vstupenky",
                "unsupported or conflicting seat ticket type"
            );
            ensure!(
                snapshot
                    .configured_sections
                    .as_ref()
                    .is_some_and(|zones| zones.iter().any(|z| z.title == tip.zone)),
                "tooltip zone is not in the event configuration"
            );
            let normalized = tip
                .price_text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let amount = normalized
                .strip_suffix(" za kus")
                .context("seat price is not explicitly per ticket")?;
            ensure!(
                amount.ends_with(" Kč"),
                "seat price lacks an explicit Czech currency marker"
            );
            let price_minor = crate::parser::parse_price_minor(amount, "CZK", ",")
                .filter(|n| (1..=100_000_000).contains(n))
                .context("invalid or ambiguous Czech seat price")?;
            Ok(DisplayedSeatQuote {
                evidence: "rendered_seat_hover".into(),
                seat_id: seat.id.clone(),
                zone: tip.zone.clone(),
                section: tip.section.clone(),
                row: tip.row.clone(),
                seat: tip.seat.clone(),
                ticket_type: tip.ticket_type.clone(),
                restriction: tip.restriction.clone(),
                price_minor,
                currency: "CZK".into(),
                price_basis: "per_ticket".into(),
                price_text: tip.price_text.clone(),
                fees_complete: false,
                available_quantity: None,
            })
        })
        .transpose()?;
    Ok(SectionInspection {
        section_id: evidence.section_id.clone(),
        section_name: evidence.section_name.clone(),
        coverage: "section_only",
        rendered_seat_count: section.rows.iter().map(|r| r.seats.len()).sum(),
        hovered_seat_id: evidence.seat_id.clone(),
        displayed_quote: quote,
        stock_verified: false,
        adjacency_verified: false,
    })
}

fn identifier(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "invalid map inspection identifier"
    );
    Ok(())
}

fn bounded_text(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 512
            && value.trim() == value
            && value.chars().all(|c| !c.is_control()),
        "invalid map inspection text"
    );
    Ok(())
}
