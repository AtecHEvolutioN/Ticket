use scraper::{Html, Selector};
use ticket_watch::seat_geometry::{RenderedSeats, SeatShape, extract_rendered_seats};

const PAGE: &str = include_str!("fixtures/seat-geometry.html");

fn extract(page: &str) -> anyhow::Result<Option<RenderedSeats>> {
    let doc = Html::parse_document(page);
    let region = doc
        .select(&Selector::parse("aside").unwrap())
        .next()
        .unwrap();
    extract_rendered_seats(region)
}

fn duplicate(page: &str, change: impl FnOnce(&str) -> String) -> String {
    let start = page.find("<svg data-component").unwrap();
    let end = start + page[start..].find("</svg>").unwrap() + "</svg>".len();
    page.replace(
        "</aside>",
        &format!("{}</aside>", change(&page[start..end])),
    )
}

#[test]
fn seat_identities_are_explicit_and_never_become_stock_or_adjacency() {
    let seats = extract(PAGE).unwrap().unwrap();
    assert_eq!(seats.sections.len(), 1);
    assert_eq!(seats.rendered_seat_count, 3);
    let block = &seats.sections[0];
    assert_eq!((&*block.id, &*block.name), ("s_12", "1P-110"));
    assert_eq!(block.rows[0].name, "12");
    let resale = &block.rows[0].seats[1];
    assert_eq!(resale.name, "2");
    assert_eq!(resale.displayed_ticket_type, Some("resale"));
    assert_eq!((resale.position.x, resale.position.y), (130.0, 200.0));
    assert!(
        matches!(resale.shape, SeatShape::ResaleIcon { scale, radius } if scale == 0.24 && radius == 12.0)
    );
    assert_eq!(seats.coverage, "partial_rendered_dom");
    assert!(
        !seats.stock_verified && !seats.adjacency_verified && !seats.viewport_coverage_verified
    );
    assert!(block.rows[0].seats[0].displayed_ticket_type.is_none());
}

#[test]
fn paint_provenance_cannot_override_modal_inert_or_hidden_content() {
    assert!(
        extract(&PAGE.replace(" data-ticketwatch-map-painted=\"true\"", ""))
            .unwrap()
            .is_none()
    );
    for (before, after) in [
        ("<html>", "<html data-ticketwatch-native-modal=\"open\">"),
        ("<body>", "<body inert>"),
        ("<aside aria-label", "<aside hidden aria-label"),
        ("<svg data-component", "<svg hidden data-component"),
        ("<g class=\"seats\">", "<g hidden class=\"seats\">"),
    ] {
        assert!(extract(&PAGE.replace(before, after)).unwrap().is_none());
    }
    for (before, after) in [
        ("data-section-id=\"s_12\"", "data-section-id=\"s_12\" inert"),
        (
            "data-row-name=\"12\"",
            "data-row-name=\"12\" aria-hidden=\"true\"",
        ),
        ("id=\"seat-1\"", "id=\"seat-1\" hidden"),
        ("id=\"seat-1\"", "id=\"seat-1\" style=\"display:none\""),
    ] {
        assert!(extract(&PAGE.replace(before, after)).is_err());
    }
}

#[test]
fn empty_layers_do_not_report_zero_available_seats() {
    let page = "<aside><svg data-component=svg data-ticketwatch-map-painted=true viewBox='0 0 100 100'><g class=seats hidden></g></svg></aside>";
    assert!(extract(page).unwrap().is_none());
    assert!(extract(&page.replace(" hidden", "")).unwrap().is_none());
    assert!(
        extract(&page.replace("<g class=seats hidden></g>", ""))
            .unwrap()
            .is_none()
    );
}

#[test]
fn duplicate_views_deduplicate_but_duplicate_identity_or_conflicting_copies_fail() {
    let baseline = extract(PAGE).unwrap().unwrap();
    let copies = extract(&duplicate(PAGE, |s| {
        s.replace("class=\"grey\"", "class=\"blue\"")
    }))
    .unwrap()
    .unwrap();
    assert_eq!(copies.rendered_copies, 2);
    assert_eq!(copies.rendered_seat_count, 3);
    assert_eq!(copies.geometry_fingerprint, baseline.geometry_fingerprint);
    for page in [
        duplicate(PAGE, |s| s.replace("cx=\"100\"", "cx=\"101\"")),
        PAGE.replace("id=\"seat-2\"", "id=\"seat-1\""),
        PAGE.replace("data-seat-name=\"2\"", "data-seat-name=\"1\""),
        PAGE.replace("data-row-name=\"13\"", "data-row-name=\"12\""),
    ] {
        assert!(extract(&page).is_err());
    }
}

#[test]
fn malformed_positions_transforms_and_unbound_seats_fail_closed() {
    for (before, after) in [
        ("cx=\"100\"", "cx=\"NaN\""),
        ("cx=\"100\"", "cx=\"1001\""),
        ("cy=\"200\"", "cy=\"-1\""),
        ("r=\"10.2\"", "r=\"0\""),
        ("r=\"50\"", "r=\"5e-324\""),
        ("transform=\"\"", "transform=\"translate(1 2)\""),
        ("scale(0.24)", "scale(0.24) rotate(10)"),
        ("translate(118, 188)", "translate(118,,188)"),
        ("scale(0.24)", "scale(-1)"),
        ("type=\"resale\"", "type=\"unknown\""),
        ("class=\"grey\"", "style=\"cx: 10px\""),
        ("class=\"grey\"", "style=\"translate: 10px 20px\""),
        ("class=\"grey\"", "style=\"rotate: 20deg\""),
        ("class=\"grey\"", "style=\"scale: 2\""),
        (
            "class=\"grey\"",
            "style=\"offset-path: path('M0 0L10 10')\"",
        ),
        ("data-row-name=\"12\"", "data-wrong=\"12\""),
        ("data-component=\"svg__seat\"", "data-component=\"other\""),
        ("id=\"seat-1\"", "id=\"https://example.com\""),
    ] {
        assert!(
            extract(&PAGE.replace(before, after)).is_err(),
            "accepted {after}"
        );
    }
    let unbound = PAGE.replace("<g class=\"polygons\"></g>", "<g class=polygons><circle data-component=svg__seat id=stray data-seat-name=9 cx=5 cy=5 r=1 /></g>");
    assert!(extract(&unbound).is_err());
}

#[test]
fn bounds_and_view_order_do_not_turn_partial_geometry_into_a_complete_venue() {
    let baseline = extract(PAGE).unwrap().unwrap();
    let retyped = PAGE.replace("<g data-component=\"svg__seat\" id=\"seat-2\" type=\"resale\" data-seat-name=\"2\" transform=\"translate(118, 188) scale(0.24)\" class=\"pink\"><circle cx=\"50\" cy=\"50\" r=\"50\" /><path d=\"M0 0L2 2Z\" /></g>", "<circle data-component=\"svg__seat\" id=\"seat-2\" data-seat-name=\"2\" cx=\"130\" cy=\"200\" r=\"10.2\" />");
    let retyped = extract(&retyped).unwrap().unwrap();
    assert_eq!(retyped.geometry_fingerprint, baseline.geometry_fingerprint);
    assert_ne!(retyped.sections, baseline.sections);
    assert_eq!(retyped.coverage, "partial_rendered_dom");
    assert!(extract(&PAGE.replace("0 0 1000 800", "0 0 0 800")).is_err());
    let many = (0..5).fold(PAGE.to_owned(), |p, _| duplicate(&p, str::to_owned));
    assert!(extract(&many).is_err());
}
