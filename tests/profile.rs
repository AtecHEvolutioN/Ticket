use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const URL: &str = "https://www.ticketmaster.pl/event/offline-review/123456";
const OFFERS: &str = r#"<!doctype html><main class="complete">
<article class="offer" data-id="synthetic-offer">
  <span class="type">Synthetic standing ticket</span><span class="section">GA</span>
  <span class="price">435,85 PLN</span><span class="quantity">2</span>
  <button class="buy">Buy</button>
</article></main>"#;

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("review.toml");
    fs::write(
        &config,
        format!(
            r#"
database = "untouched.sqlite"
[[watches]]
id = "review"
name = "Synthetic offline review"
artist = "Synthetic artist"
country = "PL"
url = "{URL}"
currency = "PLN"
[watches.parser]
selectors_verified = false
offers_selector = ".offer"
offer_available_selector = ".buy"
complete_selector = ".complete"
offer_id_attribute = "data-id"
ticket_type_selector = ".type"
section_selector = ".section"
price_selector = ".price"
quantity_selector = ".quantity"
quantity_kind = "listing"
price_scope = "per_ticket"
price_decimal_separator = ","
"#
        ),
    )
    .unwrap();
    let html = directory.path().join("capture.html");
    fs::write(&html, OFFERS).unwrap();
    (directory, config, html)
}

fn metadata(final_url: &str) -> Value {
    json!({"schema_version":1, "original_url":URL, "final_url":final_url,
        "captured_at":"2026-11-09T12:00:00.000Z", "status":200,
        "rendered":true, "profile_verified":false})
}

fn sidecar(html: &Path, value: Value) {
    let mut path = html.as_os_str().to_os_string();
    path.push(".json");
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn review(config: &Path, html: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .env_remove("DISCORD_BOT_TOKEN")
        .env_remove("TICKETMASTER_API_KEY")
        .arg("--config")
        .arg(config)
        .args(["profile-check", "--watch", "review", "--html"])
        .arg(html)
        .output()
        .unwrap()
}

#[test]
fn capture_review_outputs_unverified_candidates_without_mutating_state() {
    let (directory, config, html) = setup();
    sidecar(&html, metadata(URL));
    let database = directory.path().join("untouched.sqlite");
    fs::write(&database, b"not even a database: must never be opened").unwrap();
    let before = fs::read(&config).unwrap();
    let result = review(&config, &html);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["review_only"], true);
    assert_eq!(report["configured_verified"], false);
    assert_eq!(report["source"]["event_identity_matches"], true);
    assert_eq!(report["source"]["independently_verified"], false);
    assert!(report.get("availability").is_none());
    let offer = &report["unverified_candidates"][0];
    assert_eq!(offer["ticket_type"], "Synthetic standing ticket");
    assert_eq!(offer["price_minor"], 43585);
    assert_eq!(offer["quantity"], 2);
    assert_eq!(offer["quantity_kind"], "listing");
    assert_eq!(offer["price_scope"], "per_ticket");
    assert_eq!(fs::read(&config).unwrap(), before);
    assert_eq!(
        fs::read(&database).unwrap(),
        b"not even a database: must never be opened"
    );
    assert!(!directory.path().join("untouched.lock").exists());
    assert!(!directory.path().join("untouched.sqlite-wal").exists());
}

#[test]
fn wrong_event_capture_is_rejected_before_candidate_output() {
    let (directory, config, html) = setup();
    sidecar(
        &html,
        metadata("https://www.ticketmaster.pl/event/another/999999"),
    );
    let result = review(&config, &html);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("does not match the watched event"));
    assert!(!directory.path().join("untouched.sqlite").exists());
}

#[test]
fn ambiguous_raw_capture_url_is_rejected_before_path_normalization() {
    let (directory, config, html) = setup();
    sidecar(
        &html,
        metadata("https://www.ticketmaster.pl/login/../event/offline-review/123456"),
    );
    let result = review(&config, &html);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("does not match the watched event"));
    assert!(!directory.path().join("untouched.sqlite").exists());
}

#[test]
fn challenge_cannot_be_reviewed_as_positive_offer() {
    let (_directory, config, html) = setup();
    sidecar(&html, metadata(URL));
    fs::write(
        &html,
        format!("<h1>Please complete the captcha to continue</h1>{OFFERS}"),
    )
    .unwrap();
    let result = review(&config, &html);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("queue, challenge, or access restriction")
    );
}

#[test]
fn unsuccessful_or_self_verified_capture_metadata_is_rejected() {
    let (_directory, config, html) = setup();
    let mut value = metadata(URL);
    value["status"] = json!(403);
    sidecar(&html, value);
    assert!(!review(&config, &html).status.success());
    let mut value = metadata(URL);
    value["profile_verified"] = json!(true);
    sidecar(&html, value);
    assert!(!review(&config, &html).status.success());
}

#[test]
fn raw_html_without_sidecar_has_explicitly_unverified_provenance() {
    let (directory, config, html) = setup();
    let result = review(&config, &html);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["source"]["sidecar_present"], false);
    assert_eq!(report["source"]["event_identity_matches"], false);
    assert_eq!(report["source"]["rendered"], false);
    assert_eq!(report["review_only"], true);
    assert!(!directory.path().join("untouched.sqlite").exists());
}

#[test]
fn explicit_capture_json_and_cli_help_are_available() {
    let (directory, config, html) = setup();
    let capture_json = directory.path().join("metadata.json");
    fs::write(&capture_json, metadata(URL).to_string()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .arg("--config")
        .arg(&config)
        .args(["profile-check", "--watch", "review", "--html"])
        .arg(&html)
        .arg("--capture-json")
        .arg(capture_json)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let help = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .args(["profile-check", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--capture-json"));
}
