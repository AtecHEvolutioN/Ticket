use ticket_watch::{
    map_inspection::{MapInspection, SeatTooltip, validate_inspection},
    seat_map::{SeatMapSnapshot, extract_seat_map},
};

fn snapshot() -> SeatMapSnapshot {
    fn svg(page: &str) -> &str {
        &page[page.find("<svg ").unwrap()..page.find("</svg>").unwrap() + 6]
    }
    let overview = include_str!("fixtures/map-geometry.html")
        .replace("s_a", "s_12")
        .replace("A1", "110");
    let seats = include_str!("fixtures/seat-geometry.html");
    let page = include_str!("fixtures/seat-map-regional.html").replace(
        "</aside>",
        &format!("{}{}</aside>", svg(&overview), svg(seats)),
    );
    let url = "https://www.ticketmaster.cz/event/synthetic-concert/1234567890";
    extract_seat_map(url, url, &page).unwrap()
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
