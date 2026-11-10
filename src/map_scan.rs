//! Bounded historical hover observations from one rendered Czech map section.
//!
//! A scan is not an atomic inventory snapshot. Counts describe display candidates
//! and attempts, never remaining tickets. Missing tooltips remain unknown.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    map_inspection::{DisplayedSeatQuote, MapInspection, SeatTooltip, validate_inspection},
    seat_map::SeatMapSnapshot,
};

pub const MAX_SCAN_LIMIT: u32 = 50;
pub const MAX_SCAN_CANDIDATES: u32 = 2000;
pub const MAX_SCAN_DURATION_MS: i64 = 30_000;
pub const MAX_SCAN_AGE_MS: i64 = 120_000;
const CLOCK_SKEW_MS: i64 = 5000;
const COVERAGE: &str = "partial_rendered_section";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanOutcome {
    Quoted,
    TooltipUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    VisibleCandidatesExhausted,
    SeatLimit,
    TimeBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserScanResult {
    pub seat_id: String,
    pub row: String,
    pub seat: String,
    pub observed_at_ms: i64,
    pub tooltip: Option<SeatTooltip>,
    pub outcome: ScanOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserMapScan {
    pub section_id: String,
    pub section_name: String,
    pub limit: u32,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub candidate_count: u32,
    pub visible_candidate_count: u32,
    pub results: Vec<BrowserScanResult>,
    pub stop_reason: StopReason,
    pub coverage: String,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeatScanObservation {
    pub seat_id: String,
    pub row: String,
    pub seat: String,
    pub observed_at_ms: i64,
    pub quote: Option<DisplayedSeatQuote>,
    pub outcome: ScanOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionScan {
    pub event_url: String,
    pub event_id: String,
    pub site_host: String,
    pub section_id: String,
    pub section_name: String,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub coverage: String,
    pub limit: u32,
    pub candidate_count: u32,
    pub visible_candidate_count: u32,
    pub attempted_count: u32,
    pub unattempted_count: u32,
    pub quoted_count: u32,
    pub stop_reason: StopReason,
    pub observations: Vec<SeatScanObservation>,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
}

/// Validate fresh worker observations against the final event-bound rendered map.
pub fn validate_scan(
    snapshot: &SeatMapSnapshot,
    evidence: &BrowserMapScan,
    now_ms: i64,
) -> Result<SectionScan> {
    ensure!(
        now_ms > 0
            && evidence.started_at_ms >= now_ms.saturating_sub(MAX_SCAN_AGE_MS)
            && evidence.finished_at_ms <= now_ms.saturating_add(CLOCK_SKEW_MS),
        "scan timestamp is stale or in the future"
    );
    validate_scope(&evidence.section_id, &evidence.section_name)?;
    validate_window(evidence.started_at_ms, evidence.finished_at_ms)?;
    validate_counts(
        evidence.limit,
        evidence.candidate_count,
        evidence.visible_candidate_count,
        evidence.results.len(),
        evidence.stop_reason,
    )?;
    ensure!(
        evidence.coverage == COVERAGE && !evidence.stock_verified && !evidence.adjacency_verified,
        "unsupported scan inventory or coverage claims"
    );
    let inspection = MapInspection {
        section_id: evidence.section_id.clone(),
        section_name: evidence.section_name.clone(),
        seat_id: None,
        coverage: "section_only".into(),
        tooltip: None,
        stock_verified: false,
        adjacency_verified: false,
    };
    // Also binds empty scans to a numbered overview and an explicit seat block.
    validate_inspection(snapshot, &inspection)?;
    let section = snapshot
        .rendered_seats
        .as_ref()
        .context("scan seat layout missing")?
        .sections
        .iter()
        .find(|section| section.id == evidence.section_id)
        .context("scan section block missing")?;
    let candidates = section
        .rows
        .iter()
        .flat_map(|row| row.seats.iter())
        .filter(|seat| seat.displayed_ticket_type == Some("resale"))
        .count();
    ensure!(
        candidates == evidence.candidate_count as usize,
        "scan candidate count differs from the rendered section"
    );
    let mut observations = Vec::with_capacity(evidence.results.len());
    for result in &evidence.results {
        ensure!(
            (result.outcome == ScanOutcome::Quoted) == result.tooltip.is_some(),
            "scan outcome conflicts with tooltip evidence"
        );
        let (row, seat) = section
            .rows
            .iter()
            .flat_map(|row| row.seats.iter().map(move |seat| (row, seat)))
            .find(|(_, seat)| seat.id == result.seat_id)
            .context("scan seat is missing from the requested section")?;
        ensure!(
            row.name == result.row
                && seat.name == result.seat
                && seat.displayed_ticket_type == Some("resale"),
            "scan seat identity or type changed"
        );
        let validated = validate_inspection(
            snapshot,
            &MapInspection {
                seat_id: Some(result.seat_id.clone()),
                tooltip: result.tooltip.clone(),
                ..inspection.clone()
            },
        )?;
        observations.push(SeatScanObservation {
            seat_id: result.seat_id.clone(),
            row: result.row.clone(),
            seat: result.seat.clone(),
            observed_at_ms: result.observed_at_ms,
            quote: validated.displayed_quote,
            outcome: result.outcome,
        });
    }
    let attempted_count = observations.len() as u32;
    let report = SectionScan {
        event_url: snapshot.event_url.clone(),
        event_id: snapshot.event_id.clone(),
        site_host: snapshot.site_host.clone(),
        section_id: evidence.section_id.clone(),
        section_name: evidence.section_name.clone(),
        started_at_ms: evidence.started_at_ms,
        finished_at_ms: evidence.finished_at_ms,
        coverage: COVERAGE.into(),
        limit: evidence.limit,
        candidate_count: evidence.candidate_count,
        visible_candidate_count: evidence.visible_candidate_count,
        attempted_count,
        unattempted_count: evidence.candidate_count - attempted_count,
        quoted_count: observations.iter().filter(|o| o.quote.is_some()).count() as u32,
        stop_reason: evidence.stop_reason,
        observations,
        stock_verified: false,
        adjacency_verified: false,
    };
    validate_report(&report)?;
    Ok(report)
}

/// Defensive shape validation for a normalized report read back from storage.
/// This verifies internal claims, not transport provenance or current freshness.
pub fn validate_report(report: &SectionScan) -> Result<()> {
    let (host, event) = crate::event_identity::identity(&report.event_url)
        .context("scan report lacks a public event identity")?;
    crate::config::validate_public_url(&report.event_url)?;
    ensure!(
        host == "ticketmaster.cz" && report.site_host == host && report.event_id == event,
        "scan report event identity mismatch"
    );
    validate_scope(&report.section_id, &report.section_name)?;
    validate_window(report.started_at_ms, report.finished_at_ms)?;
    validate_counts(
        report.limit,
        report.candidate_count,
        report.visible_candidate_count,
        report.observations.len(),
        report.stop_reason,
    )?;
    ensure!(
        report.coverage == COVERAGE && !report.stock_verified && !report.adjacency_verified,
        "unsupported scan report claims"
    );
    ensure!(
        report.attempted_count as usize == report.observations.len()
            && report.unattempted_count == report.candidate_count - report.attempted_count
            && report.quoted_count as usize
                == report
                    .observations
                    .iter()
                    .filter(|o| o.quote.is_some())
                    .count(),
        "scan report count mismatch"
    );
    let mut previous_id: Option<&str> = None;
    let mut previous_time = report.started_at_ms;
    let mut labels = BTreeSet::new();
    for observation in &report.observations {
        identifier(&observation.seat_id)?;
        text(&observation.row)?;
        text(&observation.seat)?;
        ensure!(
            previous_id.is_none_or(|id| id < observation.seat_id.as_str())
                && labels.insert((&observation.row, &observation.seat)),
            "duplicate or unordered scan seat identities"
        );
        ensure!(
            observation.observed_at_ms >= previous_time
                && observation.observed_at_ms <= report.finished_at_ms,
            "scan observation timestamp is outside its ordered window"
        );
        ensure!(
            (observation.outcome == ScanOutcome::Quoted) == observation.quote.is_some(),
            "scan observation outcome mismatch"
        );
        if let Some(quote) = &observation.quote {
            validate_quote(quote, &report.section_name, observation)?;
        }
        previous_id = Some(&observation.seat_id);
        previous_time = observation.observed_at_ms;
    }
    Ok(())
}

fn validate_quote(
    quote: &DisplayedSeatQuote,
    section: &str,
    observation: &SeatScanObservation,
) -> Result<()> {
    ensure!(
        quote.seat_id == observation.seat_id
            && quote.section == section
            && quote.row == observation.row
            && quote.seat == observation.seat,
        "scan quote does not match its observation"
    );
    validate_displayed_quote(quote)
}

/// Validate a persisted normalized quote independently of its event/seat binding.
/// The caller must verify those bindings and the observation timestamp separately.
pub fn validate_displayed_quote(quote: &DisplayedSeatQuote) -> Result<()> {
    identifier(&quote.seat_id)?;
    ensure!(
        quote.evidence == "rendered_seat_hover"
            && quote.currency == "CZK"
            && quote.price_basis == "per_ticket"
            && !quote.fees_complete
            && quote.available_quantity.is_none()
            && quote.ticket_type == "Ověřené přeprodávané vstupenky",
        "scan quote scope mismatch"
    );
    ensure!(
        !quote.section.is_empty()
            && quote.section.len() <= 12
            && quote.section.bytes().all(|b| b.is_ascii_digit()),
        "invalid quote section"
    );
    for value in [
        &quote.zone,
        &quote.row,
        &quote.seat,
        &quote.ticket_type,
        &quote.price_text,
    ] {
        text(value)?;
    }
    if let Some(restriction) = &quote.restriction {
        text(restriction)?;
    }
    let normalized = quote
        .price_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let amount = normalized
        .strip_suffix(" Kč za kus")
        .context("scan quote lacks explicit price unit and currency")?;
    ensure!(
        (1..=100_000_000).contains(&quote.price_minor)
            && crate::parser::parse_price_minor(amount, "CZK", ",") == Some(quote.price_minor),
        "scan quote normalized price mismatch"
    );
    Ok(())
}

fn validate_scope(id: &str, name: &str) -> Result<()> {
    identifier(id)?;
    ensure!(
        !name.is_empty() && name.len() <= 12 && name.bytes().all(|b| b.is_ascii_digit()),
        "scan requires a numbered section"
    );
    Ok(())
}

fn identifier(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
        "invalid scan identifier"
    );
    Ok(())
}

fn text(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 512
            && value.trim() == value
            && value.chars().all(|c| !c.is_control()),
        "invalid scan text"
    );
    Ok(())
}

fn validate_window(start: i64, finish: i64) -> Result<()> {
    ensure!(
        start > 0 && finish >= start && finish.saturating_sub(start) <= MAX_SCAN_DURATION_MS,
        "invalid scan time window"
    );
    Ok(())
}

fn validate_counts(
    limit: u32,
    candidates: u32,
    visible: u32,
    results: usize,
    stop: StopReason,
) -> Result<()> {
    ensure!(
        (1..=MAX_SCAN_LIMIT).contains(&limit)
            && candidates <= MAX_SCAN_CANDIDATES
            && visible <= candidates
            && results <= limit.min(visible) as usize,
        "invalid scan candidate or attempt counts"
    );
    match stop {
        StopReason::VisibleCandidatesExhausted => ensure!(
            results == visible as usize,
            "unexplained unattempted visible candidates"
        ),
        StopReason::SeatLimit => ensure!(
            results == limit as usize && limit < visible,
            "scan did not reach its seat limit"
        ),
        StopReason::TimeBudget => ensure!(
            results < limit.min(visible) as usize,
            "scan time-budget reason conflicts with attempt counts"
        ),
    }
    Ok(())
}
