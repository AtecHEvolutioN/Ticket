use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
};

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use ticket_watch::{
    cart::MAX_EXACT_INTEGER,
    cart_preferences::{
        CartPreferences, EventOverride, MAX_INPUT_BYTES, PreferencesDocument, parse_document,
        resolve, validate_document,
    },
    preference_store::{PreferenceSnapshot, import, inspect},
};

const NOW: i64 = 1_800_000_000_000;
const ANDROID: &[u8] = include_bytes!("fixtures/android-ticket-preferences.json");

fn hash(document: &PreferencesDocument) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(document).unwrap())
    )
}

fn initialized() -> (TempDir, PathBuf, PreferenceSnapshot) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.sqlite");
    let snapshot = import(&path, &PreferencesDocument::default(), 0, NOW).unwrap();
    (directory, path, snapshot)
}

fn assert_refused_unchanged(path: &Path, expected_revision: u64, now_ms: i64) {
    let before = fs::read(path).unwrap();
    assert!(
        import(
            path,
            &PreferencesDocument::default(),
            expected_revision,
            now_ms
        )
        .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn android_fixture_survives_restart_export_and_import_without_changing_choices() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.sqlite");
    let document = parse_document(ANDROID).unwrap();
    let created = import(&path, &document, 0, NOW).unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(created.updated_at_ms, NOW);
    assert_eq!(created.document, document);
    assert_eq!(created.document_hash, hash(&document));
    // Every operation closes its connection; inspect models a restarted process.
    assert_eq!(inspect(&path).unwrap(), created);
    let exported = serde_json::to_vec_pretty(&inspect(&path).unwrap().document).unwrap();
    let roundtrip = parse_document(&exported).unwrap();
    assert_eq!(roundtrip, document);
    let restored = import(
        &directory.path().join("restored.sqlite"),
        &roundtrip,
        0,
        NOW,
    )
    .unwrap();
    assert_eq!(restored, created);
    assert_eq!(document.defaults.quantity, 4);
    let effective = resolve(
        &restored.document,
        "https://ticketmaster.cz/event/renamed/88770901",
    )
    .unwrap();
    assert_eq!(effective.preferences.quantity, 3);
    assert!(!effective.preferences.require_adjacent);
    assert_eq!(effective.preferences.max_total_minor, Some(825000));
}

#[test]
fn current_content_is_idempotent_but_stale_revision_never_succeeds() {
    let (_directory, path, first) = initialized();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(import(&path, &first.document, 1, NOW + 10).unwrap(), first);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_refused_unchanged(&path, 0, NOW + 10);
    assert_refused_unchanged(&path, 2, NOW + 10);

    let mut changed = first.document.clone();
    changed.defaults.quantity = 3;
    let second = import(&path, &changed, 1, NOW + 20).unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(second.updated_at_ms, NOW + 20);
    assert_ne!(second.document_hash, first.document_hash);
    let bytes = fs::read(&path).unwrap();
    assert!(import(&path, &changed, 1, NOW + 30).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(inspect(&path).unwrap(), second);
}

#[test]
fn concurrent_updates_allow_only_one_writer_with_the_expected_revision() {
    let (_directory, path, first) = initialized();
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [3, 4]
        .into_iter()
        .map(|quantity| {
            let path = path.clone();
            let barrier = barrier.clone();
            let mut document = first.document.clone();
            document.defaults.quantity = quantity;
            thread::spawn(move || {
                barrier.wait();
                import(&path, &document, 1, NOW + 1)
            })
        })
        .collect();
    let outcomes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(outcomes.iter().filter(|result| result.is_err()).count(), 1);
    let winner = outcomes.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(winner.revision, 2);
    assert_eq!(inspect(&path).unwrap(), winner);
}

#[test]
fn concurrent_creators_publish_one_complete_registry_without_clobbering() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new/private/preferences.sqlite");
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [3, 4]
        .into_iter()
        .map(|quantity| {
            let path = path.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut document = PreferencesDocument::default();
                document.defaults.quantity = quantity;
                barrier.wait();
                import(&path, &document, 0, NOW)
            })
        })
        .collect();
    let outcomes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let winner = outcomes.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(winner.revision, 1);
    assert_eq!(inspect(&path).unwrap(), winner);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn missing_inspection_and_nonzero_initial_revision_do_not_create_paths() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("missing");
    let path = parent.join("preferences.sqlite");
    assert!(inspect(&path).is_err());
    assert!(import(&path, &PreferencesDocument::default(), 1, NOW).is_err());
    assert!(!parent.exists());
    assert!(inspect(Path::new(":memory:")).is_err());
    assert!(
        import(
            Path::new(":memory:"),
            &PreferencesDocument::default(),
            0,
            NOW
        )
        .is_err()
    );
}

#[test]
fn empty_unrelated_and_unsupported_files_are_never_initialized_or_migrated() {
    let directory = tempfile::tempdir().unwrap();
    let empty = directory.path().join("empty.sqlite");
    fs::write(&empty, []).unwrap();
    assert!(inspect(&empty).is_err());
    assert_refused_unchanged(&empty, 0, NOW);

    let unrelated = directory.path().join("unrelated.sqlite");
    let connection = Connection::open(&unrelated).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('retained');",
        )
        .unwrap();
    drop(connection);
    let before = fs::read(&unrelated).unwrap();
    assert!(inspect(&unrelated).is_err());
    assert_eq!(fs::read(&unrelated).unwrap(), before);
    assert_refused_unchanged(&unrelated, 0, NOW);

    let (_registry_directory, own, _) = initialized();
    let connection = Connection::open(&own).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);
    let before = fs::read(&own).unwrap();
    assert!(inspect(&own).is_err());
    assert_eq!(fs::read(&own).unwrap(), before);
    assert_refused_unchanged(&own, 1, NOW + 1);
}

#[test]
fn invalid_input_and_stale_clock_are_rejected_before_any_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("missing");
    let path = parent.join("preferences.sqlite");
    let mut invalid = PreferencesDocument::default();
    invalid.defaults.quantity = 0;
    assert!(import(&path, &invalid, 0, NOW).is_err());
    for now in [-1, MAX_EXACT_INTEGER as i64 + 1] {
        assert!(import(&path, &PreferencesDocument::default(), 0, now).is_err());
    }
    assert!(
        import(
            &path,
            &PreferencesDocument::default(),
            MAX_EXACT_INTEGER + 1,
            NOW
        )
        .is_err()
    );
    assert!(!parent.exists());

    let (_existing_directory, existing, first) = initialized();
    assert!(import(&existing, &first.document, 1, NOW - 1).is_err());
    let mut changed = first.document.clone();
    changed.defaults.quantity = 3;
    assert!(import(&existing, &changed, 1, NOW - 1).is_err());
    assert_eq!(inspect(&existing).unwrap(), first);
}

#[test]
fn parser_rejects_ambiguous_json_and_observation_fields_before_registry_import() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("not-created/preferences.sqlite");
    let canonical = serde_json::to_string(&PreferencesDocument::default()).unwrap();
    let duplicate = canonical.replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    let mut stale_observation = serde_json::to_value(PreferencesDocument::default()).unwrap();
    stale_observation["observed_at"] = serde_json::json!(NOW - 10000);
    stale_observation["offers"] = serde_json::json!([]);
    for raw in [
        b"{invalid".to_vec(),
        duplicate.into_bytes(),
        serde_json::to_vec(&stale_observation).unwrap(),
    ] {
        let result = parse_document(&raw).and_then(|document| import(&path, &document, 0, NOW));
        assert!(result.is_err());
    }
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn valid_but_oversized_typed_documents_are_bounded_before_file_creation() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("missing");
    let path = parent.join("preferences.sqlite");
    let preferences = CartPreferences {
        allowed_rows: (0..100)
            .map(|i| format!("{i:03}-{}", "x".repeat(120)))
            .collect(),
        ..CartPreferences::default()
    };
    let document = PreferencesDocument {
        event_overrides: (0..30)
            .map(|i| EventOverride {
                event_url: format!(
                    "https://www.ticketmaster.cz/event/test/{}",
                    1_000_000_000 + i
                ),
                preferences: preferences.clone(),
            })
            .collect(),
        ..PreferencesDocument::default()
    };
    validate_document(&document).unwrap();
    assert!(serde_json::to_vec(&document).unwrap().len() > MAX_INPUT_BYTES);
    assert!(import(&path, &document, 0, NOW).is_err());
    assert!(!parent.exists());
    let (_existing_directory, existing, first) = initialized();
    assert!(import(&existing, &document, 1, NOW + 1).is_err());
    assert_eq!(inspect(&existing).unwrap(), first);
}

#[test]
fn hashes_use_the_parsed_struct_and_preserve_list_order_as_document_content() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.sqlite");
    let mut document = PreferencesDocument::default();
    document.defaults.allowed_rows = vec!["1".into(), "2".into()];
    let first = import(&path, &document, 0, NOW).unwrap();
    assert_eq!(first.document_hash, hash(&document));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE preference_registry SET document_json=?1",
            [serde_json::to_string_pretty(&document).unwrap()],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        inspect(&path).unwrap(),
        first,
        "Whitespace is not document content"
    );
    document.defaults.allowed_rows.reverse();
    let second = import(&path, &document, 1, NOW + 1).unwrap();
    assert_eq!(second.revision, 2);
    assert_ne!(second.document_hash, first.document_hash);
    assert_eq!(second.document_hash, hash(&document));
}

#[test]
fn corrupted_hash_or_ambiguous_stored_document_cannot_be_read_or_overwritten() {
    for corrupt_json in [false, true] {
        let (_directory, path, first) = initialized();
        let connection = Connection::open(&path).unwrap();
        if corrupt_json {
            let ambiguous = serde_json::to_string(&first.document).unwrap().replacen(
                "\"schema_version\":1",
                "\"schema_version\":1,\"schema_version\":1",
                1,
            );
            connection
                .execute(
                    "UPDATE preference_registry SET document_json=?1",
                    [ambiguous],
                )
                .unwrap();
        } else {
            connection
                .execute(
                    "UPDATE preference_registry SET document_hash=?1",
                    ["f".repeat(64)],
                )
                .unwrap();
        }
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(inspect(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_refused_unchanged(&path, 1, NOW + 1);
    }
}

#[test]
fn inspection_checks_row_bounds_and_shape_even_after_database_corruption() {
    for mutation in [
        "UPDATE preference_registry SET revision=0",
        "UPDATE preference_registry SET revision=9007199254740992",
        "UPDATE preference_registry SET updated_at_ms=-1",
        "UPDATE preference_registry SET updated_at_ms=9007199254740992",
        "UPDATE preference_registry SET document_json=printf('%262145s','x')",
        "DELETE FROM preference_registry",
        "INSERT INTO preference_registry SELECT 2,revision,document_hash,updated_at_ms,document_json FROM preference_registry",
        "CREATE TABLE unexpected(value TEXT)",
        "CREATE TABLE sqliteXextra(value TEXT)",
    ] {
        let (_directory, path, _) = initialized();
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        connection.execute_batch(mutation).unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(inspect(&path).is_err(), "{mutation}");
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_refused_unchanged(&path, 1, NOW + 1);
    }
}

#[test]
fn exact_integer_revision_limit_allows_idempotency_but_never_wraps() {
    let (_directory, path, first) = initialized();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE preference_registry SET revision=?1",
            [MAX_EXACT_INTEGER as i64],
        )
        .unwrap();
    drop(connection);
    let last = inspect(&path).unwrap();
    assert_eq!(last.revision, MAX_EXACT_INTEGER);
    assert_eq!(
        import(&path, &first.document, MAX_EXACT_INTEGER, NOW).unwrap(),
        last
    );
    let mut changed = first.document;
    changed.defaults.quantity = 3;
    assert!(import(&path, &changed, MAX_EXACT_INTEGER, NOW + 1).is_err());
    assert_eq!(inspect(&path).unwrap(), last);
}

#[cfg(unix)]
#[test]
fn new_paths_are_private_and_existing_permissions_are_preserved() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let existing = directory.path().join("existing");
    fs::create_dir(&existing).unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o750)).unwrap();
    let parent = existing.join("new/private");
    let path = parent.join("preferences.sqlite");
    let first = import(&path, &PreferencesDocument::default(), 0, NOW).unwrap();
    for folder in [&existing.join("new"), &parent] {
        assert_eq!(
            fs::metadata(folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
        0o750
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut document = first.document;
    document.defaults.quantity = 3;
    import(&path, &document, 1, NOW + 1).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::read_dir(&parent).unwrap().count(),
        1,
        "No stage or journal remains"
    );
}

#[cfg(unix)]
#[test]
fn symlink_destinations_are_never_followed() {
    use std::os::unix::fs::symlink;
    let (directory, real, first) = initialized();
    let linked = directory.path().join("linked.sqlite");
    symlink(&real, &linked).unwrap();
    assert!(inspect(&linked).is_err());
    assert!(import(&linked, &first.document, 1, NOW + 1).is_err());
    assert_eq!(inspect(&real).unwrap(), first);
    let absent = directory.path().join("absent.sqlite");
    let broken = directory.path().join("broken.sqlite");
    symlink(&absent, &broken).unwrap();
    assert!(import(&broken, &first.document, 0, NOW).is_err());
    assert!(!absent.exists());
}
