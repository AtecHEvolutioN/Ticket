use ticket_watch::{
    map_inspection::SeatTooltip,
    map_scan::{
        BrowserMapScan, BrowserScanResult, ScanOutcome, StopReason, validate_displayed_quote,
        validate_report, validate_scan,
    },
    seat_map::{SeatMapSnapshot, extract_seat_map},
};

const START: i64 = 1_700_000_000_000;
const NOW: i64 = START + 1000;

fn snapshot() -> SeatMapSnapshot {
    fn svg(page: &str) -> &str {
        &page[page.find("<svg ").unwrap()..page.find("</svg>").unwrap() + 6]
    }
    let overview = include_str!("fixtures/map-geometry.html")
        .replace("s_a", "s_12")
        .replace("A1", "110");
    let raw_seats = include_str!("fixtures/seat-geometry.html");
    let resale = raw_seats
        .lines()
        .find(|line| line.contains("id=\"seat-2\""))
        .unwrap();
    let second = resale
        .replace("seat-2", "seat-4")
        .replace("data-seat-name=\"2\"", "data-seat-name=\"4\"")
        .replace("translate(118, 188)", "translate(148, 188)");
    let seats = raw_seats.replace(resale, &format!("{resale}\n{second}"));
    let page = include_str!("fixtures/seat-map-regional.html").replace(
        "</aside>",
        &format!("{}{}</aside>", svg(&overview), svg(&seats)),
    );
    let url = "https://www.ticketmaster.cz/event/synthetic-concert/1234567890";
    extract_seat_map(url, url, &page).unwrap()
}

fn evidence() -> BrowserMapScan {
    let result = |id: &str, seat: &str, price: &str, at| BrowserScanResult {
        seat_id: id.into(),
        row: "12".into(),
        seat: seat.into(),
        observed_at_ms: at,
        tooltip: Some(SeatTooltip {
            zone: "1. POSCHODÍ".into(),
            section: "110".into(),
            row: "12".into(),
            seat: seat.into(),
            ticket_type: "Ověřené přeprodávané vstupenky".into(),
            restriction: Some("Omezený výhled".into()),
            price_text: price.into(),
        }),
        outcome: ScanOutcome::Quoted,
    };
    BrowserMapScan {
        section_id: "s_12".into(),
        section_name: "110".into(),
        limit: 20,
        started_at_ms: START,
        finished_at_ms: START + 300,
        candidate_count: 2,
        visible_candidate_count: 2,
        results: vec![
            result("seat-2", "2", "2\u{a0}438,00\u{a0}Kč za kus", START + 100),
            result("seat-4", "4", "2 499,99 Kč za kus", START + 200),
        ],
        stop_reason: StopReason::VisibleCandidatesExhausted,
        coverage: "partial_rendered_section".into(),
        stock_verified: false,
        adjacency_verified: false,
    }
}

#[test]
fn multiple_displayed_quotes_keep_individual_times_prices_and_partial_coverage() {
    let report = validate_scan(&snapshot(), &evidence(), NOW).unwrap();
    assert_eq!(report.quoted_count, 2);
    assert_eq!(report.attempted_count, 2);
    assert_eq!(report.unattempted_count, 0);
    assert_eq!(report.coverage, "partial_rendered_section");
    assert!(!report.stock_verified && !report.adjacency_verified);
    assert_eq!(
        report.observations[0].quote.as_ref().unwrap().price_minor,
        243800
    );
    assert_eq!(
        report.observations[1].quote.as_ref().unwrap().price_minor,
        249999
    );
    assert_eq!(report.observations[0].observed_at_ms, START + 100);
    assert!(
        report
            .observations
            .iter()
            .all(|o| !o.quote.as_ref().unwrap().fees_complete)
    );
    let copy = serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
    assert_eq!(report, copy);
    validate_report(&copy).unwrap();
}

#[test]
fn unknown_tooltips_offscreen_and_bounded_scans_never_become_stock_counts() {
    let snapshot = snapshot();
    let mut raw = evidence();
    raw.results[0].tooltip = None;
    raw.results[0].outcome = ScanOutcome::TooltipUnavailable;
    let report = validate_scan(&snapshot, &raw, NOW).unwrap();
    assert_eq!(report.quoted_count, 1);
    assert!(report.observations[0].quote.is_none());
    for (visible, limit, stop) in [
        (1, 20, StopReason::VisibleCandidatesExhausted),
        (2, 1, StopReason::SeatLimit),
        (2, 20, StopReason::TimeBudget),
    ] {
        let mut raw = evidence();
        raw.results.pop();
        raw.visible_candidate_count = visible;
        raw.limit = limit;
        raw.stop_reason = stop;
        let report = validate_scan(&snapshot, &raw, NOW).unwrap();
        assert_eq!(report.unattempted_count, 1);
        assert!(!report.stock_verified);
    }
    let mut raw = evidence();
    raw.results.clear();
    raw.visible_candidate_count = 0;
    let report = validate_scan(&snapshot, &raw, NOW).unwrap();
    assert_eq!(report.quoted_count, 0);
    assert_eq!(report.unattempted_count, 2);
    assert!(!report.stock_verified);
    let mut no_candidates = snapshot.clone();
    for section in &mut no_candidates.rendered_seats.as_mut().unwrap().sections {
        for row in &mut section.rows {
            for seat in &mut row.seats {
                seat.displayed_ticket_type = None;
            }
        }
    }
    raw.candidate_count = 0;
    let empty = validate_scan(&no_candidates, &raw, NOW).unwrap();
    assert!(empty.observations.is_empty());
    assert_eq!(empty.candidate_count, 0);
    assert!(!empty.stock_verified && !empty.adjacency_verified);
}

#[test]
fn inconsistent_counts_limits_and_stop_reasons_are_rejected() {
    let snapshot = snapshot();
    for change in 0..10 {
        let mut raw = evidence();
        match change {
            0 => raw.limit = 0,
            1 => raw.limit = 51,
            2 => raw.candidate_count = 2001,
            3 => raw.visible_candidate_count = 3,
            4 => raw.candidate_count = 3,
            5 => raw.limit = 1,
            6 => raw.stop_reason = StopReason::SeatLimit,
            7 => raw.stop_reason = StopReason::TimeBudget,
            8 => {
                raw.results.pop();
            }
            _ => raw.coverage = "complete".into(),
        }
        assert!(
            validate_scan(&snapshot, &raw, NOW).is_err(),
            "case {change}"
        );
    }
}

#[test]
fn seat_labels_types_duplicates_and_outcomes_are_bound_to_the_final_map() {
    let snapshot = snapshot();
    for change in 0..11 {
        let mut raw = evidence();
        match change {
            0 => raw.section_id = "s_other".into(),
            1 => raw.section_name = "111".into(),
            2 => raw.results[0].seat_id = "seat-1".into(),
            3 => raw.results[0].row = "13".into(),
            4 => raw.results[0].seat = "9".into(),
            5 => raw.results[0].tooltip.as_mut().unwrap().row = "13".into(),
            6 => raw.results[1] = raw.results[0].clone(),
            7 => raw.results.swap(0, 1),
            8 => raw.results[0].outcome = ScanOutcome::TooltipUnavailable,
            9 => raw.results[0].tooltip = None,
            _ => raw.stock_verified = true,
        }
        assert!(
            validate_scan(&snapshot, &raw, NOW).is_err(),
            "case {change}"
        );
    }
    let mut raw = evidence();
    raw.results[0].tooltip = None;
    raw.results[0].outcome = ScanOutcome::TooltipUnavailable;
    raw.results[0].row = "13".into();
    assert!(validate_scan(&snapshot, &raw, NOW).is_err());
    let mut wrong = snapshot.clone();
    wrong.site_host = "ticketmaster.pl".into();
    assert!(validate_scan(&wrong, &evidence(), NOW).is_err());
}

#[test]
fn stale_future_inverted_long_and_unordered_timestamps_are_rejected() {
    let snapshot = snapshot();
    for change in 0..7 {
        let mut raw = evidence();
        match change {
            0 => raw.started_at_ms = 0,
            1 => raw.finished_at_ms = START - 1,
            2 => raw.finished_at_ms = START + 30_001,
            3 => raw.results[0].observed_at_ms = START - 1,
            4 => raw.results[1].observed_at_ms = START + 301,
            5 => raw.results[1].observed_at_ms = START + 99,
            _ => raw.started_at_ms = START - 120_001,
        }
        assert!(
            validate_scan(&snapshot, &raw, NOW).is_err(),
            "case {change}"
        );
    }
    assert!(validate_scan(&snapshot, &evidence(), START - 6000).is_err());
    assert!(validate_scan(&snapshot, &evidence(), START + 120_001).is_err());
    assert!(validate_scan(&snapshot, &evidence(), -1).is_err());
}

#[test]
fn normalized_reports_reject_mutated_scope_prices_and_inventory_claims() {
    let original = validate_scan(&snapshot(), &evidence(), NOW).unwrap();
    for change in 0..11 {
        let mut report = original.clone();
        match change {
            0 => report.event_id = "other".into(),
            1 => report.site_host = "ticketmaster.pl".into(),
            2 => report.attempted_count = 0,
            3 => report.unattempted_count = 1,
            4 => report.quoted_count = 1,
            5 => report.observations[0].quote.as_mut().unwrap().price_minor += 1,
            6 => report.observations[0].quote.as_mut().unwrap().currency = "EUR".into(),
            7 => {
                report.observations[0]
                    .quote
                    .as_mut()
                    .unwrap()
                    .available_quantity = Some(1)
            }
            8 => report.observations[0].quote.as_mut().unwrap().fees_complete = true,
            9 => report.observations[0].quote.as_mut().unwrap().seat_id = "seat-other".into(),
            _ => report.observations[0].quote.as_mut().unwrap().price_text = "2438 za kus".into(),
        }
        assert!(validate_report(&report).is_err(), "case {change}");
    }
    let mut quote = original.observations[0].quote.clone().unwrap();
    quote.row = "\n12".into();
    assert!(validate_displayed_quote(&quote).is_err());
    let mut json = serde_json::to_value(&original).unwrap();
    json["untrusted_extra"] = true.into();
    assert!(serde_json::from_value::<ticket_watch::map_scan::SectionScan>(json).is_err());
}
