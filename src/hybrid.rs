//! Join catalog selection to website watches without treating catalog status as inventory.
use std::collections::HashSet;

use anyhow::Result;
use chrono::{DateTime, Days, NaiveDate, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::{Subscription, Watch, WebsiteProfile, validate_public_url};

#[derive(Debug, Default)]
pub struct Plan {
    pub watches: Vec<Watch>,
    pub skipped: Vec<Value>,
}

/// URL identity ignores language/tracking context and the public www alias.
/// The watch itself retains the validated URL's host to avoid forcing a redirect.
pub fn canonical_event_url(input: &str) -> Result<String> {
    let mut url = validate_public_url(input)?;
    if !url.path().contains("/event/") {
        anyhow::bail!("A catalog watch must target a public event URL");
    }
    url.set_query(None);
    url.set_fragment(None);
    if let Some(host) = url.host_str().and_then(|host| host.strip_prefix("www.")) {
        let host = host.to_owned();
        url.set_host(Some(&host))?;
    }
    Ok(url.into())
}

pub fn plan(subscription: &Subscription, profile: &WebsiteProfile, events: &[Value]) -> Plan {
    plan_at(subscription, profile, events, Utc::now())
}

fn plan_at(
    subscription: &Subscription,
    profile: &WebsiteProfile,
    events: &[Value],
    at: DateTime<Utc>,
) -> Plan {
    let mut output = Plan::default();
    if !subscription.enabled {
        return output;
    }
    let mut seen = HashSet::new();
    for event in events {
        match event_watch(subscription, profile, event, at) {
            Ok(watch) if seen.insert(watch.id.clone()) => output.watches.push(watch),
            Ok(_) => output
                .skipped
                .push(skipped(subscription, event, "duplicate event URL")),
            Err(reason) => output.skipped.push(skipped(subscription, event, reason)),
        }
    }
    output
}

fn skipped(subscription: &Subscription, event: &Value, reason: &str) -> Value {
    json!({
        "subscription_id": subscription.id,
        "event_id": event.get("id").and_then(Value::as_str).map(|id| bounded(id, 160)),
        "reason": reason,
    })
}

fn event_watch(
    subscription: &Subscription,
    profile: &WebsiteProfile,
    event: &Value,
    at: DateTime<Utc>,
) -> std::result::Result<Watch, &'static str> {
    if subscription.profile != profile.id {
        return Err("subscription profile mismatch");
    }
    if subscription.all_artists
        && (!subscription.attraction_id.is_empty() || subscription.country.is_none())
    {
        return Err("all-artist discovery requires a country without an attraction filter");
    }
    let _event_id = nonempty(event.get("id")).ok_or("missing event identity")?;
    if !subscription.all_artists
        && !event
            .get("attraction_ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| {
                ids.iter()
                    .any(|id| id.as_str() == Some(&subscription.attraction_id))
            })
    {
        return Err("selected attraction is not confirmed for this event");
    }
    let name = nonempty(event.get("name")).ok_or("missing event name")?;
    if subscription.exclude_keywords.iter().any(|keyword| {
        !keyword.trim().is_empty() && name.to_lowercase().contains(&keyword.trim().to_lowercase())
    }) {
        return Err("event name matches an excluded keyword");
    }
    if event
        .get("event_status")
        .and_then(Value::as_str)
        .is_some_and(|status| {
            matches!(
                status.to_ascii_lowercase().as_str(),
                "cancelled" | "canceled"
            )
        })
    {
        return Err("catalog marks event cancelled");
    }
    if event_is_past(event, at) {
        return Err("catalog event date has passed");
    }
    let source_url = nonempty(event.get("url")).ok_or("no supported public event URL")?;
    let mut url = validate_public_url(source_url).map_err(|_| "unsupported public event URL")?;
    let identity = canonical_event_url(source_url).map_err(|_| "unsupported public event URL")?;
    if !profile
        .hosts
        .iter()
        .any(|host| Some(host.as_str()) == url.host_str())
    {
        return Err("event host has no matching website profile");
    }
    url.set_query(None);
    url.set_fragment(None);
    let venue = event
        .get("venues")
        .and_then(Value::as_array)
        .and_then(|venues| {
            venues
                .iter()
                .find(|venue| venue_matches(subscription, venue))
        })
        .ok_or("no venue confirms the selected location")?;
    let country = venue["country"]["countryCode"]
        .as_str()
        .expect("venue_matches checked country");
    let city = nonempty(venue.get("city")).map(str::to_owned);
    let local_date = event
        .pointer("/dates/start/localDate")
        .and_then(Value::as_str);
    let venue_name = nonempty(venue.get("name"));
    let display_name = [Some(name), local_date, venue_name]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" — ");
    Ok(Watch {
        id: format!("auto-{:x}", Sha256::digest(identity.as_bytes())),
        name: bounded(&display_name, 500),
        artist: if subscription.all_artists {
            event_artists(event)
        } else {
            subscription.artist.clone()
        },
        country: country.into(),
        city,
        url: url.into(),
        enabled: true,
        mode: profile.mode,
        interval_secs: profile.interval_secs,
        notify_initial: profile.notify_initial,
        // Discovery or page metadata never enables availability alerts in this pipeline.
        notify_metadata: false,
        notify_offer_changes: profile.notify_offer_changes,
        min_quantity: profile.min_quantity,
        max_price_minor: profile.max_price_minor,
        currency: profile.currency.clone(),
        parser: profile.parser.clone(),
    })
}

fn event_artists(event: &Value) -> String {
    let mut names = Vec::new();
    if let Some(attractions) = event.get("attractions").and_then(Value::as_array) {
        for attraction in attractions.iter().take(100) {
            if let Some(name) = nonempty(attraction.get("name")) {
                let name = bounded(name, 400);
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    if names.is_empty() {
        "Artist not provided".into()
    } else {
        bounded(&names.join(" / "), 500)
    }
}

fn nonempty(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn venue_matches(subscription: &Subscription, venue: &Value) -> bool {
    let Some(country) = venue
        .pointer("/country/countryCode")
        .and_then(Value::as_str)
    else {
        return false;
    };
    if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return false;
    }
    subscription
        .country
        .as_ref()
        .is_none_or(|wanted| wanted == country)
        && subscription.city.as_ref().is_none_or(|wanted| {
            nonempty(venue.get("city")).is_some_and(|city| normalized(city) == normalized(wanted))
        })
}

fn event_is_past(event: &Value, at: DateTime<Utc>) -> bool {
    if event
        .get("event_status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("postponed"))
        || [
            "/dates/start/dateTBA",
            "/dates/start/dateTBD",
            "/dates/end/approximate",
        ]
        .iter()
        .any(|path| event.pointer(path).and_then(Value::as_bool) == Some(true))
    {
        // A postponed or explicitly uncertain date may retain its former timestamp.
        return false;
    }
    if let Some(end) = event
        .pointer("/dates/end/dateTime")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    {
        return end <= at;
    }
    let date = event
        .pointer("/dates/end/localDate")
        .or_else(|| event.pointer("/dates/start/localDate"))
        .and_then(Value::as_str)
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
    // No timezone library is needed to avoid dropping an event still occurring in
    // another timezone. Keep yesterday/today, and unknown/TBA dates, conservatively.
    date.is_some_and(|date| {
        at.date_naive()
            .checked_sub_days(Days::new(1))
            .is_some_and(|yesterday| date < yesterday)
    })
}

fn bounded(text: &str, max_bytes: usize) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = clean.len().min(max_bytes);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FetchMode, ParserConfig};

    fn selection() -> (Subscription, WebsiteProfile) {
        (
            Subscription {
                id: "korn-pl".into(),
                artist: "KoRn".into(),
                attraction_id: "artist-1".into(),
                country: Some("PL".into()),
                city: Some("Krakow".into()),
                profile: "poland".into(),
                ..Subscription::default()
            },
            WebsiteProfile {
                id: "poland".into(),
                hosts: vec!["www.ticketmaster.pl".into(), "ticketmaster.pl".into()],
                mode: FetchMode::Browser,
                min_quantity: Some(2),
                max_price_minor: Some(100000),
                currency: Some("PLN".into()),
                parser: ParserConfig {
                    selectors_verified: true,
                    offers_selector: Some(".offer".into()),
                    ..ParserConfig::default()
                },
                ..WebsiteProfile::default()
            },
        )
    }
    fn event() -> Value {
        json!({"id":"event-1", "name":"KoRn", "attraction_ids":["artist-1"],
            "url":"https://www.ticketmaster.pl/event/korn/123", "event_status":"onsale",
            "dates":{"start":{"localDate":"2099-11-17"}},
            "venues":[{"name":"Arena", "city":"Krakow", "country":{"countryCode":"PL"}}],
            "price_ranges":[{"min":10,"max":1000,"currency":"PLN"}]})
    }
    fn at() -> DateTime<Utc> {
        "2026-11-09T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn selected_catalog_event_becomes_a_website_watch_without_inventory_claims() {
        let (subscription, profile) = selection();
        let result = plan_at(&subscription, &profile, &[event()], at());
        assert!(result.skipped.is_empty());
        let watch = &result.watches[0];
        assert_eq!(watch.artist, "KoRn");
        assert_eq!(watch.country, "PL");
        assert!(watch.name.contains("2099-11-17"));
        assert_eq!(watch.mode, FetchMode::Browser);
        assert_eq!(watch.max_price_minor, Some(100000));
        assert_eq!(watch.min_quantity, Some(2));
        assert!(!watch.notify_metadata);
        assert!(!watch.notify_initial);
        assert!(watch.id.starts_with("auto-"));
    }

    #[test]
    fn wrong_artist_or_location_or_profile_has_explicit_coverage_reason() {
        let (subscription, profile) = selection();
        for (path, value) in [
            ("/attraction_ids", json!(["unrelated"])),
            ("/venues/0/country/countryCode", json!("CZ")),
            ("/venues/0/city", json!("Prague")),
            ("/url", json!("https://www.ticketmaster.cz/event/korn/123")),
            (
                "/url",
                json!("https://secure.ticketmaster.pl/payment/?t=secret"),
            ),
        ] {
            let mut candidate = event();
            *candidate.pointer_mut(path).unwrap() = value;
            let result = plan_at(&subscription, &profile, &[candidate], at());
            assert!(result.watches.is_empty(), "{path}");
            assert_eq!(result.skipped.len(), 1);
            assert!(!result.skipped[0].to_string().contains("secret"));
        }
    }

    #[test]
    fn country_and_city_must_match_the_same_venue() {
        let (subscription, profile) = selection();
        let mut candidate = event();
        candidate["venues"] = json!([
            {"city":"Warsaw", "country":{"countryCode":"PL"}},
            {"city":"Krakow", "country":{"countryCode":"CZ"}}
        ]);
        assert!(
            plan_at(&subscription, &profile, &[candidate], at())
                .watches
                .is_empty()
        );
    }

    #[test]
    fn canonical_url_deduplicates_www_aliases_and_language_without_price_identity() {
        let (subscription, profile) = selection();
        let original = event();
        let mut duplicate = original.clone();
        duplicate["id"] = json!("another-catalog-id");
        duplicate["url"] = json!("https://ticketmaster.pl/event/korn/123?language=en#tickets");
        let result = plan_at(&subscription, &profile, &[original, duplicate], at());
        assert_eq!(result.watches.len(), 1);
        assert_eq!(result.skipped[0]["reason"], "duplicate event URL");
    }

    #[test]
    fn cancelled_past_and_explicit_excluded_addons_do_not_become_watches() {
        let (mut subscription, profile) = selection();
        subscription.exclude_keywords = vec!["Parking".into()];
        for (path, value) in [
            ("/name", json!("KoRn - parking")),
            ("/event_status", json!("cancelled")),
            ("/dates/start/localDate", json!("2026-11-07")),
        ] {
            let mut candidate = event();
            *candidate.pointer_mut(path).unwrap() = value;
            assert!(
                plan_at(&subscription, &profile, &[candidate], at())
                    .watches
                    .is_empty(),
                "{path}"
            );
        }
        let mut yesterday = event();
        yesterday["dates"]["start"]["localDate"] = json!("2026-11-08");
        assert_eq!(
            plan_at(&subscription, &profile, &[yesterday], at())
                .watches
                .len(),
            1
        );
    }

    #[test]
    fn disabled_selection_does_not_generate_checks_and_offsale_is_not_sold_out() {
        let (mut subscription, profile) = selection();
        let mut candidate = event();
        candidate["event_status"] = json!("offsale");
        assert_eq!(
            plan_at(&subscription, &profile, &[candidate.clone()], at())
                .watches
                .len(),
            1
        );
        subscription.enabled = false;
        assert!(
            plan_at(&subscription, &profile, &[candidate], at())
                .watches
                .is_empty()
        );
    }

    #[test]
    fn postponed_or_tba_dates_do_not_retire_a_watch_from_an_old_placeholder() {
        let (subscription, profile) = selection();
        for flag in ["dateTBA", "dateTBD"] {
            let mut candidate = event();
            candidate["dates"]["start"]["localDate"] = json!("2020-01-01");
            candidate["dates"]["start"][flag] = json!(true);
            assert_eq!(
                plan_at(&subscription, &profile, &[candidate], at())
                    .watches
                    .len(),
                1
            );
        }
        let mut candidate = event();
        candidate["event_status"] = json!("postponed");
        candidate["dates"]["start"]["localDate"] = json!("2020-01-01");
        assert_eq!(
            plan_at(&subscription, &profile, &[candidate], at())
                .watches
                .len(),
            1
        );
    }
}
