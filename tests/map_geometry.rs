use scraper::{Html, Selector};
use ticket_watch::map_geometry::{MapGeometry, PathCommand, extract_svg_geometry, render_svg};

const PAGE: &str = include_str!("fixtures/map-geometry.html");
fn extract(page: &str) -> anyhow::Result<Option<MapGeometry>> {
    let document = Html::parse_document(page);
    let region = document
        .select(&Selector::parse("aside").unwrap())
        .next()
        .unwrap();
    extract_svg_geometry(region)
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
fn extracts_typed_section_paths_and_label_coordinates_without_inventory() {
    let map = extract(PAGE).unwrap().unwrap();
    assert_eq!(map.sections.len(), 2);
    assert_eq!(map.sections[0].name, "A1");
    assert!(matches!(
        map.sections[0].path[2],
        PathCommand::CubicTo { .. }
    ));
    assert_eq!(map.sections[0].label.as_ref().unwrap().lines.len(), 1);
    assert_eq!(
        map.sections[1]
            .label
            .as_ref()
            .unwrap()
            .rotation
            .unwrap()
            .degrees,
        -5.0
    );
    assert_eq!(map.view_box.width, 1000.0);
    assert_eq!(map.coordinate_space, "svg_view_box");
    assert!(!map.stock_verified && !map.adjacency_verified && !map.individual_seats_verified);
}

#[test]
fn zoomed_seat_layer_does_not_replace_or_conflict_with_overview_geometry() {
    let zoomed = PAGE.replace(
        "<g class=\"seats\" hidden></g>",
        "<g class=\"seats\"><g data-component=\"svg__block\"></g></g>",
    );
    assert!(extract(&zoomed).unwrap().is_none());
    let combined = duplicate(PAGE, |_| {
        let start = zoomed.find("<svg ").unwrap();
        let end = zoomed.find("</svg>").unwrap() + "</svg>".len();
        zoomed[start..end].to_owned()
    });
    let map = extract(&combined).unwrap().unwrap();
    assert_eq!(map.rendered_copies, 1);
    assert_eq!(
        map.geometry_fingerprint,
        extract(PAGE).unwrap().unwrap().geometry_fingerprint
    );
    for attribute in [
        "hidden",
        "inert",
        "aria-hidden=\"true\"",
        "style=\"display:none\"",
    ] {
        let preloaded = zoomed.replace(
            "<g class=\"seats\">",
            &format!("<g class=\"seats\" {attribute}>"),
        );
        assert_eq!(
            extract(&preloaded).unwrap().unwrap().geometry_fingerprint,
            extract(PAGE).unwrap().unwrap().geometry_fingerprint
        );
    }
}

#[test]
fn duplicate_views_deduplicate_but_conflicting_paths_or_labels_fail() {
    let baseline = extract(PAGE).unwrap().unwrap();
    let duplicate = duplicate(PAGE, |svg| {
        svg.replace("data-active=\"true\"", "data-active=\"false\"")
            .replace("fill=\"red\"", "fill=\"blue\"")
    });
    let map = extract(&duplicate).unwrap().unwrap();
    assert_eq!(map.rendered_copies, 2);
    assert_eq!(map.sections.len(), 2);
    assert_eq!(map.geometry_fingerprint, baseline.geometry_fingerprint);
    for changed in [
        duplicate
            .replace("M100,100L300,100C350", "M101,100L300,100C350")
            .replacen("M101", "M100", 1),
        duplicate.replacen("data-section-name=\"A1\"", "data-section-name=\"Wrong\"", 1),
    ] {
        assert!(extract(&changed).is_err());
    }
}

#[test]
fn paint_marker_does_not_override_hidden_parent_modal_or_native_hidden() {
    assert!(
        extract(&PAGE.replace(" data-ticketwatch-map-painted=\"true\"", ""))
            .unwrap()
            .is_none()
    );
    for (before, after) in [
        ("<body>", "<body inert>"),
        ("<html>", "<html data-ticketwatch-native-modal=\"open\">"),
        ("<aside aria-label", "<aside hidden aria-label"),
        (
            "<aside aria-label",
            "<aside aria-hidden=\"true\" aria-label",
        ),
        ("<svg data-component", "<svg hidden data-component"),
        (
            "<svg data-component",
            "<svg style=\"display:none\" data-component",
        ),
    ] {
        assert!(
            extract(&PAGE.replace(before, after)).unwrap().is_none(),
            "accepted {after}"
        );
    }
    assert!(
        extract(&PAGE.replacen("<path data-component", "<path hidden data-component", 1)).is_err()
    );
}

#[test]
fn malformed_and_unbounded_paths_are_not_partial_geometry() {
    let original = "M100,100L300,100C350,100,350,300,300,300L100,300Z";
    for invalid in [
        "M100,100 L200 Z",
        "M100,100L200,200L100,200",
        "m100,100L200,200L100,200Z",
        "M100,100Q200,200,100,200Z",
        "M100,100L200,,200L100,200Z",
        "M100,100L200,200,L100,200Z",
        "M100,100L1e309,200L100,200Z",
        "M-1,100L200,200L100,200Z",
        "M100,100L1001,200L100,200Z",
        "M100,100LNaN,200L100,200Z",
        "M100,100L200,200M100,200Z",
        "M100,100L200,200L100,200ZL300,300",
    ] {
        assert!(
            extract(&PAGE.replace(original, invalid)).is_err(),
            "accepted {invalid}"
        );
    }
    let huge = format!("M100,100{}Z", "L200,200".repeat(513));
    assert!(extract(&PAGE.replace(original, &huge)).is_err());
}

#[test]
fn unknown_transforms_nested_shapes_and_ambiguous_identity_fail() {
    for page in [
        PAGE.replace(
            "<g class=\"polygons\">",
            "<g class=\"polygons\" transform=\"scale(2)\">",
        ),
        PAGE.replacen(
            "<path data-component",
            "<path transform=\"translate(5)\" data-component",
            1,
        ),
        PAGE.replace(
            "<g class=\"seats\" hidden></g>",
            "<foreignObject><div>foreign</div></foreignObject>",
        ),
        PAGE.replace("data-section-id=\"s_b\"", "data-section-id=\"s_a\""),
        PAGE.replace("data-label-id=\"s_b\"", "data-label-id=\"missing\""),
        PAGE.replace("rotate(-5,550,180)", "matrix(1,0,0,1,5,5)"),
        PAGE.replacen(
            "<path data-component",
            "<path style=\"transform:translate(3px)\" data-component",
            1,
        ),
        PAGE.replacen(
            "<path data-component",
            "<path style=\"d:path('M0 0Z')\" data-component",
            1,
        ),
        PAGE.replace("font-size=\"20\"><tspan", "font-size=\"20\">Wrong<tspan"),
        PAGE.replace(
            "<g class=\"seats\" hidden></g>",
            "<text data-component=\"svg__label\" data-label-id=\"s_a\">Outside</text>",
        ),
        PAGE.replace("viewBox=\"0 0 1000 800\"", "viewBox=\"0 0 -1000 800\""),
    ] {
        assert!(extract(&page).is_err());
    }
}

#[test]
fn safe_export_rebuilds_shapes_and_escapes_text_without_source_markup() {
    let page = PAGE.replace("A1", "A&lt;script&gt;&amp;\"quote\"");
    // Attribute quotes must be escaped in HTML; the value is then decoded once.
    let page = page.replace(
        "A&lt;script&gt;&amp;\"quote\"",
        "A&lt;script&gt;&amp;&quot;quote&quot;",
    );
    let map = extract(&page).unwrap().unwrap();
    let svg = render_svg(&map).unwrap();
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(svg.contains("A&lt;script&gt;&amp;&quot;quote&quot;"));
    assert!(
        !svg.contains("data-active") && !svg.contains("fill=\"red\"") && !svg.contains("<script>")
    );
    let mut bad = map.clone();
    bad.view_box.width = f64::NAN;
    assert!(render_svg(&bad).is_err());
    let mut bad = map;
    bad.sections[0].path.pop();
    assert!(render_svg(&bad).is_err());
}
