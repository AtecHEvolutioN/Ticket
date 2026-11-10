//! A separate, versioned registry for Android-compatible ticket preferences.
//! Imports update intent only: no watch, browser, cart journal or provider action
//! is changed. Hashes describe the serialized document, including list order.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    cart::MAX_EXACT_INTEGER,
    cart_preferences::{MAX_INPUT_BYTES, PreferencesDocument, parse_document, validate_document},
};

const APPLICATION_ID: i64 = 0x54505231; // TPR1; not an inventory or cart journal.
const SCHEMA_VERSION: i64 = 1;
const SCHEMA: &str = "CREATE TABLE preference_registry (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    document_hash TEXT NOT NULL CHECK(length(document_hash) = 64 AND document_hash NOT GLOB '*[^0-9a-f]*'),
    updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms BETWEEN 0 AND 9007199254740991),
    document_json TEXT NOT NULL CHECK(length(CAST(document_json AS BLOB)) BETWEEN 1 AND 262144)
) STRICT";
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PreferenceSnapshot {
    pub revision: u64,
    pub document_hash: String,
    pub updated_at_ms: i64,
    pub document: PreferencesDocument,
}

struct BoundedJson(Vec<u8>);

impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_INPUT_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("preferences document exceeds size limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn canonical_document(document: &PreferencesDocument) -> Result<(String, String)> {
    validate_document(document)?;
    let mut encoded = BoundedJson(Vec::new());
    serde_json::to_writer(&mut encoded, document)
        .context("serialize bounded preferences document")?;
    let hash = format!("{:x}", Sha256::digest(&encoded.0));
    let json = String::from_utf8(encoded.0).context("preferences serialization is not UTF-8")?;
    Ok((json, hash))
}

fn valid_time(now_ms: i64) -> Result<()> {
    ensure!(
        (0..=MAX_EXACT_INTEGER as i64).contains(&now_ms),
        "preference timestamp is outside the exact integer range"
    );
    Ok(())
}

fn validate_path(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty() && path != Path::new(":memory:") && path.file_name().is_some(),
        "preferences registry must be a durable file"
    );
    Ok(())
}

fn existing_file(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.file_type().is_file(),
                "preferences registry is not a regular file"
            );
            ensure!(metadata.len() > 0, "existing preferences registry is empty");
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect preferences registry path"),
    }
}

fn open(path: &Path, writable: bool) -> Result<Connection> {
    let access = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let connection = Connection::open_with_flags(
        path,
        access | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .context("open preferences registry")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    // This is a connection setting, not a database migration or persistent write.
    connection.pragma_update(None, "trusted_schema", false)?;
    Ok(connection)
}

fn validate_schema(connection: &Connection) -> Result<()> {
    let app: i64 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        app == APPLICATION_ID && version == SCHEMA_VERSION,
        "not a supported dedicated preferences registry"
    );
    let mut statement = connection.prepare(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY name LIMIT 2",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        objects.len() == 1
            && objects[0].0 == "table"
            && objects[0].1 == "preference_registry"
            && objects[0].2.as_deref() == Some(SCHEMA),
        "preferences registry schema is missing or altered"
    );
    Ok(())
}

fn snapshot(connection: &Connection) -> Result<PreferenceSnapshot> {
    validate_schema(connection)?;
    let count: i64 =
        connection.query_row("SELECT count(*) FROM preference_registry", [], |r| r.get(0))?;
    ensure!(count == 1, "preferences registry must contain one snapshot");
    // Check the byte bounds before allocating either persisted string. Inspection
    // runs in one read transaction, so a writer cannot swap values between reads.
    let (singleton, revision, updated_at_ms, hash_bytes, document_bytes): (
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = connection.query_row(
        "SELECT singleton,revision,updated_at_ms,length(CAST(document_hash AS BLOB)),length(CAST(document_json AS BLOB)) FROM preference_registry",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    ensure!(
        singleton == 1
            && (1..=MAX_EXACT_INTEGER as i64).contains(&revision)
            && hash_bytes == 64
            && (1..=MAX_INPUT_BYTES as i64).contains(&document_bytes),
        "invalid preferences registry snapshot metadata"
    );
    valid_time(updated_at_ms)?;
    let (stored_hash, stored_json): (String, String) = connection.query_row(
        "SELECT document_hash,document_json FROM preference_registry WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let document = parse_document(stored_json.as_bytes())?;
    let (_, expected_hash) = canonical_document(&document)?;
    ensure!(
        stored_hash == expected_hash,
        "preferences registry document hash mismatch"
    );
    Ok(PreferenceSnapshot {
        revision: revision as u64,
        document_hash: stored_hash,
        updated_at_ms,
        document,
    })
}

/// Read an existing registry without creating, migrating or updating it.
pub fn inspect(path: &Path) -> Result<PreferenceSnapshot> {
    validate_path(path)?;
    ensure!(existing_file(path)?, "preferences registry does not exist");
    let mut connection = open(path, false)?;
    let tx = connection.transaction()?;
    let result = snapshot(&tx)?;
    tx.commit()?;
    Ok(result)
}

struct Stage(PathBuf);

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn stage(parent: &Path) -> Result<Stage> {
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(parent)
        .context("create private preferences registry directory")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..32 {
        let path = parent.join(format!(
            ".ticket-preferences-{}-{nonce}-{}.tmp",
            std::process::id(),
            STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(_) => return Ok(Stage(path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create private preferences registry stage"),
        }
    }
    Err(anyhow!("could not allocate preferences registry stage"))
}

fn update_existing(
    path: &Path,
    document: &PreferencesDocument,
    json: &str,
    hash: &str,
    expected_revision: u64,
    now_ms: i64,
) -> Result<PreferenceSnapshot> {
    // Refuse foreign or damaged files before opening them for writing. Repeat the
    // schema/content check under the write lock; the preliminary read is not CAS.
    inspect(path)?;
    let mut connection = open(path, true)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = snapshot(&tx)?;
    ensure!(
        current.revision == expected_revision,
        "stale preference revision: expected {expected_revision}, current {}",
        current.revision
    );
    ensure!(
        now_ms >= current.updated_at_ms,
        "preference timestamp would move backwards"
    );
    if current.document == *document {
        tx.commit()?;
        return Ok(current);
    }
    ensure!(
        current.revision < MAX_EXACT_INTEGER,
        "preference revision limit reached"
    );
    let next = PreferenceSnapshot {
        revision: current.revision + 1,
        document_hash: hash.to_owned(),
        updated_at_ms: now_ms,
        document: document.clone(),
    };
    let changed = tx.execute(
        "UPDATE preference_registry SET revision=?1,document_hash=?2,updated_at_ms=?3,document_json=?4 WHERE singleton=1 AND revision=?5",
        params![next.revision as i64, hash, now_ms, json, expected_revision as i64],
    )?;
    ensure!(changed == 1, "preference revision changed during import");
    tx.commit()?;
    Ok(next)
}

/// Compare-and-swap one complete preferences document. Revision zero creates a
/// missing registry; it never resets an existing file. Identical current content
/// returns the existing snapshot, but a stale expected revision still fails.
pub fn import(
    path: &Path,
    document: &PreferencesDocument,
    expected_revision: u64,
    now_ms: i64,
) -> Result<PreferenceSnapshot> {
    validate_path(path)?;
    valid_time(now_ms)?;
    ensure!(
        expected_revision <= MAX_EXACT_INTEGER,
        "expected preference revision is outside the exact integer range"
    );
    let (json, hash) = canonical_document(document)?;
    if existing_file(path)? {
        return update_existing(path, document, &json, &hash, expected_revision, now_ms);
    }
    ensure!(
        expected_revision == 0,
        "preferences registry does not exist; expected revision must be zero"
    );
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staged = stage(parent)?;
    {
        let mut connection = open(&staged.0, true)?;
        connection.pragma_update(None, "journal_mode", "DELETE")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.execute(
            "INSERT INTO preference_registry(singleton,revision,document_hash,updated_at_ms,document_json) VALUES(1,1,?1,?2,?3)",
            params![hash, now_ms, json],
        )?;
        tx.commit()?;
        connection
            .close()
            .map_err(|(_, error)| error)
            .context("close initialized preferences registry")?;
    }
    File::open(&staged.0)?.sync_all()?;
    // No-clobber publication exposes only a fully committed, closed SQLite file.
    // Concurrent creators cannot overwrite each other or initialize someone
    // else's empty database. The loser applies the ordinary revision check.
    match fs::hard_link(&staged.0, path) {
        Ok(()) => {
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(PreferenceSnapshot {
                revision: 1,
                document_hash: hash,
                updated_at_ms: now_ms,
                document: document.clone(),
            })
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            update_existing(path, document, &json, &hash, expected_revision, now_ms)
        }
        Err(error) => Err(error).context("publish preferences registry without replacement"),
    }
}
