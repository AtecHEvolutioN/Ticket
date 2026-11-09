use serde_json::{Value, json};
use ticket_watch::cart_catalog::{
    CatalogEvidence, CatalogScope, CatalogSeating, extract_cz_cart_catalog,
};

const URL: &str = "https://www.ticketmaster.cz/event/synthetic-vstupenky/1234567890";
const PAGE: &str = include_str!("fixtures/cart-catalog-cz.html");

fn change_metadata(change: impl FnOnce(&mut Value)) -> String {
    let start_marker = "<script id=\"__NEXT_DATA__\" type=\"application/json\">";
    let start = PAGE.find(start_marker).unwrap() + start_marker.len();
    let end = start + PAGE[start..].find("</script>").unwrap();
    let mut value: Value = serde_json::from_str(&PAGE[start..end]).unwrap();
    change(&mut value);
    format!("{}{}{}", &PAGE[..start], value, &PAGE[end..])
}

fn state(value: &mut Value) -> &mut Value {
    &mut value["props"]["pageProps"]["initialReduxState"]
}

fn synchronize_query(value: &mut Value) {
    let state = state(value);
    let selection = state["ticketSelection"].clone();
    let query = state["api"]["queries"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap();
    for (key, data) in query["data"].as_object_mut().unwrap() {
        *data = selection[key].clone();
    }
}

#[test]
fn exact_configuration_retains_all_categories_without_inventing_inventory() {
    let result = extract_cz_cart_catalog(URL, URL, PAGE).unwrap();
    assert_eq!(result.evidence, CatalogEvidence::ConfigurationOnly);
    assert_eq!(result.scope, CatalogScope::PrimaryConfiguration);
    assert!(!result.stock_verified && !result.hold_verified);
    assert_eq!(result.event_id, "1234567890");
    assert_eq!(result.event_name, "Synthetic concert");
    assert_eq!(result.currency, "CZK");
    assert_eq!(result.event_ticket_limit, 6);
    assert_eq!(result.current_selected_count, 0);
    assert_eq!(result.categories.len(), 5);
    assert_eq!(result.rendered_categories.len(), 4);
    assert!(
        result
            .categories
            .iter()
            .all(|c| c.seating == CatalogSeating::Unknown && !c.adjacent_candidate)
    );
    let seated = result.categories.iter().find(|c| c.id == "2").unwrap();
    assert_eq!(seated.prices[0].price_id, "6");
    assert_eq!(seated.prices[0].total_minor, 188900);
    assert_eq!(seated.prices[0].section_ids, ["4P"]);
    assert_eq!(seated.prices[1].price_level_title, "Platinum");
    assert_eq!(seated.discount_type, "STANDARD");
    let premium = result.categories.iter().find(|c| c.id == "10").unwrap();
    assert_eq!(premium.prices[0].price_id, seated.prices[1].price_id);
    assert_ne!(premium.prices[0].total_minor, seated.prices[1].total_minor);
    let display = result
        .rendered_categories
        .iter()
        .find(|c| c.title == premium.title)
        .unwrap();
    assert_eq!(display.category_ids, ["10", "11"]);
}

#[test]
fn matching_event_identity_works_for_korn_and_unrelated_generic_category_labels() {
    for (name, category_label) in [("KoRn", "Sezení"), ("An unrelated show", "Základní")] {
        let page = change_metadata(|value| {
            let state = state(value);
            state["eventInfo"]["name"] = json!(name);
            state["ticketSelection"]["ticketTypes"][1]["title"] = json!(category_label);
            synchronize_query(value);
        })
        .replace("<h1>Synthetic concert</h1>", &format!("<h1>{name}</h1>"))
        .replace(
            "<span>Sezení</span>",
            &format!("<span>{category_label}</span>"),
        );
        let result = extract_cz_cart_catalog(URL, URL, &page).unwrap();
        assert_eq!(result.event_name, name);
        assert_eq!(result.categories[1].title, category_label);
        assert_eq!(result.categories[1].seating, CatalogSeating::Unknown);
    }
}

#[test]
fn current_count_comes_from_visible_merged_stepper_once() {
    let (before, after) = PAGE.rsplit_once("aria-valuenow=\"0\"").unwrap();
    let page = format!("{before}aria-valuenow=\"2\"{after}");
    let result = extract_cz_cart_catalog(URL, URL, &page).unwrap();
    assert_eq!(result.current_selected_count, 2);
    let merged = result.rendered_categories.last().unwrap();
    assert_eq!(merged.category_ids, ["10", "11"]);
    assert_eq!(merged.current_selected_count, 2);
    assert!(
        result
            .categories
            .iter()
            .all(|c| c.initial_selected_quantity == 0)
    );
    let page = PAGE.replace("aria-valuenow=\"0\"", "aria-valuenow=\"2\"");
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err()); // Eight exceeds event limit.
}

#[test]
fn translations_hidden_markup_and_visible_adjacency_warning_are_distinct() {
    assert!(
        !extract_cz_cart_catalog(URL, URL, PAGE)
            .unwrap()
            .rendered_non_adjacent_warning
    );
    for attr in [
        "hidden",
        "aria-hidden=\"true\"",
        "style=\"display:none\"",
        "style=\"visibility:hidden\"",
    ] {
        let page = PAGE.replace("<div data-testid=\"modals\"></div>", &format!("<div role=\"dialog\" {attr}><h2>Upozornění: vstupenky nemusí být vedle sebe</h2></div>"));
        assert!(
            !extract_cz_cart_catalog(URL, URL, &page)
                .unwrap()
                .rendered_non_adjacent_warning
        );
    }
    let page = PAGE.replace("<div data-testid=\"modals\"></div>", "<div role=\"dialog\"><h2>Upozornění: vstupenky nemusí být vedle sebe</h2><button>Upravit výběr</button></div>");
    let result = extract_cz_cart_catalog(URL, URL, &page).unwrap();
    assert!(result.rendered_non_adjacent_warning);
    assert!(result.categories.iter().all(|c| !c.adjacent_candidate));
}

#[test]
fn rejects_cross_event_or_unbound_metadata_at_each_binding_layer() {
    for pointer in [
        "/query/eventId",
        "/props/pageProps/initialReduxState/eventId",
        "/props/pageProps/initialReduxState/eventInfo/eventId",
        "/props/pageProps/initialReduxState/eventInfo/params",
        "/props/pageProps/initialReduxState/ticketSelection/params/eventId",
    ] {
        for bad in [Value::Null, json!("9876543210")] {
            let page = change_metadata(|value| *value.pointer_mut(pointer).unwrap() = bad);
            assert!(
                extract_cz_cart_catalog(URL, URL, &page).is_err(),
                "accepted {pointer}"
            );
        }
    }
    let page = change_metadata(|value| {
        let query = state(value)["api"]["queries"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap();
        query["originalArgs"]["eventId"] = json!("9876543210");
    });
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    let page = change_metadata(|value| {
        let queries = state(value)["api"]["queries"].as_object_mut().unwrap();
        let first_key = queries.keys().next().unwrap().clone();
        let query = queries.remove(&first_key).unwrap();
        queries.insert(
            "ticketSelection({\"eventId\":\"9876543210\"})".into(),
            query,
        );
    });
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
}

#[test]
fn rejects_redirects_artist_pages_challenges_and_conflicting_page_identity() {
    for actual in [
        "https://www.ticketmaster.cz/event/other/9876543210",
        "https://www.ticketmaster.cz/artist/1234567890",
        "https://www.ticketmaster.pl/event/1234567890",
        "https://secure.ticketmaster.cz/event/1234567890",
        "https://www.ticketmaster.cz/event/1234567890?token=secret",
    ] {
        assert!(extract_cz_cart_catalog(URL, actual, PAGE).is_err());
    }
    for marker in [
        "<h2>Verify you are human</h2>",
        "<h2>Your browsing activity has been paused</h2>",
        "<iframe title=\"recaptcha challenge\"></iframe>",
    ] {
        assert!(
            extract_cz_cart_catalog(
                URL,
                URL,
                &PAGE.replace("<body>", &format!("<body>{marker}"))
            )
            .is_err()
        );
    }
    assert!(
        extract_cz_cart_catalog(
            URL,
            URL,
            &PAGE.replace("<h1>Synthetic concert</h1>", "<h1>Another artist</h1>")
        )
        .is_err()
    );
    assert!(
        extract_cz_cart_catalog(
            URL,
            URL,
            &PAGE.replacen(URL, "https://www.ticketmaster.cz/event/9876543210", 1)
        )
        .is_err()
    );
}

#[test]
fn rejects_currency_disagreement_missing_currency_and_invalid_money() {
    for currency in [Value::Null, json!("EUR"), json!("czk")] {
        let page = change_metadata(|value| state(value)["eventInfo"]["currencyCode"] = currency);
        assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    }
    let page = change_metadata(|value| {
        state(value)["ticketSelection"]["ticketTypes"][0]["prices"][0]["currency"] = json!("EUR");
        synchronize_query(value);
    });
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    assert!(extract_cz_cart_catalog(URL, URL, &PAGE.replace("Kč", "EUR")).is_err());
    for amount in [json!(-1), json!(1.234), json!("1750"), json!(100000001)] {
        let page = change_metadata(|value| {
            state(value)["ticketSelection"]["ticketTypes"][0]["prices"][0]["faceValue"] = amount;
            synchronize_query(value);
        });
        assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    }
    for numeric_literal in ["1750.00000000000000001", "1.75e3", "1750.001"] {
        let page = PAGE.replace(
            "\"faceValue\":1750",
            &format!("\"faceValue\":{numeric_literal}"),
        );
        assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    }
    let page = change_metadata(|value| {
        state(value)["ticketSelection"]["ticketTypes"][0]["prices"][0]["priceBreakdown"][0]["value"] =
            json!(174999);
        synchronize_query(value);
    });
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
}

#[test]
fn accepts_exact_cent_values_and_preserves_integer_minor_units() {
    let page = change_metadata(|value| {
        let price = &mut state(value)["ticketSelection"]["ticketTypes"][0]["prices"][0];
        price["faceValue"] = json!(1750.25);
        price["priceBreakdown"][0]["value"] = json!(175025);
        price["serviceFeeChargesValue"] = json!(375.50);
        synchronize_query(value);
    })
    .replacen("2 125,00 Kč", "2 125,75 Kč", 1);
    let result = extract_cz_cart_catalog(URL, URL, &page).unwrap();
    assert_eq!(result.categories[0].prices[0].total_minor, 212575);
}

#[test]
fn separate_resale_rows_are_excluded_and_primary_price_mismatch_is_rejected() {
    let listing = "<ul><li role=\"button\"><div data-testid=\"ticketTypeInfo\"><span>Ověřená přeprodávaná vstupenka</span><span>Dostupných: 2</span><span>3 335,00 Kč za kus</span></div></li></ul>";
    let page = PAGE.replace("</body>", &format!("{listing}</body>"));
    let result = extract_cz_cart_catalog(URL, URL, &page).unwrap();
    assert_eq!(result.ignored_resale_listing_rows, 1);
    assert_eq!(result.rendered_categories.len(), 4);
    assert_eq!(result.current_selected_count, 0);
    assert!(!result.stock_verified);
    assert!(
        extract_cz_cart_catalog(URL, URL, &PAGE.replacen("2 125,00 Kč", "1 999,00 Kč", 1)).is_err()
    );
    assert!(
        extract_cz_cart_catalog(
            URL,
            URL,
            &PAGE.replace(
                "<span>Sezení</span>",
                "<span>Unrecognized primary category</span>"
            )
        )
        .is_err()
    );
}

#[test]
fn rejects_stale_cache_ambiguous_ids_and_duplicate_json_properties() {
    let page =
        change_metadata(|value| state(value)["ticketSelection"]["eventTicketLimit"] = json!(5));
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    let page = change_metadata(|value| {
        state(value)["ticketSelection"]["ticketTypes"][1]["id"] = json!("1");
        synchronize_query(value);
    });
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    let page = PAGE.replacen(
        "\"eventId\":\"1234567890\"",
        "\"eventId\":\"9876543210\",\"eventId\":\"1234567890\"",
        1,
    );
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    assert!(
        extract_cz_cart_catalog(
            URL,
            URL,
            &PAGE.replace(
                "</body>",
                "<script id=\"__NEXT_DATA__\" type=\"application/json\">{}</script></body>"
            )
        )
        .is_err()
    );
    for stale_field in ["eventInfo", "ticketSelection"] {
        let page = change_metadata(|value| {
            state(value)[stale_field]["inFlightParams"] = json!({"eventId":"1234567890"})
        });
        assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    }
}

#[test]
fn bounds_input_arrays_and_quantities_before_producing_a_catalog() {
    assert!(extract_cz_cart_catalog(URL, URL, &" ".repeat(4 * 1024 * 1024 + 1)).is_err());
    let page = change_metadata(|value| {
        let selection = &mut state(value)["ticketSelection"];
        let category = selection["ticketTypes"][0].clone();
        selection["ticketTypes"] = json!(vec![category; 129]);
        synchronize_query(value);
    });
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
    for value in ["-1", "1.5", "1001", "7"] {
        assert!(
            extract_cz_cart_catalog(
                URL,
                URL,
                &PAGE.replacen(
                    "aria-valuenow=\"0\"",
                    &format!("aria-valuenow=\"{value}\""),
                    1
                )
            )
            .is_err()
        );
    }
    let page = change_metadata(|value| {
        state(value)["ticketSelection"]["ticketTypes"][0]["quantities"] = json!([0, 1, 1, 6]);
        synchronize_query(value);
    });
    assert!(extract_cz_cart_catalog(URL, URL, &page).is_err());
}
