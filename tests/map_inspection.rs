use ticket_watch::{
    map_inspection::{MapInspection, SeatTooltip, validate_inspection},
    seat_map::{SeatMapSnapshot, extract_seat_map},
};

fn snapshot() -> SeatMapSnapshot {
    regional_snapshot(false)
}

fn regional_snapshot(dutch: bool) -> SeatMapSnapshot {
    fn svg(page: &str) -> &str {
        &page[page.find("<svg ").unwrap()..page.find("</svg>").unwrap() + 6]
    }
    let overview = include_str!("fixtures/map-geometry.html")
        .replace("s_a", "s_12")
        .replace("A1", "110");
    let seats = include_str!("fixtures/seat-geometry.html");
    let mut page = include_str!("fixtures/seat-map-regional.html").replace(
        "</aside>",
        &format!("{}{}</aside>", svg(&overview), svg(seats)),
    );
    let mut url = "https://www.ticketmaster.cz/event/synthetic-concert/1234567890";
    if dutch {
        page = page
            .replace("ticketmaster.cz", "ticketmaster.nl")
            .replace("\"CZ\"", "\"NL\"")
            .replace("Mapa míst", "Seat Map")
            .replace("1. POSCHODÍ", "1e Ring")
            .replace("id=\"seat-1\"", "id=\"seat-1\" type=\"primary\"")
            .replace("data-section-name=\"110\"", "data-section-name=\"Vak110\"");
        url = "https://www.ticketmaster.nl/event/synthetic-concert/1234567890";
    }
    extract_seat_map(url, url, &page).unwrap()
}

#[test]
fn dutch_displayed_resale_quotes_require_bound_euro_price_and_numbered_section() {
    // Synthetic contract regression, not a claim of a successful live Dutch hover.
    let snapshot = regional_snapshot(true);
    let mut value = evidence();
    value.section_name = "Vak110".into();
    let tip = value.tooltip.as_mut().unwrap();
    tip.zone = "1e Ring".into();
    tip.ticket_type = "Verified Resale Tickets".into();
    for price in ["€ 85.00 each", "€85,00 per stuk"] {
        value.tooltip.as_mut().unwrap().price_text = price.into();
        let result = validate_inspection(&snapshot, &value).unwrap();
        let quote = result.displayed_quote.unwrap();
        assert_eq!(quote.price_minor, 8500);
        assert_eq!(quote.currency, "EUR");
        assert_eq!(result.displayed_inventory_kind, Some("resale"));
        assert_eq!(result.adapter_validation, "limited_live_validation");
        assert_eq!(quote.section, "110");
        assert!(!quote.fees_complete);
        assert_eq!(quote.available_quantity, None);
        assert!(!result.stock_verified && !result.adjacency_verified);
    }
    for invalid in [
        "€85,00 each",
        "€85.00 per stuk",
        "85.00 each",
        "€85.00 total",
        "€85.00 + fees each",
    ] {
        value.tooltip.as_mut().unwrap().price_text = invalid.into();
        assert!(validate_inspection(&snapshot, &value).is_err(), "{invalid}");
    }
    value.tooltip.as_mut().unwrap().price_text = "€85.00 each".into();
    value.tooltip.as_mut().unwrap().section = "111".into();
    assert!(validate_inspection(&snapshot, &value).is_err());
}

#[test]
fn dutch_primary_quote_requires_explicit_circle_type_not_a_standard_label_or_color() {
    let snapshot = regional_snapshot(true);
    let mut value = evidence();
    value.section_name = "Vak110".into();
    value.seat_id = Some("seat-1".into());
    let tip = value.tooltip.as_mut().unwrap();
    tip.zone = "1e Ring".into();
    tip.section = "VAK 110".into();
    tip.seat = "1".into();
    tip.ticket_type = "Presale | Zitplaats".into();
    tip.restriction = None;
    tip.price_text = "€ 89,95 per stuk".into();
    let result = validate_inspection(&snapshot, &value).unwrap();
    assert_eq!(result.displayed_inventory_kind, Some("primary"));
    let quote = result.displayed_quote.unwrap();
    assert_eq!(quote.price_minor, 8995);
    assert_eq!(quote.currency, "EUR");
    assert!(!quote.fees_complete);
    assert_eq!(quote.available_quantity, None);
    let mut unknown = snapshot.clone();
    for section in &mut unknown.rendered_seats.as_mut().unwrap().sections {
        for row in &mut section.rows {
            for seat in &mut row.seats {
                if seat.id == "seat-1" {
                    seat.displayed_ticket_type = None;
                }
            }
        }
    }
    assert!(validate_inspection(&unknown, &value).is_err());
    value.tooltip.as_mut().unwrap().ticket_type = "Verified Resale Tickets".into();
    assert!(validate_inspection(&snapshot, &value).is_err());
}

#[test]
fn repeated_dutch_native_ids_preserve_distinct_named_blocks_but_cannot_bind_an_ambiguous_hover() {
    let mut snapshot = regional_snapshot(true);
    let seats = snapshot.rendered_seats.as_mut().unwrap();
    let mut repeated = seats.sections[0].clone();
    repeated.name = "another-explicit-block".into();
    seats.sections.push(repeated);
    let mut value = evidence();
    value.section_name = "Vak110".into();
    value.tooltip = None;
    value.seat_id = None;
    assert!(validate_inspection(&snapshot, &value).is_err());
}

fn evidence() -> MapInspection {
    MapInspection {
        section_id: "s_12".into(),
        section_name: "110".into(),
        seat_id: Some("seat-2".into()),
        coverage: "section_only".into(),
        stock_verified: false,
        adjacency_verified: false,
        tooltip: Some(SeatTooltip {
            zone: "1. POSCHODÍ".into(),
            section: "110".into(),
            row: "12".into(),
            seat: "2".into(),
            ticket_type: "Ověřené přeprodávané vstupenky".into(),
            restriction: Some("Omezený výhled".into()),
            price_text: "2\u{a0}438,00\u{a0}Kč za kus".into(),
        }),
    }
}

#[test]
fn displayed_seat_quote_is_bound_to_explicit_labels_and_keeps_stock_unknown() {
    let result = validate_inspection(&snapshot(), &evidence()).unwrap();
    let quote = result.displayed_quote.unwrap();
    assert_eq!(quote.price_minor, 243800);
    assert_eq!(quote.currency, "CZK");
    assert_eq!(quote.price_basis, "per_ticket");
    assert!(!quote.fees_complete);
    assert_eq!(quote.available_quantity, None);
    assert!(!result.stock_verified && !result.adjacency_verified);
    assert_eq!(result.rendered_seat_count, 3);
}

#[test]
fn an_unknown_configured_zone_name_cannot_validate_a_hover_quote() {
    let mut map = snapshot();
    for zone in map.configured_sections.as_mut().unwrap() {
        zone.title = None;
    }
    assert!(validate_inspection(&map, &evidence()).is_err());
}

#[test]
fn mismatched_section_seat_and_tooltip_never_create_a_quote() {
    let snapshot = snapshot();
    for field in ["section", "row", "seat", "zone", "ticket_type"] {
        let mut value = evidence();
        let tip = value.tooltip.as_mut().unwrap();
        match field {
            "section" => tip.section = "111".into(),
            "row" => tip.row = "13".into(),
            "seat" => tip.seat = "1".into(),
            "zone" => tip.zone = "Unknown zone".into(),
            _ => tip.ticket_type = "Standard".into(),
        }
        assert!(validate_inspection(&snapshot, &value).is_err(), "{field}");
    }
    for seat in [None, Some("absent"), Some("seat-1")] {
        let mut value = evidence();
        value.seat_id = seat.map(str::to_owned);
        assert!(validate_inspection(&snapshot, &value).is_err());
    }
    let mut value = evidence();
    value.section_name = "111".into();
    assert!(validate_inspection(&snapshot, &value).is_err());
    value = evidence();
    value.stock_verified = true;
    assert!(validate_inspection(&snapshot, &value).is_err());
}

#[test]
fn partial_or_ambiguous_prices_and_unobserved_fee_totals_are_rejected() {
    let snapshot = snapshot();
    for price in [
        "2 438,00 Kč",
        "2 438,00 za kus",
        "2 438,00 EUR za kus",
        "od 2 438,00 Kč za kus",
        "2 438,00 - 3 000,00 Kč za kus",
        "2 438,00 Kč + 100 Kč za kus",
        "0 Kč za kus",
        "2438\nKč za kus",
    ] {
        let mut value = evidence();
        value.tooltip.as_mut().unwrap().price_text = price.into();
        assert!(validate_inspection(&snapshot, &value).is_err(), "{price}");
    }
    let mut value = evidence();
    value.tooltip = None;
    value.seat_id = None;
    let result = validate_inspection(&snapshot, &value).unwrap();
    assert!(result.displayed_quote.is_none() && result.hovered_seat_id.is_none());
    assert!(!result.stock_verified);
}
