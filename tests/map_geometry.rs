use scraper::{Html, Selector};
use ticket_watch::map_geometry::{
    LabelPolicy, MapGeometry, PathCommand, extract_svg_geometry, extract_svg_geometry_for_site,
    render_svg,
};

const PAGE: &str = include_str!("fixtures/map-geometry.html");
const NL_PAGE: &str = include_str!("fixtures/map-geometry-nl.html");
fn extract(page: &str) -> anyhow::Result<Option<MapGeometry>> {
    let document = Html::parse_document(page);
    let region = document
        .select(&Selector::parse("aside").unwrap())
        .next()
        .unwrap();
    extract_svg_geometry(region)
}
fn extract_for_site(page: &str, host: &str) -> anyhow::Result<Option<MapGeometry>> {
    let document = Html::parse_document(page);
    let region = document
        .select(&Selector::parse("aside").unwrap())
        .next()
        .unwrap();
    extract_svg_geometry_for_site(region, host)
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

#[test]
fn netherlands_aliases_keep_source_names_and_every_bound_label_placement() {
    let map = extract_for_site(NL_PAGE, "ticketmaster.nl")
        .unwrap()
        .unwrap();
    assert_eq!(map.label_policy, LabelPolicy::NetherlandsObservedAliases);
    assert_eq!(map.sections.len(), 5);
    let vak = map.sections.iter().find(|s| s.id == "s_vak").unwrap();
    assert_eq!(vak.name, "Vak127");
    assert_eq!(vak.label.as_ref().unwrap().lines[0].text, "127");
    assert!(vak.additional_labels.is_empty());
    let circle = map.sections.iter().find(|s| s.id == "s_circle").unwrap();
    assert_eq!(circle.name, "INNER CIRCLE - Standing Room Only");
    assert_eq!(circle.additional_labels.len(), 1);
    assert_eq!(circle.label.as_ref().unwrap().lines[0].anchor.y, 350.0);
    assert_eq!(circle.additional_labels[0].lines[0].anchor.y, 500.0);
    let exported = render_svg(&map).unwrap();
    assert_eq!(exported.matches("<path ").count(), 5);
    assert_eq!(exported.matches("<text ").count(), 6);
    assert!(exported.contains("<title>Vak127</title>"));
    assert!(exported.contains("<title>INNER CIRCLE - Standing Room Only</title>"));
    assert!(exported.contains("y=\"350\"") && exported.contains("y=\"500\""));
    assert!(!map.stock_verified && !map.adjacency_verified && !map.individual_seats_verified);
    let moved = NL_PAGE.replace("x=\"400\" y=\"500\"", "x=\"400\" y=\"510\"");
    assert_ne!(
        map.geometry_fingerprint,
        extract_for_site(&moved, "ticketmaster.nl")
            .unwrap()
            .unwrap()
            .geometry_fingerprint
    );
    let two = extract_for_site(&duplicate(NL_PAGE, str::to_owned), "ticketmaster.nl")
        .unwrap()
        .unwrap();
    assert_eq!(two.rendered_copies, 2);
    assert_eq!(two.geometry_fingerprint, map.geometry_fingerprint);
    let conflicted = duplicate(NL_PAGE, |svg| {
        svg.replace("x=\"400\" y=\"500\"", "x=\"400\" y=\"510\"")
    });
    assert!(extract_for_site(&conflicted, "ticketmaster.nl").is_err());
}

#[test]
fn netherlands_label_aliases_require_validated_market_and_exact_observed_spelling() {
    assert!(extract(NL_PAGE).is_err());
    for host in [
        "ticketmaster.cz",
        "ticketmaster.pl",
        "ticketmaster.co.uk",
        "untrusted.example",
    ] {
        assert!(
            extract_for_site(NL_PAGE, host).is_err(),
            "accepted aliases on {host}"
        );
    }
    for changed in [
        NL_PAGE.replace("Vak127", "Vak227"),
        NL_PAGE.replace("Vak127", "Vak0127"),
        NL_PAGE.replace("Vak127", "vak127"),
        NL_PAGE.replace("SKYBOXEN 601", "BOX 601"),
        NL_PAGE.replace("SKYBOXEN 601", "SKYBOXEN 602"),
        NL_PAGE.replace("Standing Room Only", "Standing Tickets"),
        NL_PAGE.replace("STAANPLAATSEN", "STANDING"),
        NL_PAGE.replace("data-label-id=\"s_vak\"", "data-label-id=\"missing\""),
    ] {
        assert!(extract_for_site(&changed, "ticketmaster.nl").is_err());
    }
    let mut map = extract_for_site(NL_PAGE, "ticketmaster.nl")
        .unwrap()
        .unwrap();
    map.label_policy = LabelPolicy::Exact;
    assert!(render_svg(&map).is_err());
}

#[test]
fn additional_netherlands_labels_are_bounded_distinct_and_same_text() {
    for changed in [
        NL_PAGE.replace("x=\"400\" y=\"500\"", "x=\"400\" y=\"350\""),
        NL_PAGE.replacen(
            ">INNER CIRCLE</tspan>",
            ">INNER CIRCLE - Standing Room Only</tspan>",
            1,
        ),
        NL_PAGE.replacen(">INNER CIRCLE</tspan>", ">OUTER CIRCLE</tspan>", 1),
    ] {
        assert!(extract_for_site(&changed, "ticketmaster.nl").is_err());
    }
    let repeated = (1..=7).map(|n| format!(
        "<text data-component=\"svg__label\" data-label-id=\"s_circle\" font-size=\"20\"><tspan x=\"400\" y=\"{}\" dy=\"1em\">INNER CIRCLE</tspan></text>",
        500 + n * 10)).collect::<String>();
    let too_many = NL_PAGE.replace("</g>\n</svg>", &format!("{repeated}</g>\n</svg>"));
    assert!(extract_for_site(&too_many, "ticketmaster.nl").is_err());
    let at_limit = too_many.replace(
        "<text data-component=\"svg__label\" data-label-id=\"s_circle\" font-size=\"20\"><tspan x=\"400\" y=\"570\" dy=\"1em\">INNER CIRCLE</tspan></text>",
        "",
    );
    let bounded = extract_for_site(&at_limit, "ticketmaster.nl")
        .unwrap()
        .unwrap();
    assert_eq!(
        bounded
            .sections
            .iter()
            .find(|s| s.id == "s_circle")
            .unwrap()
            .additional_labels
            .len(),
        7
    );
    assert!(render_svg(&bounded).is_ok());
    let exact_names = NL_PAGE
        .replace("Vak127", "127")
        .replace("SKYBOXEN 601", "601")
        .replace(" - Standing Room Only", "")
        .replace(" - STAANPLAATSEN", "");
    assert!(
        extract(&exact_names).is_err(),
        "Exact mode still refuses repeated labels even when every name matches"
    );
    assert!(extract_for_site(&exact_names, "ticketmaster.nl").is_ok());
    let map = extract_for_site(NL_PAGE, "ticketmaster.nl")
        .unwrap()
        .unwrap();
    let mut changed = map.clone();
    let circle = changed
        .sections
        .iter_mut()
        .find(|s| s.id == "s_circle")
        .unwrap();
    circle
        .additional_labels
        .push(circle.additional_labels[0].clone());
    assert!(render_svg(&changed).is_err());
    let mut changed = map;
    changed
        .sections
        .iter_mut()
        .find(|s| s.id == "s_circle")
        .unwrap()
        .label = None;
    assert!(render_svg(&changed).is_err());
}

#[test]
fn seat_map_passes_only_the_bound_netherlands_market_to_geometry() {
    let base = include_str!("fixtures/seat-map-regional.html");
    let marker = "<script id=\"__NEXT_DATA__\" type=\"application/json\">";
    let start = base.find(marker).unwrap() + marker.len();
    let end = start + base[start..].find("</script>").unwrap();
    let mut data: serde_json::Value = serde_json::from_str(&base[start..end]).unwrap();
    let state = &mut data["props"]["pageProps"]["initialReduxState"];
    state["config"]["countryCode"] = serde_json::json!("NL");
    state["eventInfo"]["marketCode"] = serde_json::json!("NL");
    state["eventInfo"]["venue"]["country"] = serde_json::json!("NL");
    let info = state["eventInfo"].clone();
    for query in state["api"]["queries"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        if query["endpointName"] == "eventInfo" {
            query["data"] = info.clone();
        }
    }
    let page = format!("{}{}{}", &base[..start], data, &base[end..])
        .replace("ticketmaster.cz", "ticketmaster.nl");
    let svg_start = NL_PAGE.find("<svg ").unwrap();
    let svg_end = NL_PAGE.find("</svg>").unwrap() + "</svg>".len();
    let page = page.replace(
        "</aside>",
        &format!("{}</aside>", &NL_PAGE[svg_start..svg_end]),
    );
    let url = "https://www.ticketmaster.nl/event/synthetic-concert/1234567890";
    let map = ticket_watch::seat_map::extract_seat_map(url, url, &page).unwrap();
    assert_eq!(
        map.rendered_geometry.unwrap().label_policy,
        LabelPolicy::NetherlandsObservedAliases
    );
    assert!(!map.stock_verified && !map.adjacency_verified && !map.geometry_verified);
    let conflicted = page.replace("\"marketCode\":\"NL\"", "\"marketCode\":\"CZ\"");
    assert!(ticket_watch::seat_map::extract_seat_map(url, url, &conflicted).is_err());
}
