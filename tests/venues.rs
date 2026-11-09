use serde_json::json;
use std::path::Path;
use ticket_watch::{
    store::Store,
    venues::{VenueLayout, VenuePreset, from_discovery_event},
};

fn venue() -> VenuePreset {
    VenuePreset {
        site_host: "www.ticketmaster.cz".into(),
        name: "O2 arena".into(),
        city: "Praha 9".into(),
        country: "CZ".into(),
        source_event_urls: vec!["https://www.ticketmaster.cz/event/bocelli/493115848".into()],
        ..VenuePreset::default()
    }
}

fn layout(id: &str, sections: &[&str]) -> VenueLayout {
    VenueLayout {
        id: id.into(),
        name: id.replace('-', " "),
        adapter: Some("ticketmaster-regional-v1".into()),
        sections: sections.iter().map(|s| s.to_string()).collect(),
        source_event_urls: venue().source_event_urls,
        verified_at: Some(1_793_995_200),
    }
}

#[test]
fn same_venue_new_event_reuses_reference_without_inventory() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let first = db.venue_upsert(&venue()).unwrap();
    let mut second = venue();
    second.name = "  O2   ARENA  ".into();
    second.source_event_urls =
        vec!["https://ticketmaster.cz/event/different-artist/23456789".into()];
    let second = db.venue_upsert(&second).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.source_event_urls.len(), 2);
    assert!(second.layouts.is_empty());
    assert_eq!(db.venue_list().unwrap().len(), 1);
    let data = serde_json::to_value(&second).unwrap();
    for field in [
        "price",
        "prices",
        "availability",
        "quantity",
        "capacity",
        "offers",
        "ticket_types",
    ] {
        assert!(data.get(field).is_none());
    }
}

#[test]
fn venue_source_history_stays_bounded_without_stopping_new_events() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let initial = db.venue_upsert(&venue()).unwrap();
    for number in 10000..10110 {
        let mut next = venue();
        next.source_event_urls = vec![format!("https://ticketmaster.cz/event/show/{number}")];
        assert_eq!(db.venue_upsert(&next).unwrap().id, initial.id);
    }
    let saved = db.venue_get(&initial.id).unwrap().unwrap();
    assert_eq!(saved.source_event_urls.len(), 100);
    assert!(saved.source_event_urls.last().unwrap().ends_with("/10109"));
    assert!(saved.layouts.is_empty());
    assert_eq!(db.venue_list().unwrap().len(), 1);
}

#[test]
fn source_aliases_and_renamed_event_slugs_remain_one_reference() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let mut preset = venue();
    preset
        .source_event_urls
        .push("https://ticketmaster.cz/event/new-title/493115848?language=en".into());
    let saved = db.venue_upsert(&preset).unwrap();
    assert_eq!(saved.source_event_urls.len(), 1);
    preset.source_event_urls =
        vec!["https://www.ticketmaster.cz/event/another-title/493115848".into()];
    assert_eq!(db.venue_upsert(&preset).unwrap().source_event_urls.len(), 1);
}

#[test]
fn provider_promotion_keeps_id_and_named_layout_variants() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let mut initial = venue();
    initial.layouts = vec![
        layout("concert-end-stage", &["101", "102"]),
        layout("basketball", &["101", "COURT"]),
    ];
    let first = db.venue_upsert(&initial).unwrap();
    let mut catalog = venue();
    catalog.provider_venue_id = Some("KovZ1234".into());
    let promoted = db.venue_upsert(&catalog).unwrap();
    assert_eq!(first.id, promoted.id);
    assert_eq!(promoted.layouts, first.layouts);
    let mut renamed = catalog;
    renamed.name = "Renamed arena".into();
    let renamed = db.venue_upsert(&renamed).unwrap();
    assert_eq!(renamed.id, first.id);
    assert_eq!(renamed.layouts.len(), 2);
    assert_eq!(db.venue_get(&first.id).unwrap(), Some(renamed.clone()));
    let exported = serde_json::to_string(&renamed).unwrap();
    let imported: VenuePreset = serde_json::from_str(&exported).unwrap();
    let mut fresh = Store::open(Path::new(":memory:")).unwrap();
    assert_eq!(fresh.venue_upsert(&imported).unwrap(), renamed);
}

#[test]
fn old_name_after_promotion_and_rename_cannot_overwrite_the_venue() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let mut original = venue();
    original.layouts = vec![layout("concert", &["101"])];
    let saved = db.venue_upsert(&original).unwrap();
    let mut promoted = venue();
    promoted.provider_venue_id = Some("official-arena-1".into());
    db.venue_upsert(&promoted).unwrap();
    promoted.name = "Renamed Arena".into();
    let renamed = db.venue_upsert(&promoted).unwrap();
    assert!(db.venue_upsert(&venue()).is_err());
    assert_eq!(db.venue_get(&saved.id).unwrap(), Some(renamed));
    assert_eq!(db.venue_list().unwrap().len(), 1);
}

#[test]
fn same_name_across_cities_and_sites_is_not_merged() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let first = db.venue_upsert(&venue()).unwrap();
    let mut other_city = venue();
    other_city.city = "Brno".into();
    let second = db.venue_upsert(&other_city).unwrap();
    let mut other_site = venue();
    other_site.site_host = "ticketmaster.de".into();
    other_site.source_event_urls.clear();
    let third = db.venue_upsert(&other_site).unwrap();
    assert_ne!(first.id, second.id);
    assert_ne!(first.id, third.id);
    assert_eq!(db.venue_list().unwrap().len(), 3);
}

#[test]
fn conflicting_provider_ids_stay_separate_and_name_lookup_is_ambiguous() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let mut a = venue();
    a.provider_venue_id = Some("venue-a".into());
    let a_saved = db.venue_upsert(&a).unwrap();
    let mut b = venue();
    b.provider_venue_id = Some("venue-b".into());
    let b_saved = db.venue_upsert(&b).unwrap();
    assert_ne!(a_saved.id, b_saved.id);
    assert!(db.venue_lookup(&venue()).is_err());
    assert_eq!(db.venue_lookup(&a).unwrap(), Some(a_saved.clone()));
    b.id = a_saved.id;
    assert!(db.venue_upsert(&b).is_err());
    assert_eq!(db.venue_list().unwrap().len(), 2);
}

#[test]
fn explicit_layout_update_changes_only_one_variant_and_drafts_stay_drafts() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let mut first = venue();
    let mut draft = layout("draft", &["A"]);
    draft.verified_at = None;
    first.layouts = vec![draft.clone(), layout("seated", &["101"])];
    let saved = db.venue_upsert(&first).unwrap();
    let mut changed = saved.clone();
    changed.layouts = vec![layout("seated", &["101", "102"])];
    let result = db.venue_upsert(&changed).unwrap();
    assert_eq!(result.layouts[0], saved.layouts[0]);
    assert_eq!(result.layouts[0].verified_at, None);
    assert_eq!(result.layouts[1].sections, vec!["101", "102"]);
    assert_eq!(db.venue_lookup(&venue()).unwrap().unwrap().id, saved.id);
}

#[test]
fn inventory_and_unknown_fields_are_rejected_at_every_level() {
    let mut value = serde_json::to_value(venue()).unwrap();
    value["quantity"] = json!(100);
    assert!(serde_json::from_value::<VenuePreset>(value).is_err());
    let mut value = serde_json::to_value(venue()).unwrap();
    value["layouts"] = json!([{"id":"end-stage", "name":"End stage", "price":99}]);
    assert!(serde_json::from_value::<VenuePreset>(value).is_err());
}

#[test]
fn rejects_unscoped_identity_private_links_and_unproven_review() {
    for (field, value) in [
        ("site_host", "secure.ticketmaster.cz"),
        ("country", "Czechia"),
        ("city", ""),
        ("provider_venue_id", "a/b"),
    ] {
        let mut preset = serde_json::to_value(venue()).unwrap();
        preset[field] = json!(value);
        assert!(
            serde_json::from_value::<VenuePreset>(preset)
                .unwrap()
                .normalized()
                .is_err()
        );
    }
    for url in [
        "https://ticketmaster.cz/event/12345?token=secret",
        "https://ticketmaster.cz/artist/12345",
        "https://ticketmaster.pl/event/12345",
    ] {
        let mut preset = venue();
        preset.source_event_urls = vec![url.into()];
        assert!(preset.normalized().is_err());
    }
    let mut preset = venue();
    let mut invalid = layout("concert", &["101"]);
    invalid.source_event_urls.clear();
    preset.layouts.push(invalid);
    assert!(preset.normalized().is_err());
}

#[test]
fn metadata_registration_imports_only_static_fields_and_preserves_layouts() {
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let mut preset = venue();
    preset.layouts.push(layout("end-stage", &["413"]));
    let initial = db.venue_upsert(&preset).unwrap();
    let event = json!({"url":"https://ticketmaster.cz/event/someone-else/87654321", "price_ranges":[{"min":100}], "availability":"available", "venues":[{"id":"tm123", "name":"O2 arena", "city":"Praha 9", "country":{"countryCode":"CZ"}, "capacity":20000}]});
    let normalized = from_discovery_event(&event).unwrap();
    assert!(normalized[0].layouts.is_empty());
    let updated = db.venue_register_event(&event).unwrap();
    assert_eq!(initial.id, updated[0].id);
    assert_eq!(initial.layouts, updated[0].layouts);
    assert_eq!(updated[0].provider_venue_id.as_deref(), Some("tm123"));
}

#[test]
fn imports_roll_back_and_presets_survive_reopening_existing_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ticket.sqlite");
    // Mimic an old database without a venue table; opening is an additive migration.
    let old = rusqlite::Connection::open(&path).unwrap();
    old.execute_batch("CREATE TABLE preserved_old_data(value TEXT); INSERT INTO preserved_old_data VALUES ('still here');").unwrap();
    drop(old);
    let mut db = Store::open(&path).unwrap();
    let first = db.venue_upsert(&venue()).unwrap();
    let mut valid_new = venue();
    valid_new.city = "Another city".into();
    let mut conflict = first.clone();
    conflict.city = "Wrong city".into();
    assert!(db.venue_import(&[valid_new, conflict]).is_err());
    assert_eq!(db.venue_list().unwrap().len(), 1);
    drop(db);
    let db = Store::open(&path).unwrap();
    assert_eq!(db.venue_get(&first.id).unwrap(), Some(first));
    let check = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        check
            .query_row("SELECT value FROM preserved_old_data", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "still here"
    );
}

#[test]
fn observed_venue_seed_is_valid_reference_metadata_only() {
    let presets: Vec<VenuePreset> =
        serde_json::from_str(include_str!("../examples/venue-presets.json")).unwrap();
    assert_eq!(presets.len(), 21);
    let mut db = Store::open(Path::new(":memory:")).unwrap();
    let imported = db.venue_import(&presets).unwrap();
    assert_eq!(imported.len(), 21);
    assert!(
        imported
            .iter()
            .all(|p| p.layouts.is_empty() && p.provider_venue_id.is_none())
    );
    assert_eq!(db.venue_list().unwrap().len(), 21);
}
