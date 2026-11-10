use ticket_watch::{cart::*, cart_journal::NativeSearchRequest, cart_preferences::*};

const EVENT: &str = "https://www.ticketmaster.cz/event/example/1472650657";
const NOW: i64 = 1_800_000_000_000;
fn plan_request() -> CartPlanRequest {
    serde_json::from_value(serde_json::json!({
        "intent":{"event_url":EVENT,"expected_event_start":"2030-11-16T18:00:00+01:00","quantity":2,"max_total_minor":null},
        "context":{"session_id":"session-1","document_id":"document-1"},
        "snapshot":{"event_url":EVENT,"event_start":"2030-11-16T17:00:00Z","session_id":"session-1","document_id":"document-1", "requested_quantity":2,"observed_at_ms":NOW,"expires_at_ms":NOW+5000,"coverage":"complete_for_request","groups":[{
            "id":"group-1","category_id":"category-1","ticket_type":"Standard", "quantity":2,"kind":"seated","availability":"confirmed","evidence":"exact_group","adjacency":"site_confirmed",
            "seats":[{"id":"seat-1","section":"A","row":"1","label":"1"},{"id":"seat-2","section":"A","row":"1","label":"2"}],
            "price":{"amount_minor":400000,"currency":"CZK","basis":"group_total","fees":"complete"}
        }]}
    })).unwrap()
}
fn native_request() -> NativeSearchRequest {
    NativeSearchRequest {
        event_url: EVENT.into(),
        expected_event_start: "2030-11-16T17:00:00Z".into(),
        source_event_start: "2030-11-16T17:00:00Z".into(),
        quantity: 2,
        session_id: "session-1".into(),
        document_id: "document-1".into(),
        config_id: "capture-1".into(),
        config_observed_at_ms: NOW,
        config_expires_at_ms: NOW + 5000,
        category_id: "category-1".into(),
        category_title: Some("Standard".into()),
        filters: CartFilters::default(),
        price_id: "price-1".into(),
        configured_unit_price_minor: 200000,
        currency: "CZK".into(),
        max_total_minor: None,
        adjacent_required: true,
        cheapest_required: true,
    }
}
fn filtered_preferences() -> PreferencesDocument {
    let mut doc = PreferencesDocument::default();
    doc.defaults.allowed_category_ids = vec!["category-1".into()];
    doc.defaults.allowed_ticket_types = vec![" standard ".into()];
    doc.defaults.allowed_sections = vec!["a".into()];
    doc.defaults.allowed_rows = vec!["1".into()];
    doc
}

#[test]
fn defaults_roundtrip_and_materialize_two_without_changing_captured_search() {
    let doc = PreferencesDocument::default();
    assert_eq!(
        doc,
        parse_document(&serde_json::to_vec(&doc).unwrap()).unwrap()
    );
    assert_eq!(doc.defaults.quantity, 2);
    assert!(doc.defaults.require_adjacent);
    let mut input = plan_request();
    input.intent.quantity = None;
    let prepared = apply_to_plan(&doc, &input).unwrap();
    assert_eq!(prepared.request.intent.quantity, Some(2));
    assert_eq!(
        prepared.effective_preferences.source,
        PreferenceSource::Defaults
    );
    assert_eq!(prepared.request.snapshot, input.snapshot);
    assert_eq!(plan(&prepared.request, NOW).outcome, PlanOutcome::Planned);
    assert!(input.intent.quantity.is_none());
}

#[test]
fn full_event_override_and_alias_identity_replace_all_globals() {
    let mut doc = filtered_preferences();
    doc.defaults.quantity = 3;
    let custom = CartPreferences {
        quantity: 4,
        max_total_minor: Some(800000),
        require_adjacent: false,
        ..CartPreferences::default()
    };
    doc.event_overrides.push(EventOverride {
        event_url: EVENT.into(),
        preferences: custom.clone(),
    });
    let resolved = resolve(
        &doc,
        "https://ticketmaster.cz/event/renamed/1472650657?language=en",
    )
    .unwrap();
    assert_eq!(resolved.source, PreferenceSource::EventOverride);
    assert_eq!(resolved.preferences, custom);
    let other = resolve(&doc, "https://ticketmaster.cz/event/1472650658").unwrap();
    assert_eq!(other.preferences.quantity, 3);
    assert!(apply_to_plan(&doc, &plan_request()).is_err());
    doc.event_overrides.push(EventOverride {
        event_url: "https://ticketmaster.cz/event/1472650657".into(),
        preferences: CartPreferences::default(),
    });
    assert!(validate_document(&doc).is_err());
}

#[test]
fn preference_filters_exclude_known_nonmatches_before_cheapest_selection() {
    let doc = filtered_preferences();
    let mut input = plan_request();
    for (id, change) in [
        ("wrong-category", 0),
        ("wrong-type", 1),
        ("wrong-section", 2),
        ("wrong-row", 3),
    ] {
        let mut group = input.snapshot.groups[0].clone();
        group.id = id.into();
        group.price.amount_minor = Some(1000);
        match change {
            0 => group.category_id = Some("category-2".into()),
            1 => group.ticket_type = Some("VIP".into()),
            2 => group.seats[0].section = "B".into(),
            _ => group.seats[0].row = "2".into(),
        }
        input.snapshot.groups.push(group);
    }
    let prepared = apply_to_plan(&doc, &input).unwrap();
    let decision = plan(&prepared.request, NOW);
    assert_eq!(decision.outcome, PlanOutcome::Planned);
    assert_eq!(decision.selected.unwrap().group_id, "group-1");
    assert_eq!(decision.rejected_groups.len(), 4);
    assert_eq!(prepared.request.intent.filters, doc.defaults.filters());
}

#[test]
fn missing_requested_category_or_type_evidence_prevents_cheapest_claim() {
    for missing_type in [false, true] {
        let doc = filtered_preferences();
        let mut input = plan_request();
        let mut unknown = input.snapshot.groups[0].clone();
        unknown.id = "unknown-cheaper".into();
        unknown.price.amount_minor = Some(1000);
        if missing_type {
            unknown.ticket_type = None;
        } else {
            unknown.category_id = None;
        }
        input.snapshot.groups.push(unknown);
        let decision = plan(&apply_to_plan(&doc, &input).unwrap().request, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Provisional);
        assert_eq!(decision.reason, Some(DecisionReason::UnresolvedCandidates));
        assert_eq!(decision.selected.unwrap().group_id, "group-1");
        assert!(!decision.reservation_authorized);
    }
}

#[test]
fn adjacency_false_supports_exact_seated_groups_without_adding_standing_or_duplicate_seats() {
    let mut doc = PreferencesDocument::default();
    doc.defaults.require_adjacent = false;
    let mut input = plan_request();
    let group = &mut input.snapshot.groups[0];
    group.adjacency = AdjacencyEvidence::NotAdjacent;
    group.seats[1].section = "B".into();
    group.seats[1].row = "2".into();
    group.seats[1].label = Some("1".into());
    assert_eq!(
        plan(&apply_to_plan(&doc, &input).unwrap().request, NOW).outcome,
        PlanOutcome::Planned
    );
    assert_eq!(plan(&input, NOW).outcome, PlanOutcome::Refused);
    input.snapshot.groups[0].kind = SeatingKind::Standing;
    assert_eq!(
        plan(&apply_to_plan(&doc, &input).unwrap().request, NOW).outcome,
        PlanOutcome::Refused
    );
    input.snapshot.groups[0].kind = SeatingKind::Seated;
    input.snapshot.groups[0].seats[1] = input.snapshot.groups[0].seats[0].clone();
    assert_eq!(
        plan(&apply_to_plan(&doc, &input).unwrap().request, NOW).outcome,
        PlanOutcome::Refused
    );
}

#[test]
fn complete_total_budget_and_unsupported_currency_are_applied_not_ignored() {
    let mut doc = PreferencesDocument::default();
    doc.defaults.max_total_minor = Some(399999);
    let prepared = apply_to_plan(&doc, &plan_request()).unwrap();
    assert_eq!(prepared.request.intent.max_total_minor, Some(399999));
    assert_eq!(plan(&prepared.request, NOW).outcome, PlanOutcome::Refused);
    doc.defaults.currency = "EUR".into();
    assert!(parse_document(&serde_json::to_vec(&doc).unwrap()).is_ok());
    assert!(apply_to_plan(&doc, &plan_request()).is_err());
    assert!(apply_to_native_search(&doc, &native_request()).is_err());
}

#[test]
fn native_preferences_materialize_quantity_and_enforce_category_type_budget_and_rows() {
    let mut doc = filtered_preferences();
    doc.defaults.quantity = 4;
    doc.defaults.require_adjacent = false;
    doc.defaults.max_total_minor = Some(800000);
    let input = native_request();
    let prepared = apply_to_native_search(&doc, &input).unwrap();
    assert_eq!(input.quantity, 2);
    assert_eq!(prepared.request.quantity, 4);
    assert!(!prepared.request.adjacent_required);
    assert_eq!(prepared.request.filters, doc.defaults.filters());
    assert_eq!(prepared.request.max_total_minor, Some(800000));
    doc.defaults.max_total_minor = Some(799999);
    assert!(apply_to_native_search(&doc, &input).is_err());
    doc.defaults.max_total_minor = None;
    let mut unknown = input.clone();
    unknown.category_title = None;
    assert!(apply_to_native_search(&doc, &unknown).is_err());
    unknown.category_title = Some("VIP".into());
    assert!(apply_to_native_search(&doc, &unknown).is_err());
    unknown = input;
    unknown.category_id = "category-2".into();
    assert!(apply_to_native_search(&doc, &unknown).is_err());
}

#[test]
fn strict_schema_rejects_missing_budget_unknown_fields_duplicates_and_private_error_text() {
    let good = serde_json::to_value(PreferencesDocument::default()).unwrap();
    for change in 0..6 {
        let mut json = good.clone();
        match change {
            0 => {
                json["defaults"]
                    .as_object_mut()
                    .unwrap()
                    .remove("max_total_minor");
            }
            1 => json["defaults"]["allow_restricted_view"] = false.into(),
            2 => json["defaults"]["price_strategy"] = "PRIVATE_SENTINEL".into(),
            3 => json["defaults"]["quantity"] = "PRIVATE_SENTINEL".into(),
            4 => json["defaults"]["max_total_minor"] = 0.into(),
            _ => json["defaults"]["allowed_rows"] = serde_json::json!(["A", " a "]),
        }
        let error = parse_document(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(!format!("{error:#}").contains("PRIVATE_SENTINEL"));
    }
    assert!(parse_document(br#"{"schema_version":1,"schema_version":1}"#).is_err());
    assert!(
        parse_document(&vec![
            b' ';
            ticket_watch::cart_preferences::MAX_INPUT_BYTES + 1
        ])
        .is_err()
    );
}
