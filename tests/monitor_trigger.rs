use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use ticket_watch::{
    cart_preferences::{CartPreferences, EventOverride, PreferenceSource, PreferencesDocument},
    catalog::{self, Selection, Snapshot},
    config::{Config, PageAdapter, Subscription, Watch, WebsiteProfile},
    model::{Availability, Evidence, FeeScope, Observation, Offer, PriceScope, QuantityKind},
    monitor_trigger::{self, Check, MissingEvidence, OfferReason, SubtotalBasis, TriggerReason},
    preference_store::PreferenceSnapshot,
    store::Store,
};

const NOW: i64 = 1_800_000_000_500;
const EVENT: &str = "https://www.ticketmaster.cz/event/example-concert/123456789";

fn selected_watch() -> Watch {
    let mut watch = Watch {
        id: "selected".into(),
        url: EVENT.into(),
        currency: Some("CZK".into()),
        ..Watch::default()
    };
    watch.parser.adapter = PageAdapter::Ticketmaster;
    watch
}

fn offer() -> Offer {
    Offer {
        key: "private-provider-key".into(),
        section: Some("101".into()),
        row: Some("4".into()),
        seats: Some("1-2".into()),
        ticket_type: Some("Standard Ticket".into()),
        inventory: Some("private inventory text".into()),
        price_text: Some("https://private.invalid/checkout?token=private-value".into()),
        price_minor: Some(1000),
        currency: Some("CZK".into()),
        quantity: Some(2),
        quantity_kind: QuantityKind::Listing,
        price_scope: PriceScope::PerTicket,
        fees: FeeScope::Unknown,
    }
}

fn observation() -> Observation {
    Observation {
        watch_id: "selected".into(),
        observed_at: NOW / 1000,
        availability: Availability::Available,
        evidence: Evidence::Offers,
        complete: false,
        offers: vec![offer()],
        ticket_categories: vec![],
        adapter: Some("ticketmaster-regional-v1".into()),
        detail: "private diagnostic with https://private.invalid/session".into(),
    }
}

fn preferences(document: PreferencesDocument) -> PreferenceSnapshot {
    PreferenceSnapshot {
        revision: 1,
        document_hash: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&document).unwrap())
        ),
        updated_at_ms: 0,
        document,
    }
}

struct Fixture {
    _directory: TempDir,
    config: Config,
    preferences: PreferenceSnapshot,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = Config {
            database: directory.path().join("monitor.sqlite"),
            watches: vec![selected_watch()],
            ..Config::default()
        };
        Store::open(&config.database)
            .unwrap()
            .apply(&config.watches[0], &observation())
            .unwrap();
        Self {
            _directory: directory,
            config,
            preferences: preferences(PreferencesDocument::default()),
        }
    }
    fn preview(&self) -> monitor_trigger::TriggerPreview {
        monitor_trigger::preview(&self.config, "selected", &self.preferences, NOW).unwrap()
    }
    fn replace(&self, observed: Observation) {
        Store::open(&self.config.database)
            .unwrap()
            .apply(&self.config.watches[0], &observed)
            .unwrap();
    }
    fn raw(&self, observed: serde_json::Value) {
        let connection = Connection::open(&self.config.database).unwrap();
        connection
            .execute(
                "UPDATE watches SET last_observed_at=?1 WHERE id='selected'",
                [observed["observed_at"].as_i64().unwrap()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE watch_observations SET observation_json=?1 WHERE watch_id='selected'",
                [observed.to_string()],
            )
            .unwrap();
    }
    fn with_defaults(&mut self, prefs: CartPreferences) {
        self.preferences = preferences(PreferencesDocument {
            defaults: prefs,
            ..PreferencesDocument::default()
        });
    }
}

#[test]
fn fresh_positive_only_recommends_new_preparation_with_receipt_checks_unverified() {
    let mut fixture = Fixture::new();
    fixture.with_defaults(CartPreferences {
        allowed_category_ids: vec!["23".into()],
        ..CartPreferences::default()
    });
    let before = std::fs::read(&fixture.config.database).unwrap();
    let result = fixture.preview();
    assert!(result.needs_fresh_native_preparation);
    assert!(!result.reservation_authorized);
    assert!(!result.cheapest_verified);
    assert!(!result.source_snapshot_complete);
    assert_eq!(result.preference_revision, 1);
    assert_eq!(result.observation_age_ms, Some(500));
    assert_eq!(result.observation_timestamp_resolution_ms, 1000);
    assert_eq!(result.observed_offer_rows, 1);
    assert_eq!(result.offers[0].checks.quantity, Check::Matches);
    assert_eq!(result.offers[0].checks.category, Check::Unknown);
    assert_eq!(result.offers[0].checks.adjacency, Check::Unknown);
    assert_eq!(result.offers[0].displayed_group_subtotal_minor, Some(2000));
    for missing in [
        MissingEvidence::NativeCategoryIdentity,
        MissingEvidence::AuthenticatedPerformance,
        MissingEvidence::ExactSeats,
        MissingEvidence::Adjacency,
        MissingEvidence::CompleteGroupTotal,
        MissingEvidence::CheckoutFees,
        MissingEvidence::CheapestEligibleGroup,
        MissingEvidence::JournalLockNotChecked,
    ] {
        assert!(result.missing_evidence.contains(&missing));
        assert!(result.offers[0].missing_evidence.contains(&missing));
    }
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("private"));
    assert_eq!(std::fs::read(&fixture.config.database).unwrap(), before);
}

#[test]
fn latest_blocked_or_unknown_replaces_the_old_positive_for_preview_only() {
    for availability in [
        Availability::Blocked,
        Availability::Unknown,
        Availability::Unavailable,
    ] {
        let fixture = Fixture::new();
        let mut latest = observation();
        latest.availability = availability;
        latest.evidence = Evidence::None;
        latest.offers.clear();
        fixture.replace(latest);
        let result = fixture.preview();
        assert!(!result.needs_fresh_native_preparation);
        assert!(result.offers.is_empty());
        assert_eq!(
            result.reasons,
            vec![match availability {
                Availability::Blocked => TriggerReason::LatestObservationBlocked,
                Availability::Unknown => TriggerReason::LatestObservationUnknown,
                _ => TriggerReason::LatestObservationUnavailable,
            }]
        );
    }
}

#[test]
fn stale_future_zero_and_overflow_observation_times_never_trigger() {
    for (timestamp, expected) in [
        (NOW / 1000 - 5, TriggerReason::ObservationStale),
        (NOW / 1000 + 1, TriggerReason::ObservationFromFuture),
        (0, TriggerReason::ObservationTimeUnknown),
        (i64::MAX, TriggerReason::ObservationTimeUnknown),
        (
            (ticket_watch::cart::MAX_EXACT_INTEGER / 1000 + 1) as i64,
            TriggerReason::ObservationTimeUnknown,
        ),
    ] {
        let fixture = Fixture::new();
        let mut raw = serde_json::to_value(observation()).unwrap();
        raw["observed_at"] = timestamp.into();
        fixture.raw(raw);
        let result = fixture.preview();
        assert_eq!(result.reasons, [expected]);
        assert!(!result.needs_fresh_native_preparation);
    }
    let fixture = Fixture::new();
    let result = monitor_trigger::preview(
        &fixture.config,
        "selected",
        &fixture.preferences,
        NOW / 1000 * 1000 + 5000,
    )
    .unwrap();
    assert!(
        result.needs_fresh_native_preparation,
        "exact five-second conservative boundary is accepted"
    );
}

#[test]
fn current_configuration_fingerprint_and_enabled_state_bind_the_snapshot() {
    let mut fixture = Fixture::new();
    fixture.config.watches[0].interval_secs += 1;
    assert!(
        fixture.preview().needs_fresh_native_preparation,
        "schedule interval does not alter Store fingerprint"
    );
    fixture.config.watches[0].min_quantity = Some(3);
    assert_eq!(
        fixture.preview().reasons,
        [TriggerReason::WatchConfigurationChanged]
    );
    fixture.config.watches[0].enabled = false;
    assert_eq!(fixture.preview().reasons, [TriggerReason::WatchDisabled]);
    let fixture = Fixture::new();
    Connection::open(&fixture.config.database)
        .unwrap()
        .execute("UPDATE watches SET enabled=0", [])
        .unwrap();
    assert_eq!(fixture.preview().reasons, [TriggerReason::WatchDisabled]);
}

#[test]
fn event_alias_override_controls_quantity_and_normalized_label_filters() {
    let mut fixture = Fixture::new();
    let mut observed = observation();
    observed.offers[0].quantity = Some(1);
    observed.offers[0].ticket_type = Some("STANDARD   TICKET".into());
    fixture.replace(observed);
    assert!(!fixture.preview().needs_fresh_native_preparation);
    let override_preferences = CartPreferences {
        quantity: 1,
        allowed_ticket_types: vec!["standard ticket".into()],
        ..CartPreferences::default()
    };
    fixture.preferences = preferences(PreferencesDocument {
        event_overrides: vec![EventOverride {
            event_url: "https://ticketmaster.cz/event/123456789?language=en-us".into(),
            preferences: override_preferences,
        }],
        ..PreferencesDocument::default()
    });
    fixture.preferences.revision = 4;
    let result = fixture.preview();
    assert!(result.needs_fresh_native_preparation);
    assert_eq!(
        result.effective_preferences.source,
        PreferenceSource::EventOverride
    );
    assert_eq!(result.effective_preferences.preferences.quantity, 1);
    assert_eq!(result.preference_revision, 4);
    assert_eq!(result.offers[0].checks.ticket_type, Check::Matches);
}

#[test]
fn quantities_are_never_combined_and_selectable_capacity_is_not_stock() {
    for kind in [
        QuantityKind::Listing,
        QuantityKind::Remaining,
        QuantityKind::Selectable,
        QuantityKind::Unknown,
    ] {
        let fixture = Fixture::new();
        let mut observed = observation();
        observed.offers[0].quantity = Some(
            if matches!(kind, QuantityKind::Selectable | QuantityKind::Unknown) {
                20
            } else {
                1
            },
        );
        observed.offers[0].quantity_kind = kind;
        let mut second = observed.offers[0].clone();
        second.key = "second-private-offer".into();
        observed.offers.push(second);
        fixture.replace(observed);
        let result = fixture.preview();
        assert!(!result.needs_fresh_native_preparation);
        assert_eq!(result.examined_offer_rows, 2);
    }
    let fixture = Fixture::new();
    let mut observed = observation();
    observed.offers[0].quantity_kind = QuantityKind::Remaining;
    observed.offers[0].price_minor = None;
    observed.offers[0].price_text = Some("100–200 CZK".into());
    fixture.replace(observed);
    let result = fixture.preview();
    assert!(result.needs_fresh_native_preparation);
    assert_eq!(result.offers[0].displayed_group_subtotal_minor, None);
    assert!(
        result.offers[0]
            .missing_evidence
            .contains(&MissingEvidence::PriceBasis)
    );
}

#[test]
fn filter_mismatches_exclude_rows_while_missing_fields_remain_three_way_unknown() {
    let mut fixture = Fixture::new();
    fixture.with_defaults(CartPreferences {
        allowed_ticket_types: vec!["Resale".into()],
        allowed_sections: vec!["102".into()],
        allowed_rows: vec!["5".into()],
        ..CartPreferences::default()
    });
    let result = fixture.preview();
    assert!(!result.needs_fresh_native_preparation);
    assert_eq!(result.offers[0].checks.ticket_type, Check::Mismatch);
    assert_eq!(result.offers[0].checks.section, Check::Mismatch);
    assert_eq!(result.offers[0].checks.row, Check::Mismatch);
    let mut observed = observation();
    observed.offers[0].ticket_type = None;
    observed.offers[0].section = None;
    observed.offers[0].row = None;
    observed.offers[0].currency = None;
    fixture.replace(observed);
    let result = fixture.preview();
    assert!(result.needs_fresh_native_preparation);
    assert_eq!(result.offers[0].checks.ticket_type, Check::Unknown);
    assert_eq!(result.offers[0].checks.section, Check::Unknown);
    assert_eq!(result.offers[0].checks.row, Check::Unknown);
    assert_eq!(result.offers[0].checks.currency, Check::Unknown);
    assert_eq!(result.offers[0].displayed_group_subtotal_minor, None);
}

#[test]
fn displayed_budget_keeps_fee_uncertainty_and_never_resizes_a_listing_total() {
    let cases = [
        (
            PriceScope::PerTicket,
            QuantityKind::Listing,
            2,
            FeeScope::Included,
            1000,
            Some(2000),
            Check::Matches,
            true,
        ),
        (
            PriceScope::PerTicket,
            QuantityKind::Listing,
            2,
            FeeScope::Unknown,
            1000,
            Some(2000),
            Check::Unknown,
            true,
        ),
        (
            PriceScope::PerTicket,
            QuantityKind::Listing,
            2,
            FeeScope::Excluded,
            1000,
            Some(2000),
            Check::Unknown,
            true,
        ),
        (
            PriceScope::PerTicket,
            QuantityKind::Listing,
            2,
            FeeScope::Unknown,
            2000,
            Some(4000),
            Check::Mismatch,
            false,
        ),
        (
            PriceScope::Total,
            QuantityKind::Listing,
            4,
            FeeScope::Included,
            4000,
            None,
            Check::Unknown,
            true,
        ),
        (
            PriceScope::Total,
            QuantityKind::Listing,
            2,
            FeeScope::Included,
            2000,
            Some(2000),
            Check::Matches,
            true,
        ),
        (
            PriceScope::Total,
            QuantityKind::Remaining,
            2,
            FeeScope::Included,
            2000,
            None,
            Check::Unknown,
            true,
        ),
        (
            PriceScope::Unknown,
            QuantityKind::Listing,
            2,
            FeeScope::Included,
            1000,
            None,
            Check::Unknown,
            true,
        ),
    ];
    for (scope, kind, quantity, fees, price, subtotal, check, candidate) in cases {
        let mut fixture = Fixture::new();
        fixture.with_defaults(CartPreferences {
            max_total_minor: Some(3000),
            ..CartPreferences::default()
        });
        let mut observed = observation();
        let offer = &mut observed.offers[0];
        offer.price_scope = scope;
        offer.quantity_kind = kind;
        offer.quantity = Some(quantity);
        offer.fees = fees;
        offer.price_minor = Some(price);
        fixture.replace(observed);
        let result = fixture.preview();
        assert_eq!(result.needs_fresh_native_preparation, candidate);
        assert_eq!(result.offers[0].displayed_group_subtotal_minor, subtotal);
        assert_eq!(result.offers[0].checks.displayed_budget, check);
        assert!(
            result.offers[0]
                .missing_evidence
                .contains(&MissingEvidence::CheckoutFees)
        );
        assert!(
            result.offers[0]
                .missing_evidence
                .contains(&MissingEvidence::CompleteGroupTotal)
        );
        if scope == PriceScope::Total && subtotal.is_some() {
            assert_eq!(
                result.offers[0].subtotal_basis,
                Some(SubtotalBasis::EqualSizeListingTotal)
            );
        }
    }
}

#[test]
fn unsupported_market_currency_and_non_offer_positive_evidence_do_not_trigger() {
    let mut fixture = Fixture::new();
    fixture.with_defaults(CartPreferences {
        currency: "EUR".into(),
        ..CartPreferences::default()
    });
    assert_eq!(
        fixture.preview().reasons,
        [TriggerReason::UnsupportedCurrency]
    );
    let mut fixture = Fixture::new();
    fixture.config.watches[0].url = "https://www.ticketmaster.nl/event/example/123456789".into();
    Store::open(&fixture.config.database)
        .unwrap()
        .reconcile(&fixture.config.watches)
        .unwrap();
    fixture.replace(observation());
    assert_eq!(
        fixture.preview().reasons,
        [TriggerReason::UnsupportedMarket]
    );
    for evidence in [Evidence::Metadata, Evidence::ExplicitStatus, Evidence::None] {
        let fixture = Fixture::new();
        let mut observed = observation();
        observed.evidence = evidence;
        fixture.replace(observed);
        assert!(!fixture.preview().needs_fresh_native_preparation);
    }
}

#[test]
fn truncation_reports_only_examined_rows_and_never_claims_cheapest_or_full_coverage() {
    let fixture = Fixture::new();
    let mut observed = observation();
    observed.offers = (0..130)
        .map(|index| Offer {
            key: format!("offer-{index}"),
            quantity: Some(if index == 129 { 2 } else { 1 }),
            ..offer()
        })
        .collect();
    fixture.replace(observed);
    let result = fixture.preview();
    assert_eq!(result.observed_offer_rows, 130);
    assert_eq!(result.examined_offer_rows, 128);
    assert!(result.offers_truncated);
    assert!(!result.needs_fresh_native_preparation);
    assert!(!result.source_snapshot_complete);
    assert!(!result.cheapest_verified);
    assert!(
        result
            .reasons
            .contains(&TriggerReason::OfferDiagnosticsTruncated)
    );
    assert!(
        result
            .reasons
            .contains(&TriggerReason::NoEligibleExaminedOffer)
    );
}

#[test]
fn generated_watches_require_current_catalog_and_respect_disabled_manual_overrides() {
    let mut fixture = Fixture::new();
    let mut generated = selected_watch();
    generated.id = "auto-example".into();
    fixture.config.watches.clear();
    fixture.config.profiles = vec![WebsiteProfile {
        id: "regional".into(),
        ..WebsiteProfile::default()
    }];
    fixture.config.subscriptions = vec![Subscription {
        id: "selected-artists".into(),
        profile: "regional".into(),
        ..Subscription::default()
    }];
    let mut observed = observation();
    observed.watch_id.clone_from(&generated.id);
    let mut snapshot = Snapshot {
        fingerprint: catalog::fingerprint(&fixture.config).unwrap(),
        selections: vec![Selection {
            id: "selected-artists".into(),
            watches: vec![generated.clone()],
            ..Selection::default()
        }],
        ..Snapshot::default()
    };
    {
        let mut store = Store::open(&fixture.config.database).unwrap();
        store.apply(&generated, &observed).unwrap();
        snapshot.selections[0].watches[0].name = "Updated catalog title".into();
        store
            .catalog_save(&serde_json::to_value(&snapshot).unwrap())
            .unwrap();
    }
    assert!(
        monitor_trigger::preview(&fixture.config, &generated.id, &fixture.preferences, NOW)
            .unwrap()
            .needs_fresh_native_preparation
    );
    fixture.config.watches = vec![Watch {
        enabled: false,
        ..selected_watch()
    }];
    assert!(
        monitor_trigger::preview(&fixture.config, &generated.id, &fixture.preferences, NOW)
            .is_err()
    );
    fixture.config.watches.clear();
    fixture.config.subscriptions[0].artist = "Changed artist selection".into();
    assert!(
        monitor_trigger::preview(&fixture.config, &generated.id, &fixture.preferences, NOW)
            .is_err()
    );
}

#[test]
fn invalid_latest_json_or_watch_binding_cannot_reuse_an_old_positive() {
    let fixture = Fixture::new();
    let mut raw = serde_json::to_value(observation()).unwrap();
    raw["watch_id"] = "other".into();
    fixture.raw(raw);
    assert_eq!(
        fixture.preview().reasons,
        [TriggerReason::ObservationBindingMismatch]
    );
    let mut raw = serde_json::to_value(observation()).unwrap();
    raw["new_unreviewed_field"] = true.into();
    fixture.raw(raw);
    assert_eq!(
        fixture.preview().reasons,
        [TriggerReason::InvalidObservation]
    );
    let connection = Connection::open(&fixture.config.database).unwrap();
    for raw in [
        "{\"watch_id\":\"selected\",\"watch_id\":\"other\"}".to_string(),
        "x".repeat(1024 * 1024 + 1),
    ] {
        connection
            .execute("UPDATE watch_observations SET observation_json=?1", [raw])
            .unwrap();
        assert_eq!(
            fixture.preview().reasons,
            [TriggerReason::InvalidObservation]
        );
    }
}

#[test]
fn exact_integer_limits_cover_clocks_prices_and_computed_subtotals() {
    let fixture = Fixture::new();
    assert!(
        monitor_trigger::preview(
            &fixture.config,
            "selected",
            &fixture.preferences,
            ticket_watch::cart::MAX_EXACT_INTEGER as i64 + 1
        )
        .is_err()
    );
    let mut raw = serde_json::to_value(observation()).unwrap();
    raw["offers"][0]["price_minor"] = (ticket_watch::cart::MAX_EXACT_INTEGER + 1).into();
    fixture.raw(raw);
    assert_eq!(
        fixture.preview().reasons,
        [TriggerReason::InvalidObservation]
    );
    let mut fixture = Fixture::new();
    fixture.with_defaults(CartPreferences {
        max_total_minor: Some(5000),
        ..CartPreferences::default()
    });
    let mut observed = observation();
    observed.offers[0].price_minor = Some(ticket_watch::cart::MAX_EXACT_INTEGER as i64);
    fixture.replace(observed);
    let result = fixture.preview();
    assert!(!result.needs_fresh_native_preparation);
    assert_eq!(result.offers[0].displayed_group_subtotal_minor, None);
    assert_eq!(result.offers[0].checks.displayed_budget, Check::Mismatch);
    assert!(
        result.offers[0]
            .reasons
            .contains(&OfferReason::DisplayedSubtotalOutOfRange)
    );
}

#[test]
fn missing_database_and_invalid_preferences_are_never_created_or_migrated() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        database: directory.path().join("missing/monitor.sqlite"),
        watches: vec![selected_watch()],
        ..Config::default()
    };
    let snapshot = preferences(PreferencesDocument::default());
    assert!(monitor_trigger::preview(&config, "selected", &snapshot, NOW).is_err());
    assert!(!directory.path().join("missing").exists());
    let fixture = Fixture::new();
    for invalid in [
        PreferenceSnapshot {
            revision: 0,
            ..fixture.preferences.clone()
        },
        PreferenceSnapshot {
            document_hash: "0".repeat(64),
            ..fixture.preferences.clone()
        },
        PreferenceSnapshot {
            updated_at_ms: NOW + 1,
            ..fixture.preferences.clone()
        },
    ] {
        assert!(monitor_trigger::preview(&fixture.config, "selected", &invalid, NOW).is_err());
    }
    let before = std::fs::read(&fixture.config.database).unwrap();
    Connection::open(&fixture.config.database)
        .unwrap()
        .execute("DELETE FROM watch_observations", [])
        .unwrap();
    assert_eq!(
        fixture.preview().reasons,
        [TriggerReason::NoLatestObservation]
    );
    assert!(!before.is_empty());
    let connection = Connection::open(&fixture.config.database).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM outbox", params![], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
