use serde_json::{Value, json};
use ticket_watch::{
    cart_catalog::{CzCartCatalog, extract_cz_cart_catalog},
    cart_controls::{self, BrowserCartControls},
    cart_preferences::PreferencesDocument,
    cart_preparation::{NativeSelection, evaluate, validate_preparation, validate_selected},
};

const URL: &str = "https://www.ticketmaster.cz/event/synthetic-vstupenky/1234567890";
const HTML: &str = include_str!("fixtures/cart-catalog-cz.html");
const NOW: i64 = 1_800_000_000_000;

fn catalog() -> CzCartCatalog {
    extract_cz_cart_catalog(URL, URL, HTML).unwrap()
}
fn selection() -> NativeSelection {
    NativeSelection {
        price_id: "6".into(),
        section_ids: vec!["4P".into()],
    }
}
fn raw_controls() -> Value {
    json!({"event_url":URL,"event_id":"1234567890","category_id":"2","category_title":"Sezení",
        "demo_quantity":2,"initial_quantity":0,"final_quantity":0,
        "started_at_ms":NOW-1000,"finished_at_ms":NOW-10,
        "network_disabled_before_changes":true,"form_restored":true,
        "panels":[
            {"kind":"price","heading":"Cena","opened":true,"disabled":false,
             "options":[
                 {"checkbox_id":"_r_7_-input","label":"2 361,00 Kč • Sezení Platinum","checked":false,"disabled":false},
                 {"checkbox_id":"_r_b_-input","label":"1 889,00 Kč • Sezení 5. kategorie","checked":false,"disabled":false}]},
            {"kind":"section","heading":"Sekce","opened":true,"disabled":false,
             "options":[
                 {"checkbox_id":"_r_d_-input","label":"1. POSCHODÍ","checked":false,"disabled":false},
                 {"checkbox_id":"_r_g_-input","label":"4. POSCHODÍ","checked":false,"disabled":false}]}],
        "stock_verified":false,"adjacency_verified":false,"fees_complete":false,"hold_verified":false,"reservation_attempted":false})
}
fn prepared() -> Value {
    let mut raw = raw_controls();
    raw["panels"][0]["options"][1]["checked"] = true.into();
    raw["panels"][1]["options"][1]["checked"] = true.into();
    raw["preparation"] = json!({"price_id":"6","section_ids":["4P"],"effective_section_ids":["4P"],
        "price_option":{"checkbox_id":"_r_b_-input","label":"1 889,00 Kč • Sezení 5. kategorie"},
        "section_options":[{"section_id":"4P","checkbox_id":"_r_g_-input","label":"4. POSCHODÍ"}],
        "price_selection_verified":true,"section_selection_verified":true,"filters_restored":true});
    raw
}

#[test]
fn defaults_order_all_configured_choices_without_assuming_seating_or_ignoring_restricted_view() {
    let report = evaluate(&PreferencesDocument::default(), &catalog(), None).unwrap();
    assert_eq!(report.effective_preferences.preferences.quantity, 2);
    assert_eq!(report.candidates[0].category_id, "23");
    assert_eq!(report.candidates[0].price_id, "11");
    assert_eq!(report.candidates[0].configured_group_subtotal_minor, 342400);
    assert!(report.candidates.iter().any(|c| c.category_id == "1"));
    assert!(
        report
            .candidates
            .iter()
            .any(|c| c.category_id == "10" && !c.native_category_control_supported)
    );
    assert!(
        report
            .group_filters_pending
            .contains(&"site_confirmed_adjacency")
    );
    assert!(!report.stock_verified && !report.seating_verified && !report.adjacency_verified);
    assert!(
        !report.fees_complete
            && !report.cheapest_available_verified
            && !report.hold_verified
            && !report.reservation_authorized
    );
}

#[test]
fn nonstandard_prices_remain_ranked_but_cannot_use_the_standard_native_filter_contract() {
    for discount in ["PLATINUM", "COLLECTOR"] {
        let mut c = catalog();
        c.categories
            .iter_mut()
            .find(|category| category.id == "23")
            .unwrap()
            .discount_type = discount.into();
        let report = evaluate(&PreferencesDocument::default(), &c, None).unwrap();
        assert_eq!(report.candidates[0].category_id, "23");
        assert_eq!(report.candidates[0].configured_group_subtotal_minor, 342400);
        assert!(!report.candidates[0].native_category_control_supported);
        assert!(
            validate_selected(
                &report,
                "23",
                &NativeSelection {
                    price_id: "11".into(),
                    section_ids: vec!["4P".into()]
                }
            )
            .is_err()
        );

        c.categories
            .iter_mut()
            .find(|category| category.id == "2")
            .unwrap()
            .discount_type = discount.into();
        let controls: BrowserCartControls = serde_json::from_value(raw_controls()).unwrap();
        assert!(evaluate(&PreferencesDocument::default(), &c, Some(&controls)).is_err());

        // Update both embedded selection/query copies without changing their
        // visible labels, so a matching price alone cannot authorize the adapter.
        let html = HTML.replace("\"STANDARD\"", &format!("\"{discount}\""));
        assert!(extract_cz_cart_catalog(URL, URL, &html).is_ok());
        assert!(
            validate_preparation(URL, URL, "2", 2, &selection(), &html, &prepared(), NOW).is_err()
        );
    }
}

#[test]
fn category_type_quantity_and_subtotal_budget_filters_do_not_invent_row_or_floor_mappings() {
    let mut document = PreferencesDocument::default();
    document.defaults.allowed_ticket_types = vec![" sezení ".into()];
    document.defaults.allowed_category_ids = vec!["2".into()];
    document.defaults.allowed_sections = vec!["110".into()];
    document.defaults.allowed_rows = vec!["12".into()];
    document.defaults.max_total_minor = Some(377800);
    let mut c = catalog();
    c.handling_fee_minor = 10000;
    let report = evaluate(&document, &c, None).unwrap();
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].price_id, "6");
    assert_eq!(report.candidates[0].configured_section_ids, ["4P"]);
    assert!(
        report
            .group_filters_pending
            .contains(&"actual_seat_sections")
    );
    assert!(report.group_filters_pending.contains(&"actual_seat_rows"));
    assert_eq!(report.handling_fee_scope, "unverified");
    assert!(!report.fees_complete);
    assert!(validate_selected(&report, "2", &selection()).is_ok());
    assert!(
        validate_selected(
            &report,
            "2",
            &NativeSelection {
                price_id: "6".into(),
                section_ids: vec!["110".into()]
            }
        )
        .is_err()
    );
    document.defaults.max_total_minor = Some(377799);
    assert!(evaluate(&document, &c, None).unwrap().candidates.is_empty());
    document.defaults.max_total_minor = None;
    document.defaults.quantity = 7;
    assert!(evaluate(&document, &c, None).unwrap().candidates.is_empty());
    document.defaults.currency = "EUR".into();
    assert!(evaluate(&document, &c, None).is_err());
}

#[test]
fn maps_reordered_equal_prices_by_full_labels_and_never_checkbox_positions() {
    let mut c = catalog();
    let category = c.categories.iter_mut().find(|x| x.id == "2").unwrap();
    let mut other = category.prices[0].clone();
    other.price_id = "99".into();
    other.price_level_title = "Different category".into();
    category.prices.push(other);
    let mut raw = raw_controls();
    raw["panels"][0]["options"].as_array_mut().unwrap().insert(0,json!({"checkbox_id":"dynamic-99","label":"1 889,00 Kč • Sezení Different category","checked":false,"disabled":false}));
    let controls: BrowserCartControls = serde_json::from_value(raw).unwrap();
    let report = evaluate(&PreferencesDocument::default(), &c, Some(&controls)).unwrap();
    let mapping = report.native_mapping.unwrap();
    assert_eq!(
        mapping
            .prices
            .iter()
            .map(|p| p.price_id.as_str())
            .collect::<Vec<_>>(),
        ["99", "2", "6"]
    );
    assert_eq!(
        mapping
            .sections
            .iter()
            .map(|s| s.section_id.as_str())
            .collect::<Vec<_>>(),
        ["1P", "4P"]
    );
}

#[test]
fn incomplete_duplicate_malformed_or_ambiguous_native_mapping_is_rejected() {
    let mutations: [fn(&mut Value); 8] = [
        |v| {
            v["panels"][0]["options"].as_array_mut().unwrap().pop();
        },
        |v| {
            v["panels"][1]["options"].as_array_mut().unwrap().pop();
        },
        |v| {
            v["panels"][0]["options"][1]["label"] = "1 889,00 Kč".into();
        },
        |v| {
            v["panels"][0]["options"][1]["label"] = "1 889,01 Kč • Sezení 5. kategorie".into();
        },
        |v| {
            v["panels"][0]["options"][1]["label"] = "18 89,00 Kč • Sezení 5. kategorie".into();
        },
        |v| {
            v["panels"][0]["options"][1]["checkbox_id"] = "_r_7_-input".into();
        },
        |v| {
            v["panels"][1]["options"][1]["label"] = "110".into();
        },
        |v| {
            v["demo_quantity"] = 3.into();
        },
    ];
    for change in mutations {
        let mut raw = raw_controls();
        change(&mut raw);
        let controls: BrowserCartControls = serde_json::from_value(raw).unwrap();
        assert!(evaluate(&PreferencesDocument::default(), &catalog(), Some(&controls)).is_err());
    }
    let mut c = catalog();
    c.sections.iter_mut().find(|s| s.id == "1P").unwrap().title = "4. POSCHODÍ".into();
    assert!(
        evaluate(
            &PreferencesDocument::default(),
            &c,
            Some(&serde_json::from_value(raw_controls()).unwrap())
        )
        .is_err()
    );
}

#[test]
fn prepared_filters_require_explicit_selection_and_match_the_checked_panel_evidence() {
    let raw = prepared();
    assert!(cart_controls::validate(URL, URL, "2", 2, HTML, &raw, NOW).is_err());
    let report = validate_preparation(URL, URL, "2", 2, &selection(), HTML, &raw, NOW).unwrap();
    assert!(report.controls.preparation.unwrap().filters_restored);
    assert!(!report.purchase_quantity_configured && !report.reservation_authorized);
    let ordinary = raw_controls();
    assert!(validate_preparation(URL, URL, "2", 2, &selection(), HTML, &ordinary, NOW).is_err());
}

#[test]
fn unrequested_price_sections_disabled_checked_options_and_unrestored_filters_are_rejected() {
    let mutations: [fn(&mut Value); 12] = [
        |v| {
            v["preparation"]["price_id"] = "2".into();
        },
        |v| {
            v["preparation"]["section_ids"] = json!([]);
        },
        |v| {
            v["preparation"]["effective_section_ids"] = json!(["1P"]);
        },
        |v| {
            v["preparation"]["price_selection_verified"] = false.into();
        },
        |v| {
            v["preparation"]["section_selection_verified"] = false.into();
        },
        |v| {
            v["preparation"]["filters_restored"] = false.into();
        },
        |v| {
            v["preparation"]["price_option"]["checkbox_id"] = "other".into();
        },
        |v| {
            v["preparation"]["section_options"][0]["checkbox_id"] = "other".into();
        },
        |v| {
            v["panels"][0]["options"][0]["checked"] = true.into();
        },
        |v| {
            v["panels"][0]["options"][1]["disabled"] = true.into();
        },
        |v| {
            v["panels"][1]["options"][0]["checked"] = true.into();
        },
        |v| {
            v["panels"][1]["options"][1]["disabled"] = true.into();
        },
    ];
    for change in mutations {
        let mut raw = prepared();
        change(&mut raw);
        assert!(validate_preparation(URL, URL, "2", 2, &selection(), HTML, &raw, NOW).is_err());
    }
}

#[test]
fn empty_native_section_request_means_all_configured_compatible_floors_not_all_seat_sections() {
    let mut raw = prepared();
    raw["preparation"]["section_ids"] = json!([]);
    let all = NativeSelection {
        price_id: "6".into(),
        section_ids: vec![],
    };
    assert!(validate_preparation(URL, URL, "2", 2, &all, HTML, &raw, NOW).is_ok());
    raw["preparation"]["effective_section_ids"] = json!(["1P", "4P"]);
    assert!(validate_preparation(URL, URL, "2", 2, &all, HTML, &raw, NOW).is_err());
    assert!(
        validate_selected(
            &evaluate(&PreferencesDocument::default(), &catalog(), None).unwrap(),
            "10",
            &NativeSelection {
                price_id: "2".into(),
                section_ids: vec!["1P".into()]
            }
        )
        .is_err()
    );
}
