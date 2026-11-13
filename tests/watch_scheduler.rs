use serde_json::json;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use ticket_watch::{
    catalog::{Selection, Snapshot},
    config::{Config, Watch},
    model::{Availability, Evidence, Observation, Offer},
    watch_scheduler::{DueWatch, Priority, Scheduler, TimingSample},
};

const NOW: i64 = 1_800_000_000;

fn watch(id: &str) -> Watch {
    Watch {
        id: id.into(),
        url: format!("https://www.ticketmaster.nl/event/{id}"),
        ..Watch::default()
    }
}

fn setup(
    watches: &[Watch],
    manual: Vec<Watch>,
    metadata: Vec<serde_json::Value>,
) -> (Config, Snapshot, Scheduler) {
    let config = Config {
        watches: manual,
        ..Config::default()
    };
    let snapshot = Snapshot {
        selections: vec![Selection {
            metadata,
            ..Selection::default()
        }],
        ..Snapshot::default()
    };
    let scheduler = Scheduler::new(&config, &snapshot, watches, &[]).unwrap();
    (config, snapshot, scheduler)
}

fn candidate(index: usize, watch: &Watch, due: Instant) -> DueWatch {
    DueWatch {
        index,
        watch_id: watch.id.clone(),
        host: url::Url::parse(&watch.url)
            .unwrap()
            .host_str()
            .unwrap()
            .into(),
        due,
    }
}

fn sale(watch: &Watch, offset: i64) -> serde_json::Value {
    json!({"url":watch.url, "public_sale":{"startDateTime":chrono::DateTime::from_timestamp(NOW+offset,0).unwrap().to_rfc3339()}})
}

fn observation(id: &str, at: i64, price: i64) -> Observation {
    Observation {
        watch_id: id.into(),
        observed_at: at,
        availability: Availability::Available,
        evidence: Evidence::Offers,
        complete: true,
        offers: vec![Offer {
            key: "seat-a".into(),
            price_minor: Some(price),
            ..Offer::default()
        }],
        ticket_categories: vec![],
        adapter: None,
        detail: "synthetic".into(),
    }
}

#[test]
fn imminent_sale_wins_without_changing_caller_due_times() {
    let watches = vec![watch("100"), watch("200")];
    let (_, _, scheduler) = setup(
        &watches,
        vec![watches[0].clone()],
        vec![sale(&watches[1], 60)],
    );
    let instant = Instant::now();
    let candidates = vec![
        candidate(0, &watches[0], instant - Duration::from_secs(600)),
        candidate(1, &watches[1], instant),
    ];
    let choice = scheduler.choose(&candidates, NOW).unwrap();
    assert_eq!(choice.index, 1);
    assert_eq!(choice.priority, Priority::SaleWindow);
    assert!(!choice.fairness_slot);
    assert_eq!(candidates[0].due, instant - Duration::from_secs(600));
    assert_eq!(candidates[1].due, instant);
}

#[test]
fn oldest_due_is_served_every_fourth_dispatch_despite_continuous_sales() {
    let watches = vec![watch("100"), watch("200")];
    let (_, _, mut scheduler) = setup(&watches, vec![], vec![sale(&watches[1], 0)]);
    let instant = Instant::now();
    let candidates = vec![
        candidate(0, &watches[0], instant - Duration::from_secs(600)),
        candidate(1, &watches[1], instant),
    ];
    for _ in 0..3 {
        assert_eq!(scheduler.choose(&candidates, NOW).unwrap().index, 1);
        scheduler.dispatched("www.ticketmaster.nl");
    }
    let fair = scheduler.choose(&candidates, NOW).unwrap();
    assert_eq!(fair.index, 0);
    assert!(fair.fairness_slot);
    scheduler.dispatched("www.ticketmaster.nl");
    assert_eq!(scheduler.choose(&candidates, NOW).unwrap().index, 1);
}

#[test]
fn another_host_cannot_consume_this_hosts_fairness_slot() {
    let watches = vec![watch("100"), watch("200")];
    let (_, _, mut scheduler) = setup(&watches, vec![], vec![sale(&watches[1], 0)]);
    let instant = Instant::now();
    let candidates = vec![
        candidate(0, &watches[0], instant - Duration::from_secs(600)),
        candidate(1, &watches[1], instant),
    ];
    for _ in 0..3 {
        scheduler.dispatched("www.ticketmaster.nl");
    }
    scheduler.dispatched("www.ticketmaster.cz"); // Takes global slot, not NL slot.
    let fair = scheduler.choose(&candidates, NOW).unwrap();
    assert_eq!(fair.index, 0);
    assert!(fair.fairness_slot);
}

#[test]
fn excluded_candidates_are_never_synthesized_and_ties_are_stable() {
    let watches = vec![watch("100"), watch("200"), watch("300")];
    let (_, _, scheduler) = setup(&watches, vec![], vec![sale(&watches[0], 0)]);
    // Caller omitted sale event because its host/cooldown/busy gate was closed.
    let instant = Instant::now();
    let candidates = vec![
        candidate(2, &watches[2], instant),
        candidate(1, &watches[1], instant),
    ];
    assert_eq!(scheduler.choose(&candidates, NOW).unwrap().index, 1);
    assert!(scheduler.choose(&[], NOW).is_none());
}

#[test]
fn presales_require_parseable_explicit_times_and_priority_expires() {
    let watches = vec![watch("100"), watch("200"), watch("300")];
    let (_, _, scheduler) = setup(
        &watches,
        vec![],
        vec![
            json!({"url":watches[0].url,"public_sale":{"startDateTime":chrono::DateTime::from_timestamp(NOW,0).unwrap().to_rfc3339(),"startTBD":true}}),
            json!({"url":watches[1].url,"presales":[{"startDateTime":chrono::DateTime::from_timestamp(NOW,0).unwrap().to_rfc3339()}]}),
            json!({"url":watches[2].url,"presales":[{"startDateTime":"2027-01-15 10:00"}]}),
        ],
    );
    assert_eq!(scheduler.priority("100", NOW), Priority::Normal);
    assert_eq!(scheduler.priority("200", NOW), Priority::SaleWindow);
    assert_eq!(scheduler.priority("200", NOW - 901), Priority::Normal);
    assert_eq!(scheduler.priority("200", NOW + 1801), Priority::Normal);
    assert_eq!(scheduler.priority("300", NOW), Priority::Normal);
}

#[test]
fn recent_observed_change_survives_unknown_and_does_not_treat_first_read_as_change() {
    let watches = vec![watch("100")];
    let (_, _, mut scheduler) = setup(&watches, vec![], vec![]);
    scheduler.observed(&observation("100", NOW, 500)).unwrap();
    assert_eq!(scheduler.priority("100", NOW), Priority::Normal);
    scheduler
        .observed(&Observation::unknown("100", NOW + 1, "blocked layout"))
        .unwrap();
    scheduler
        .observed(&observation("100", NOW + 2, 600))
        .unwrap();
    assert_eq!(
        scheduler.priority("100", NOW + 2),
        Priority::RecentlyChanged
    );
    scheduler
        .observed(&Observation::unknown("100", NOW + 3, "timeout"))
        .unwrap();
    assert_eq!(
        scheduler.priority("100", NOW + 4),
        Priority::RecentlyChanged
    );
    assert_eq!(scheduler.priority("100", NOW + 603), Priority::Normal);
}

#[test]
fn offer_order_and_diagnostics_do_not_manufacture_changes() {
    let watches = vec![watch("100")];
    let (_, _, mut scheduler) = setup(&watches, vec![], vec![]);
    let mut first = observation("100", NOW, 500);
    first.offers.push(Offer {
        key: "seat-b".into(),
        price_minor: Some(600),
        ..Offer::default()
    });
    scheduler.observed(&first).unwrap();
    first.observed_at += 1;
    first.detail = "new request timing".into();
    first.offers.reverse();
    scheduler.observed(&first).unwrap();
    assert_eq!(scheduler.priority("100", NOW + 1), Priority::Normal);
}

#[test]
fn restart_seeds_health_and_comparison_but_does_not_invent_recent_change() {
    let watches = vec![watch("100")];
    let (config, snapshot, _) = setup(&watches, vec![], vec![]);
    let directory = tempfile::TempDir::new().unwrap();
    let mut store =
        ticket_watch::store::Store::open(&directory.path().join("state.sqlite")).unwrap();
    store.reconcile(&watches).unwrap();
    store
        .apply(&watches[0], &observation("100", NOW - 120, 500))
        .unwrap();
    let saved = store.scheduler_status(&["100".into()]).unwrap();
    assert!(saved[0].get("latest_observation").is_none());
    let mut scheduler = Scheduler::new(&config, &snapshot, &watches, &saved).unwrap();
    assert_eq!(scheduler.priority("100", NOW), Priority::Normal);
    let report = scheduler.report(NOW, Instant::now(), &[], &HashMap::new(), 0, 60);
    assert_eq!(report["never_observed"], 0);
    assert_eq!(report["observation_age_secs"]["p50"], 120);
    scheduler.observed(&observation("100", NOW, 600)).unwrap();
    assert_eq!(scheduler.priority("100", NOW), Priority::RecentlyChanged);
}

#[test]
fn reconcile_removes_retired_state_and_resets_edited_watch_baseline() {
    let mut watches = vec![watch("100"), watch("200")];
    let (config, snapshot, mut scheduler) = setup(&watches, vec![], vec![]);
    scheduler
        .observed(&observation("100", NOW - 1, 500))
        .unwrap();
    scheduler.observed(&observation("100", NOW, 600)).unwrap();
    assert_eq!(scheduler.priority("100", NOW), Priority::RecentlyChanged);
    watches.truncate(1);
    watches[0].max_price_minor = Some(1000);
    scheduler.reconcile(&config, &snapshot, &watches).unwrap();
    assert!(scheduler.host("200").is_none());
    assert_eq!(scheduler.priority("100", NOW), Priority::Normal);
    assert_eq!(
        scheduler.report(NOW, Instant::now(), &[], &HashMap::new(), 0, 60)["never_observed"],
        1
    );
}

#[test]
fn backlog_reports_host_gates_and_unknown_freshness_without_latency_claims() {
    let watches = vec![watch("100"), watch("200")];
    let (_, _, mut scheduler) = setup(&watches, vec![], vec![]);
    scheduler
        .observed(&Observation::unknown("100", NOW - 30, "timeout"))
        .unwrap();
    let instant = Instant::now();
    let due = vec![
        candidate(0, &watches[0], instant - Duration::from_secs(10)),
        candidate(1, &watches[1], instant),
    ];
    let gates = HashMap::from([(
        "www.ticketmaster.nl".into(),
        instant + Duration::from_secs(60),
    )]);
    let report = scheduler.report(NOW, instant, &due, &gates, 0, 60);
    assert_eq!(report["due_waiting"], 2);
    assert_eq!(report["due_host_gated"], 2);
    assert_eq!(report["oldest_due_wait_ms"], 10000);
    assert_eq!(report["latest_unknown"], 1);
    assert_eq!(report["never_observed"], 1);
    assert_eq!(report["per_host"][0]["single_sweep_spacing_floor_secs"], 60);
    assert_eq!(
        report["fetch_to_observation_ms"]["p50"],
        serde_json::Value::Null
    );
}

#[test]
fn timing_window_is_bounded_and_percentiles_use_observed_samples_only() {
    let (_, _, mut scheduler) = setup(&[], vec![], vec![]);
    for value in 0..1005 {
        scheduler.record_timing(TimingSample {
            fetch_to_observation_ms: value,
            transport_reported_ms: None,
            parse_ms: None,
            observation_to_recorded_ms: 10,
            durable_apply_ms: 5,
            fetch_to_recorded_ms: value + 10,
            due_lateness_ms: 0,
        });
    }
    let report = scheduler.report(NOW, Instant::now(), &[], &HashMap::new(), 0, 60);
    assert_eq!(report["fetch_to_observation_ms"]["samples"], 1000);
    assert_eq!(report["fetch_to_observation_ms"]["p50"], 504);
    assert_eq!(report["fetch_to_observation_ms"]["p95"], 954);
    assert_eq!(report["fetch_to_observation_ms"]["p99"], 994);
    assert_eq!(report["fetch_to_observation_ms"]["max"], 1004);
    assert_eq!(report["parse_ms"]["samples"], 0);
}

#[test]
fn invalid_saved_timestamps_do_not_appear_fresh_or_never_observed() {
    let watches = vec![watch("100"), watch("200"), watch("300")];
    let (config, snapshot, _) = setup(&watches, vec![], vec![]);
    let stored = vec![
        json!({"watch_id":"100","last_observed_at":NOW+60}),
        json!({"watch_id":"200","last_observed_at":-1}),
    ];
    let scheduler = Scheduler::new(&config, &snapshot, &watches, &stored).unwrap();
    let report = scheduler.report(NOW, Instant::now(), &[], &HashMap::new(), 0, 60);
    assert_eq!(report["clock_invalid"], 2);
    assert_eq!(report["never_observed"], 1);
    assert_eq!(report["observation_age_secs"]["samples"], 0);
    assert_eq!(
        report["observation_age_secs"]["p50"],
        serde_json::Value::Null
    );
}

#[test]
fn scheduler_does_not_require_catalog_event_identity_for_public_artist_urls() {
    let mut config: Config = toml::from_str(include_str!("../examples/offline-demo.toml")).unwrap();
    config.watches[0].url = "https://www.ticketmaster.nl/artist/synthetic-artist".into();
    ticket_watch::config::validate_public_url(&config.watches[0].url).unwrap();
    assert!(ticket_watch::hybrid::canonical_event_url(&config.watches[0].url).is_err());

    let snapshot = Snapshot::default();
    let mut scheduler = Scheduler::new(&config, &snapshot, &config.watches, &[]).unwrap();
    let manual = &config.watches[0];
    assert_eq!(scheduler.host(&manual.id), Some("www.ticketmaster.nl"));
    assert_eq!(scheduler.priority(&manual.id, NOW), Priority::Manual);
    let choice = scheduler
        .choose(&[candidate(0, manual, Instant::now())], NOW)
        .unwrap();
    assert_eq!(choice.index, 0);
    assert_eq!(choice.priority, Priority::Manual);
    scheduler
        .reconcile(&config, &snapshot, &config.watches)
        .unwrap();

    // Optional schedule lookup must not weaken the required public URL gate.
    let mut invalid = manual.clone();
    invalid.url = "http://127.0.0.1/artist/synthetic-artist".into();
    assert!(Scheduler::new(&config, &snapshot, &[invalid], &[]).is_err());
}

#[test]
fn stale_observations_cannot_replace_health_or_invent_recent_changes() {
    let watches = vec![watch("100")];
    let (config, snapshot, mut scheduler) = setup(&watches, vec![], vec![]);
    scheduler.observed(&observation("100", NOW, 500)).unwrap();
    scheduler
        .observed(&observation("100", NOW - 1, 600))
        .unwrap();
    assert_eq!(scheduler.priority("100", NOW), Priority::Normal);
    let mut blocked = Observation::unknown("100", NOW - 2, "stale blocked response");
    blocked.availability = Availability::Blocked;
    scheduler.observed(&blocked).unwrap();
    let report = scheduler.report(NOW + 10, Instant::now(), &[], &HashMap::new(), 0, 60);
    assert_eq!(report["observation_age_secs"]["p50"], 10);
    assert_eq!(report["latest_blocked"], 0);

    let saved = [json!({"watch_id":"100", "last_observed_at":NOW+60,
        "last_availability":"blocked", "last_signature":null})];
    let mut skewed = Scheduler::new(&config, &snapshot, &watches, &saved).unwrap();
    skewed.observed(&observation("100", NOW, 500)).unwrap();
    let report = skewed.report(NOW, Instant::now(), &[], &HashMap::new(), 0, 60);
    assert_eq!(report["clock_invalid"], 1);
    assert_eq!(report["latest_blocked"], 1);
    assert_eq!(report["observation_age_secs"]["samples"], 0);
    assert_eq!(skewed.priority("100", NOW), Priority::Normal);
}

#[test]
fn generated_name_and_interval_edits_preserve_durable_future_baseline() {
    let mut watches = vec![watch("auto-synthetic")];
    watches[0].name = "Original catalog title".into();
    let id = watches[0].id.clone();
    let ids = vec![id.clone()];
    let (config, snapshot, _) = setup(&watches, vec![], vec![]);
    let directory = tempfile::TempDir::new().unwrap();
    let mut store =
        ticket_watch::store::Store::open(&directory.path().join("state.sqlite")).unwrap();
    store.reconcile(&watches).unwrap();
    store
        .apply(&watches[0], &observation(&id, NOW + 60, 500))
        .unwrap();
    let saved = store.scheduler_status(&ids).unwrap();
    let mut scheduler = Scheduler::new(&config, &snapshot, &watches, &saved).unwrap();

    for edit in ["interval", "generated name"] {
        if edit == "interval" {
            watches[0].interval_secs += 60;
        } else {
            watches[0].name = "Corrected catalog title and date".into();
        }
        store.reconcile(&watches).unwrap();
        scheduler.reconcile(&config, &snapshot, &watches).unwrap();
        assert_eq!(store.scheduler_status(&ids).unwrap(), saved, "{edit}");

        for stale in [
            observation(&id, NOW, 600),
            Observation::unknown(&id, NOW + 1, "synthetic stale read"),
        ] {
            assert_eq!(store.apply(&watches[0], &stale).unwrap(), 0, "{edit}");
            assert_eq!(store.scheduler_status(&ids).unwrap(), saved, "{edit}");
            scheduler.observed(&stale).unwrap();
            let report = scheduler.report(NOW + 1, Instant::now(), &[], &HashMap::new(), 0, 60);
            assert_eq!(report["clock_invalid"], 1, "{edit}");
            assert_eq!(report["never_observed"], 0, "{edit}");
            assert_eq!(report["latest_unknown"], 0, "{edit}");
            assert_eq!(report["observation_age_secs"]["samples"], 0, "{edit}");
            assert_eq!(scheduler.priority(&id, NOW + 1), Priority::Normal, "{edit}");
        }
    }

    // An accepted unchanged quote must compare with the original signature,
    // not the rejected price or a baseline erased by either metadata edit.
    let unchanged = observation(&id, NOW + 61, 500);
    store.apply(&watches[0], &unchanged).unwrap();
    scheduler.observed(&unchanged).unwrap();
    assert_eq!(scheduler.priority(&id, NOW + 61), Priority::Normal);
    let report = scheduler.report(NOW + 61, Instant::now(), &[], &HashMap::new(), 0, 60);
    assert_eq!(report["clock_invalid"], 0);
    assert_eq!(report["observation_age_secs"]["p50"], 0);

    let changed = observation(&id, NOW + 62, 600);
    store.apply(&watches[0], &changed).unwrap();
    scheduler.observed(&changed).unwrap();
    assert_eq!(scheduler.priority(&id, NOW + 62), Priority::RecentlyChanged);
}
