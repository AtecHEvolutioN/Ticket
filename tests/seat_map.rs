use serde_json::{Value, json};
use ticket_watch::seat_map::extract_seat_map;

const URL: &str = "https://www.ticketmaster.cz/event/synthetic-concert/1234567890";
const PAGE: &str = include_str!("fixtures/seat-map-regional.html");

fn change_metadata(change: impl FnOnce(&mut Value)) -> String {
    let marker = "<script id=\"__NEXT_DATA__\" type=\"application/json\">";
    let start = PAGE.find(marker).unwrap() + marker.len();
    let end = start + PAGE[start..].find("</script>").unwrap();
    let mut next: Value = serde_json::from_str(&PAGE[start..end]).unwrap();
    change(&mut next);
    format!("{}{}{}", &PAGE[..start], next, &PAGE[end..])
}
fn state(value: &mut Value) -> &mut Value {
    &mut value["props"]["pageProps"]["initialReduxState"]
}
fn sync_query(value: &mut Value, endpoint: &str) {
    let state = state(value);
    let data = state[endpoint].clone();
    for query in state["api"]["queries"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        if query["endpointName"] == endpoint {
            query["data"] = data.clone();
        }
    }
}
fn fingerprint(page: &str) -> String {
    extract_seat_map(URL, URL, page)
        .unwrap()
        .static_map
        .unwrap()
        .layout_fingerprint
}

#[test]
fn extracts_reference_identity_and_configuration_without_inventory_claims() {
    let map = extract_seat_map(URL, URL, PAGE).unwrap();
    assert_eq!(map.event_name, "Synthetic concert");
    assert_eq!(map.venue.regional_id, "42");
    assert_eq!(
        map.venue.discovery_id.as_deref(),
        Some("Z598xSyntheticVenue")
    );
    assert_eq!(
        map.static_map.as_ref().unwrap().url,
        "https://media.ticketmaster.eu/czechrepublic/map-a.png"
    );
    assert!(
        map.static_map
            .as_ref()
            .unwrap()
            .layout_fingerprint
            .starts_with("map-")
    );
    assert_eq!(map.configured_sections.as_ref().unwrap().len(), 2);
    assert_eq!(
        map.price_section_mappings.as_ref().unwrap()[0].section_id,
        "1P"
    );
    assert_eq!(map.evidence, "static_reference_configuration");
    assert!(
        !map.geometry_verified
            && !map.stock_verified
            && !map.adjacency_verified
            && !map.image_content_verified
    );
    assert_eq!(map.visible_canvas_count, 0);
    assert_eq!(map.visible_iframe_count, 0);
}

#[test]
fn two_events_at_one_venue_keep_distinct_map_candidates_and_stable_ordering() {
    let original = fingerprint(PAGE);
    let different = change_metadata(|value| {
        state(value)["eventInfo"]["venue"]["staticMap"] =
            json!("https://media.ticketmaster.eu/czechrepublic/map-b.png");
        sync_query(value, "eventInfo");
    });
    assert_ne!(original, fingerprint(&different));
    let reordered = change_metadata(|value| {
        state(value)["ticketSelection"]["sections"]
            .as_array_mut()
            .unwrap()
            .reverse();
        state(value)["ticketSelection"]["priceToSections"]
            .as_array_mut()
            .unwrap()
            .reverse();
        sync_query(value, "ticketSelection");
    });
    assert_eq!(original, fingerprint(&reordered));
    let other_event = PAGE.replace("1234567890", "9876543210");
    let other_url = URL.replace("1234567890", "9876543210");
    let map = extract_seat_map(&other_url, &other_url, &other_event).unwrap();
    assert_eq!(original, map.static_map.unwrap().layout_fingerprint);
    let another_section = change_metadata(|value| {
        state(value)["ticketSelection"]["sections"][0]["title"] = json!("Different configuration");
        sync_query(value, "ticketSelection");
    });
    assert_ne!(original, fingerprint(&another_section));
}

#[test]
fn no_asset_or_unloaded_sections_remain_unknown_not_empty_inventory() {
    let page = change_metadata(|value| {
        state(value)["eventInfo"]["venue"]["staticMap"] = Value::Null;
        sync_query(value, "eventInfo");
    });
    let map = extract_seat_map(URL, URL, &page).unwrap();
    assert!(map.static_map.is_none());
    assert!(map.configured_sections.is_some());
    assert!(!map.stock_verified);
    let page = change_metadata(|value| {
        state(value)["ticketSelection"] = json!({"stage":"init"});
    });
    let map = extract_seat_map(URL, URL, &page).unwrap();
    assert!(map.configured_sections.is_none());
    assert!(map.price_section_mappings.is_none());
    assert!(map.static_map.is_some());
}

#[test]
fn accepts_observed_protocol_relative_uk_and_australian_images() {
    for (host, market, country, id, url, discovery) in [
        (
            "ticketmaster.co.uk",
            "UK",
            "GB",
            "3500655683001AA0",
            "//media.ticketmaster.co.uk/tm/en-gb/tmimages/venue/maps/uk1/34556s.gif",
            "KovZ9177OxV",
        ),
        (
            "ticketmaster.com.au",
            "AU",
            "AU",
            "130065628C7B2F46",
            "//s1.ticketm.net/tm/en-au/tmimages/venue/maps/aus/98132s.gif",
            "KovZpZAdFI6A",
        ),
    ] {
        let page = change_metadata(|value| {
            state(value)["config"]["countryCode"] = json!(market);
            let event = &mut state(value)["eventInfo"];
            event["marketCode"] = json!(market);
            event["venue"]["country"] = json!(country);
            event["venue"]["staticMap"] = json!(url);
            event["venue"]["discoId"] = json!(discovery);
            sync_query(value, "eventInfo");
            state(value)["ticketSelection"]
                .as_object_mut()
                .unwrap()
                .remove("priceLevels");
            sync_query(value, "ticketSelection");
        })
        .replace("ticketmaster.cz", host)
        .replace("1234567890", id)
        .replace("Mapa míst", "Seat Map");
        let target = URL
            .replace("ticketmaster.cz", host)
            .replace("1234567890", id);
        let map = extract_seat_map(&target, &target, &page).unwrap();
        assert_eq!(map.static_map.unwrap().url, format!("https:{url}"));
        assert_eq!(map.venue.country, country);
        assert_eq!(map.venue.discovery_id.as_deref(), Some(discovery));
    }
}

#[test]
fn rejects_asset_urls_that_could_leak_sessions_or_escape_public_image_scope() {
    for url in [
        "http://media.ticketmaster.eu/czechrepublic/map.png",
        "https://media.ticketmaster.eu.evil.example/czechrepublic/map.png",
        "https://user:secret@media.ticketmaster.eu/czechrepublic/map.png",
        "https://media.ticketmaster.eu:443/czechrepublic/map.png",
        "https://media.ticketmaster.eu/czechrepublic/map.png?token=private",
        "https://media.ticketmaster.eu/czechrepublic/map.png#session",
        "https://media.ticketmaster.eu/czechrepublic/../map.png",
        "https://media.ticketmaster.eu/czechrepublic/%2e%2e/map.png",
        "https://media.ticketmaster.eu//czechrepublic/map.png",
        "https://media.ticketmaster.eu/czechrepublic/map.svg",
        "https://media.ticketmaster.co.uk/account/map.png",
        "https://127.0.0.1/map.png",
        "data:image/png;base64,AAAA",
    ] {
        let page = change_metadata(|value| {
            state(value)["eventInfo"]["venue"]["staticMap"] = json!(url);
            sync_query(value, "eventInfo");
        });
        assert!(extract_seat_map(URL, URL, &page).is_err(), "accepted {url}");
    }
}

#[test]
fn rejects_changed_event_venue_query_and_market_bindings() {
    assert!(extract_seat_map(URL, "https://www.ticketmaster.cz/event/9876543210", PAGE).is_err());
    for pointer in [
        "/query/eventId",
        "/props/pageProps/initialReduxState/eventId",
        "/props/pageProps/initialReduxState/eventInfo/eventId",
        "/props/pageProps/initialReduxState/eventInfo/params",
    ] {
        let page = change_metadata(|value| {
            *value.pointer_mut(pointer).unwrap() = json!("9876543210");
        });
        assert!(extract_seat_map(URL, URL, &page).is_err());
    }
    for (field, value) in [
        ("name", json!("Another venue")),
        (
            "staticMap",
            json!("https://media.ticketmaster.eu/czechrepublic/map-b.png"),
        ),
        ("id", json!("99")),
    ] {
        let page = change_metadata(|next| {
            state(next)["eventInfo"]["venue"][field] = value;
        });
        assert!(extract_seat_map(URL, URL, &page).is_err());
    }
    let page = change_metadata(|value| {
        state(value)["config"]["countryCode"] = json!("PL");
    });
    assert!(extract_seat_map(URL, URL, &page).is_err());
    let page = PAGE.replace("<h1>Synthetic concert</h1>", "<h1>Another artist</h1>");
    assert!(extract_seat_map(URL, URL, &page).is_err());
    let page = change_metadata(|value| {
        let query = &mut state(value)["api"]["queries"]["eventInfo({\"id\":\"1234567890\"})"];
        query["originalArgs"]["id"] = json!("9876543210");
    });
    assert!(extract_seat_map(URL, URL, &page).is_err());
}

#[test]
fn rejects_stale_ambiguous_and_unknown_section_configuration() {
    for value in [json!("loading"), json!("error")] {
        let page = change_metadata(|next| {
            state(next)["eventInfo"]["stage"] = value;
        });
        assert!(extract_seat_map(URL, URL, &page).is_err());
    }
    for node in ["eventInfo", "ticketSelection"] {
        let page = change_metadata(|value| {
            state(value)[node]
                .as_object_mut()
                .unwrap()
                .remove("inFlightParams");
        });
        assert!(extract_seat_map(URL, URL, &page).is_err());
    }
    let page = change_metadata(|value| {
        state(value)["ticketSelection"]["priceToSections"][0]["sectionIdentifier"] =
            json!("UNKNOWN");
        sync_query(value, "ticketSelection");
    });
    assert!(extract_seat_map(URL, URL, &page).is_err());
    let page = change_metadata(|value| {
        let duplicate = state(value)["ticketSelection"]["sections"][0].clone();
        state(value)["ticketSelection"]["sections"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        sync_query(value, "ticketSelection");
    });
    assert!(extract_seat_map(URL, URL, &page).is_err());
    assert!(
        extract_seat_map(
            URL,
            URL,
            &PAGE.replacen(
                "\"eventId\":\"1234567890\"",
                "\"eventId\":\"1234567890\",\"eventId\":\"1234567890\"",
                1
            )
        )
        .is_err()
    );
    assert!(extract_seat_map(URL, URL, &"x".repeat(4 * 1024 * 1024 + 1)).is_err());
}

#[test]
fn rendering_surfaces_are_diagnostics_not_geometry_and_hidden_maps_are_rejected() {
    let page = PAGE.replace("<p>Mapa místa", "<canvas></canvas><canvas hidden></canvas><iframe title=\"Venue map\"></iframe><p>Mapa místa");
    let map = extract_seat_map(URL, URL, &page).unwrap();
    assert_eq!(map.visible_canvas_count, 1);
    assert_eq!(map.visible_iframe_count, 1);
    assert!(!map.geometry_verified && !map.stock_verified && !map.adjacency_verified);
    let hidden = PAGE.replace("<aside aria-label", "<aside hidden aria-label");
    assert!(extract_seat_map(URL, URL, &hidden).is_err());
    let challenged = PAGE.replace("<body>", "<body><h2>Verify you are human</h2>");
    assert!(extract_seat_map(URL, URL, &challenged).is_err());
    let challenged = PAGE.replace(
        "<body>",
        "<body><iframe title=\"captcha challenge\"></iframe>",
    );
    assert!(extract_seat_map(URL, URL, &challenged).is_err());
}

#[test]
fn known_visible_map_assets_must_match_metadata_but_do_not_verify_image_content() {
    let page = PAGE.replace("<p>Mapa místa", "<img alt=\"Venue map\" src=\"https://media.ticketmaster.eu/czechrepublic/map-a.png\"><p>Mapa místa");
    let map = extract_seat_map(URL, URL, &page).unwrap();
    assert!(map.displayed_reference_matched);
    assert!(!map.image_content_verified && !map.geometry_verified);
    let conflict = page.replacen(
        "<img alt=\"Venue map\" src=\"https://media.ticketmaster.eu/czechrepublic/map-a.png\">",
        "<img alt=\"Venue map\" src=\"https://media.ticketmaster.eu/czechrepublic/map-b.png\">",
        1,
    );
    assert!(extract_seat_map(URL, URL, &conflict).is_err());
    let responsive = page.replace(
        "<img alt=",
        "<img srcset=\"https://media.ticketmaster.eu/czechrepublic/map-b.png 1x\" alt=",
    );
    assert!(extract_seat_map(URL, URL, &responsive).is_err());
    let responsive = page.replace("<img alt=", "<picture><source hidden srcset=\"https://media.ticketmaster.eu/czechrepublic/map-b.png\"><img alt=")
        .replace("map-a.png\"><p>", "map-a.png\"></picture><p>");
    assert!(extract_seat_map(URL, URL, &responsive).is_err());
    assert!(
        !extract_seat_map(URL, URL, PAGE)
            .unwrap()
            .displayed_reference_matched
    );
}
