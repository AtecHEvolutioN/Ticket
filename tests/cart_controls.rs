use serde_json::{Value, json};
use ticket_watch::cart_controls::validate;

const URL: &str = "https://www.ticketmaster.cz/event/synthetic-vstupenky/1234567890";
const HTML: &str = include_str!("fixtures/cart-catalog-cz.html");
const NOW: i64 = 1_800_000_000_000;

fn evidence() -> Value {
    json!({"event_url":URL,"event_id":"1234567890","category_id":"2","category_title":"Sezení",
        "demo_quantity":2,"initial_quantity":0,"final_quantity":0,
        "started_at_ms":NOW-1000,"finished_at_ms":NOW-10,
        "network_disabled_before_changes":true,"form_restored":true,
        "panels":[
            {"kind":"price","heading":"Ceny","opened":true,"disabled":false,
             "options":[{"checkbox_id":"price-0","label":"1 889,00 Kč","checked":false,"disabled":false}]},
            {"kind":"section","heading":"Sekce","opened":true,"disabled":false,
             "options":[{"checkbox_id":":section-0:","label":"4. POSCHODÍ","checked":false,"disabled":false}]}],
        "stock_verified":false,"adjacency_verified":false,"fees_complete":false,
        "hold_verified":false,"reservation_attempted":false})
}

#[test]
fn binds_demo_controls_to_catalog_without_setting_a_purchase_instruction() {
    let report = validate(URL, URL, "2", 2, HTML, &evidence(), NOW).unwrap();
    assert_eq!(report.controls.demo_quantity, 2);
    assert_eq!(report.catalog.current_selected_count, 0);
    assert_eq!(report.controls.panels[0].options[0].label, "1 889,00 Kč");
    assert!(!report.reservation_authorized && !report.purchase_quantity_configured);
    assert!(!report.controls.stock_verified && !report.controls.hold_verified);
    let mut raw = evidence();
    raw["panels"][1] =
        json!({"kind":"section","heading":"Sekce","opened":false,"disabled":true,"options":[]});
    assert!(validate(URL, URL, "2", 2, HTML, &raw, NOW).is_ok());
}

#[test]
fn rejects_wrong_event_category_quantity_stale_time_and_inventory_claims() {
    for (field, bad) in [
        (
            "event_url",
            json!("https://www.ticketmaster.cz/event/other/9876543210"),
        ),
        ("event_id", json!("9876543210")),
        ("category_id", json!("1")),
        ("category_title", json!("A different category")),
        ("demo_quantity", json!(3)),
        ("started_at_ms", json!(NOW - 60_001)),
        ("finished_at_ms", json!(NOW + 1001)),
        ("network_disabled_before_changes", json!(false)),
        ("form_restored", json!(false)),
        ("initial_quantity", json!(1)),
        ("final_quantity", json!(2)),
        ("stock_verified", json!(true)),
        ("adjacency_verified", json!(true)),
        ("fees_complete", json!(true)),
        ("hold_verified", json!(true)),
        ("reservation_attempted", json!(true)),
    ] {
        let mut raw = evidence();
        raw[field] = bad;
        assert!(
            validate(URL, URL, "2", 2, HTML, &raw, NOW).is_err(),
            "{field}"
        );
    }
    for qty in [0, 7, 21] {
        let mut raw = evidence();
        raw["demo_quantity"] = json!(qty);
        assert!(validate(URL, URL, "2", qty, HTML, &raw, NOW).is_err());
    }
}

#[test]
fn rejects_unrestored_native_form_and_ambiguous_merged_category() {
    let selected = HTML.replacen("aria-valuenow=\"0\"", "aria-valuenow=\"1\"", 1);
    assert!(validate(URL, URL, "2", 2, &selected, &evidence(), NOW).is_err());
    let mut raw = evidence();
    raw["category_id"] = json!("10");
    raw["category_title"] = json!("Oficiální vstupenka Platinum");
    assert!(validate(URL, URL, "10", 2, HTML, &raw, NOW).is_err());
}

#[test]
fn rejects_incomplete_ambiguous_and_oversized_filter_evidence() {
    let mutations: [fn(&mut Value); 8] = [
        |v| {
            v["panels"][1]["kind"] = json!("price");
        },
        |v| {
            v["panels"][0]["options"] = json!([]);
        },
        |v| {
            v["panels"][0]["disabled"] = json!(true);
        },
        |v| {
            v["panels"][0]["options"][0]["label"] = json!("hidden\ntext");
        },
        |v| {
            v["panels"][0]["options"][0]["checkbox_id"] = json!("id with space");
        },
        |v| {
            let x = v["panels"][0]["options"][0].clone();
            v["panels"][0]["options"] = json!([x, x]);
        },
        |v| {
            v["panels"][0]["options"][0]["label"] = json!("x".repeat(501));
        },
        |v| {
            v["unexpected_hold_token"] = json!("unexpected");
        },
    ];
    for mutate in mutations {
        let mut raw = evidence();
        mutate(&mut raw);
        assert!(validate(URL, URL, "2", 2, HTML, &raw, NOW).is_err());
    }
}
