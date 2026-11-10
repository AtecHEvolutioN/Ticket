//! The fixture is the real JSON written by Android's Save & export flow using
//! synthetic watches on the API-28 instrumentation device. Keep its schema and
//! numeric representation intact so this remains an interchange regression.
use ticket_watch::{
    cart::{
        CartPlanRequest, DecisionReason, GroupQuote, GroupRejectionReason, PlanOutcome,
        RankingScope, plan,
    },
    cart_preferences::{
        PreferenceSource, PreferencesDocument, apply_to_plan, parse_document, resolve,
    },
};

const NOW: i64 = 1_800_000_000_000;
const EVENT: &str = "https://www.ticketmaster.cz/event/synthetic-preferences/88770901";
const ALIAS: &str = "https://ticketmaster.cz/event/renamed/88770901?language=en";
const ANDROID_EXPORT: &[u8] = include_bytes!("fixtures/android-ticket-preferences.json");

fn preferences() -> PreferencesDocument {
    parse_document(ANDROID_EXPORT).unwrap()
}

fn group(id: &str, amount: u64) -> GroupQuote {
    serde_json::from_value(serde_json::json!({
        "id":id,"category_id":"2","ticket_type":"OVĚŘENÉ  PŘEPRODÁVANÉ\u{a0}VSTUPENKY",
        "quantity":3,"kind":"seated","availability":"confirmed","evidence":"exact_group",
        "adjacency":"not_adjacent",
        "seats":[
            {"id":format!("{id}:1"),"section":"110","row":"12","label":"1"},
            {"id":format!("{id}:2"),"section":"110","row":"12","label":"4"},
            {"id":format!("{id}:3"),"section":"110","row":"12","label":"7"}
        ],
        "price":{"amount_minor":amount,"currency":"CZK","basis":"group_total","fees":"complete"}
    }))
    .unwrap()
}

fn request() -> CartPlanRequest {
    serde_json::from_value(serde_json::json!({
        "intent":{"event_url":EVENT,"expected_event_start":"2030-11-16T18:00:00+01:00","quantity":null,"max_total_minor":null},
        "context":{"session_id":"contract-session","document_id":"contract-document"},
        "snapshot":{"event_url":ALIAS,"event_start":"2030-11-16T17:00:00Z",
            "session_id":"contract-session","document_id":"contract-document","requested_quantity":3,
            "observed_at_ms":NOW,"expires_at_ms":NOW+5000,"coverage":"complete_for_request",
            "groups":[group("eligible",500000)]}
    })).unwrap()
}

#[test]
fn actual_android_export_preserves_global_values_and_full_event_override_across_url_aliases() {
    let doc = preferences();
    assert_eq!(doc.defaults.quantity, 4);
    assert_eq!(doc.defaults.max_total_minor, Some(600050));
    let event = resolve(&doc, ALIAS).unwrap();
    assert_eq!(event.source, PreferenceSource::EventOverride);
    assert_eq!(event.preferences.quantity, 3);
    assert_eq!(event.preferences.max_total_minor, Some(825000));
    assert!(!event.preferences.require_adjacent);
    assert_eq!(event.preferences.allowed_category_ids, ["2"]);
    assert_eq!(
        event.preferences.allowed_ticket_types,
        ["Ověřené přeprodávané vstupenky"]
    );
    assert_eq!(event.preferences.allowed_sections, ["110"]);
    assert_eq!(event.preferences.allowed_rows, ["12"]);
    let sibling = resolve(&doc, "https://ticketmaster.cz/event/88770902").unwrap();
    assert_eq!(sibling.source, PreferenceSource::Defaults);
    assert_eq!(sibling.preferences, doc.defaults);
    assert_eq!(sibling.preferences.allowed_ticket_types, ["Standard"]);
}

#[test]
fn android_filters_and_adjacency_setting_apply_before_cheapest_group_selection() {
    let doc = preferences();
    let mut input = request();
    for (id, change) in [("category", 0), ("type", 1), ("section", 2), ("row", 3)] {
        let mut wrong = group(id, 1000);
        match change {
            0 => wrong.category_id = Some("02".into()),
            1 => wrong.ticket_type = Some("VIP".into()),
            2 => wrong.seats[0].section = "1101".into(),
            _ => wrong.seats[0].row = "120".into(),
        }
        input.snapshot.groups.push(wrong);
    }
    let original = input.clone();
    let prepared = apply_to_plan(&doc, &input).unwrap();
    assert_eq!(input, original);
    assert_eq!(prepared.request.snapshot, original.snapshot);
    assert_eq!(prepared.request.intent.quantity, Some(3));
    let decision = plan(&prepared.request, NOW);
    assert_eq!(decision.outcome, PlanOutcome::Planned);
    assert_eq!(
        decision.ranking_scope,
        Some(RankingScope::CheapestForRequest)
    );
    let selected = decision.selected.unwrap();
    assert_eq!(selected.group_id, "eligible");
    assert_eq!(selected.quantity, 3);
    assert_eq!(selected.total_minor, 500000);
    assert_eq!(
        decision
            .rejected_groups
            .iter()
            .map(|g| g.reason)
            .collect::<Vec<_>>(),
        [
            GroupRejectionReason::CategoryMismatch,
            GroupRejectionReason::RowMismatch,
            GroupRejectionReason::SectionMismatch,
            GroupRejectionReason::TicketTypeMismatch,
        ]
    );
    assert!(!decision.reservation_authorized && !decision.reservation_performed);
}

#[test]
fn android_settings_never_rewrite_captured_quantity_or_ignore_unknown_filters_and_budget() {
    let doc = preferences();
    let mut input = request();
    input.snapshot.requested_quantity = 2;
    assert!(apply_to_plan(&doc, &input).is_err());
    input.snapshot.requested_quantity = 3;
    let mut unknown = group("unknown-cheaper", 1000);
    unknown.category_id = None;
    input.snapshot.groups.push(unknown);
    let decision = plan(&apply_to_plan(&doc, &input).unwrap().request, NOW);
    assert_eq!(decision.outcome, PlanOutcome::Provisional);
    assert_eq!(decision.reason, Some(DecisionReason::UnresolvedCandidates));
    assert!(!decision.reservation_authorized);
    input.snapshot.groups = vec![group("above-total-limit", 825001)];
    let decision = plan(&apply_to_plan(&doc, &input).unwrap().request, NOW);
    assert_eq!(decision.outcome, PlanOutcome::Refused);
    assert_eq!(
        decision.rejected_groups[0].reason,
        GroupRejectionReason::AboveMaximumTotal
    );
}
