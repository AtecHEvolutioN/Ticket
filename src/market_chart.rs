//! Read-only history exports. Rendering never runs on the inventory/alert path.
use std::{
    collections::{BTreeMap, HashSet},
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};

const MAX_EXPORT_BYTES: usize = 16 * 1024 * 1024;
const TEMPLATE: &str = include_str!("../apps/discord/public/market.html");
const SCRIPT: &str = include_str!("../apps/discord/public/market.js");
const STYLE: &str = include_str!("../apps/discord/public/market.css");

#[derive(Debug, Serialize)]
pub struct ExportReport {
    pub output: PathBuf,
    pub events: usize,
    pub samples: usize,
    pub generated_at: i64,
    pub live_connection: bool,
}

/// Export an existing database without opening a Store or migrating its schema.
pub fn export(
    databases: &[PathBuf],
    output: &Path,
    from: Option<&str>,
    to: Option<&str>,
    event_url: Option<&str>,
    generated_at: i64,
) -> Result<ExportReport> {
    ensure!(
        !databases.is_empty() && databases.len() <= 8,
        "Supply one to eight history databases"
    );
    let extension = output
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ensure!(
        matches!(extension.as_str(), "json" | "html"),
        "History output must end in .json or .html"
    );
    ensure!(
        !output.exists(),
        "Output already exists; choose a new export filename"
    );
    let from = parse_time(from)?;
    let to = parse_time(to)?;
    ensure!(
        from.zip(to).is_none_or(|(a, b)| a <= b),
        "History start must precede end"
    );
    if let Some(url) = event_url {
        crate::config::validate_public_url(url)?;
    }
    let mut seen = HashSet::new();
    let mut events = BTreeMap::<String, Value>::new();
    for (index, database) in databases.iter().enumerate() {
        let canonical =
            std::fs::canonicalize(database).context("History database does not exist")?;
        ensure!(
            seen.insert(canonical),
            "The same database was supplied more than once"
        );
        let history = if let Some(url) = event_url {
            crate::market_history::read_export_event(database, generated_at, from, to, url)?
        } else {
            crate::market_history::read_export(database, generated_at, from, to)?
        };
        let value = serde_json::to_value(history)?;
        for event in value["events"]
            .as_array()
            .context("Invalid history export events")?
        {
            if event_url.is_some_and(|url| {
                !same_export_event(url, event["event_url"].as_str().unwrap_or(""))
            }) {
                continue;
            }
            let mut event = event.clone();
            // Separate databases can be independent scans of the same event. Never
            // add their counts or assume their observations cover disjoint tickets.
            for source in event["sources"]
                .as_array_mut()
                .context("Invalid history sources")?
            {
                if databases.len() > 1 {
                    source["source_id"] = json!(format!(
                        "{}-db-{}",
                        source["source_id"]
                            .as_str()
                            .context("Invalid history source ID")?,
                        index + 1
                    ));
                    source["scope"] = json!(format!(
                        "{} · database {}",
                        source["scope"].as_str().unwrap_or("event"),
                        index + 1
                    ));
                }
            }
            let id = event["event_id"]
                .as_str()
                .context("Invalid history event ID")?
                .to_owned();
            if let Some(existing) = events.get_mut(&id) {
                for (field, minimum) in [("first_observed_at", true), ("last_observed_at", false)] {
                    let a = existing[field]
                        .as_i64()
                        .context("Invalid history timestamp")?;
                    let b = event[field].as_i64().context("Invalid history timestamp")?;
                    existing[field] = json!(if minimum { a.min(b) } else { a.max(b) });
                }
                existing["sources"]
                    .as_array_mut()
                    .context("Invalid history sources")?
                    .extend(
                        event["sources"]
                            .as_array()
                            .context("Invalid history sources")?
                            .iter()
                            .cloned(),
                    );
            } else {
                events.insert(id, event);
            }
        }
    }
    ensure!(
        event_url.is_none() || !events.is_empty(),
        "No saved history matches this event and time range"
    );
    let events: Vec<_> = events.into_values().collect();
    ensure!(
        events.len() <= 2_000,
        "History exceeds 2,000 events; select an event or narrower time range"
    );
    let sources = events
        .iter()
        .filter_map(|e| e["sources"].as_array())
        .map(Vec::len)
        .sum::<usize>();
    ensure!(
        sources <= 5_000,
        "History exceeds 5,000 sources; select an event or narrower time range"
    );
    let samples: usize = events
        .iter()
        .flat_map(|e| e["sources"].as_array().into_iter().flatten())
        .map(|s| s["samples"].as_array().map_or(0, Vec::len))
        .sum();
    ensure!(
        samples <= crate::market_history::MAX_EXPORT_SAMPLES,
        "Combined history exceeds 10,000 samples; narrow the time range or select an event"
    );
    let series_count = events
        .iter()
        .flat_map(|e| e["sources"].as_array().into_iter().flatten())
        .flat_map(|s| s["samples"].as_array().into_iter().flatten())
        .map(|s| s["series"].as_array().map_or(0, Vec::len))
        .sum::<usize>();
    ensure!(
        series_count <= 200_000,
        "History exceeds 200,000 series points; narrow the time range or select an event"
    );
    let report = ExportReport {
        output: output.to_path_buf(),
        events: events.len(),
        samples,
        generated_at,
        live_connection: false,
    };
    let value = json!({"schema_version":1,"generated_at":generated_at,"events":events});
    let data = serde_json::to_string(&value)?;
    ensure!(
        data.len() <= MAX_EXPORT_BYTES,
        "History exceeds 16 MiB; narrow the time range or select an event"
    );
    let contents = if extension == "html" {
        render_html(&data)?
    } else {
        data
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(output).context("Create new history export")?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(output);
        return Err(error).context("Write history export");
    }
    Ok(report)
}

fn parse_time(value: Option<&str>) -> Result<Option<i64>> {
    value
        .map(|value| {
            chrono::DateTime::parse_from_rfc3339(value)
                .map(|time| time.timestamp())
                .context("Use an RFC3339 timestamp with an explicit timezone")
        })
        .transpose()
}

fn same_export_event(a: &str, b: &str) -> bool {
    if crate::event_identity::same_event(a, b) {
        return true;
    }
    let normalize = |value: &str| {
        crate::config::validate_public_url(value).map(|mut url| {
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        })
    };
    match (normalize(a), normalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Data is inert JSON, with HTML delimiters escaped even inside string values.
pub fn render_html(data: &str) -> Result<String> {
    ensure!(data.len() <= MAX_EXPORT_BYTES, "History exceeds 16 MiB");
    let _: Value = serde_json::from_str(data).context("Invalid history JSON")?;
    let safe = data
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    ensure!(
        safe.len() <= MAX_EXPORT_BYTES,
        "Escaped history exceeds the viewer's 16 MiB limit; select a smaller time range"
    );
    ensure!(
        TEMPLATE.contains("<!-- HISTORY_DATA -->")
            && TEMPLATE.contains("<script src=\"market.js\" defer></script>")
            && TEMPLATE.contains("<link rel=\"stylesheet\" href=\"market.css\">"),
        "History viewer template contract changed"
    );
    // All executable content is compiled from our static asset, never file data.
    let html = TEMPLATE.replace("<head>", "<head>\n<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\">")
        .replace("<link rel=\"stylesheet\" href=\"market.css\">", &format!("<style>{STYLE}</style>"))
        .replace("<!-- HISTORY_DATA -->", &format!("<script type=\"application/json\" id=\"ticket-history-data\">{safe}</script>"))
        .replace("<script src=\"market.js\" defer></script>", &format!("<script>{SCRIPT}</script>"));
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standalone_export_cannot_escape_data_script() {
        let dangerous = "</script><script>alert('unsafe')</script>&\u{2028}";
        let data = json!({"schema_version":1,"events":[],"label":dangerous}).to_string();
        let html = render_html(&data).unwrap();
        assert!(!html.contains(dangerous));
        let embedded = html
            .split("id=\"ticket-history-data\">")
            .nth(1)
            .unwrap()
            .split("</script>")
            .next()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(embedded).unwrap()["label"],
            dangerous
        );
        assert!(!html.contains("src=\"market.js\""));
        assert!(!html.contains("href=\"market.css\""));
    }
    #[test]
    fn exports_refuse_missing_databases_and_existing_destinations() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.sqlite");
        let output = directory.path().join("history.json");
        assert!(
            export(
                std::slice::from_ref(&missing),
                &output,
                None,
                None,
                None,
                10
            )
            .is_err()
        );
        assert!(!missing.exists() && !output.exists());
        std::fs::write(&output, "preserve").unwrap();
        assert!(export(&[missing], &output, None, None, None, 10).is_err());
        assert_eq!(std::fs::read_to_string(output).unwrap(), "preserve");
    }
}
