//! Discord REST delivery. No browser cookies or Ticketmaster credentials enter this module.
use crate::{
    config::DiscordConfig,
    model::{FeeScope, PendingAlert, PriceScope, QuantityKind},
};
use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fmt, time::Duration};
use tokio::time::Instant;

const MAX_RESPONSE_BYTES: usize = 65_536;
const MAX_OFFERS: usize = 6;

pub struct Discord {
    client: Client,
    config: DiscordConfig,
    endpoint: String,
    next_send: Instant,
}

impl fmt::Debug for Discord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Discord")
            .field("channel_id", &self.config.channel_id)
            .field("credentials", &"[redacted]")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Sent { message_id: String },
    Retry { after_secs: u64, reason: String },
    Permanent { reason: String },
}

impl Discord {
    /// Credential-free loopback client for shared local fixture tests only.
    #[cfg(test)]
    pub(crate) fn for_test(config: &DiscordConfig, endpoint: String) -> Self {
        let url = url::Url::parse(&endpoint).expect("Fixture endpoint must be a URL");
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap(),
            config: clone_config(config),
            endpoint,
            next_send: Instant::now(),
        }
    }

    /// Construct only when live sending was explicitly enabled. `preview` needs no token.
    pub fn new(config: &DiscordConfig) -> Result<Self> {
        let channel = config
            .channel_id
            .as_deref()
            .context("Discord channel_id is required for live delivery")?;
        if !is_snowflake(channel) {
            bail!("Discord channel_id must contain a valid numeric ID");
        }
        let token = std::env::var(&config.token_env).map_err(|_| {
            anyhow::anyhow!(
                "Discord bot token environment variable is not set or is not valid text"
            )
        })?;
        if token.trim().is_empty() {
            bail!("Discord bot token is empty");
        }
        let mut auth = header::HeaderValue::from_str(&format!("Bot {}", token.trim()))
            .map_err(|_| anyhow::anyhow!("Discord bot token contains invalid header characters"))?;
        auth.set_sensitive(true);
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, auth);
        let client = Client::builder()
            .default_headers(headers)
            .user_agent("TicketWatch/0.1 (Discord availability alerts)")
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| anyhow::anyhow!("Unable to initialize Discord HTTP client"))?;
        Ok(Self {
            client,
            config: clone_config(config),
            endpoint: format!("https://discord.com/api/v10/channels/{channel}/messages"),
            next_send: Instant::now(),
        })
    }

    /// One caller owns the bot limiter. Parent persists retries in its durable outbox.
    /// A timeout can mean Discord accepted a message: retries reuse its nonce.
    pub async fn send(&mut self, pending: &PendingAlert) -> Result<Delivery> {
        let now = Instant::now();
        if self.next_send > now {
            return Ok(Delivery::Retry {
                after_secs: ceil_seconds(self.next_send.duration_since(now)),
                reason: "Discord rate-limit cooldown".into(),
            });
        }
        let mut response = match self
            .client
            .post(&self.endpoint)
            .json(&preview(pending, &self.config))
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                return Ok(self.retry(
                    pending,
                    "Discord transport failure; delivery may be ambiguous",
                ));
            }
        };
        let status = response.status();
        let headers = response.headers().clone();
        let exhausted = headers
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            == Some("0");
        if exhausted && let Some(delay) = header_seconds(&headers, "x-ratelimit-reset-after") {
            self.set_cooldown(delay);
        }

        // A tiny bounded response is enough for the message ID and a rate-limit body.
        let mut bytes = Vec::new();
        let oversized = response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64);
        let mut unreadable = oversized;
        if !oversized {
            loop {
                match response.chunk().await {
                    Ok(Some(chunk))
                        if bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES =>
                    {
                        bytes.extend_from_slice(&chunk)
                    }
                    Ok(Some(_)) | Err(_) => {
                        unreadable = true;
                        break;
                    }
                    Ok(None) => break,
                }
            }
        }
        let body: Value = if unreadable {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if status == StatusCode::TOO_MANY_REQUESTS {
            let header_delay = retry_after(&headers);
            let body_delay = body
                .get("retry_after")
                .and_then(Value::as_f64)
                .and_then(finite_duration);
            let delay = header_delay
                .into_iter()
                .chain(body_delay)
                .max()
                .unwrap_or(Duration::from_secs(5));
            // A single-channel client shares the same cooldown for route and global 429s.
            self.set_cooldown(delay);
            let after_secs = ceil_seconds(self.next_send.saturating_duration_since(Instant::now()));
            return Ok(Delivery::Retry {
                after_secs,
                reason: "Discord rate limited the request".into(),
            });
        }
        if status.is_success() {
            if let Some(id) = body
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| is_snowflake(id))
            {
                return Ok(Delivery::Sent {
                    message_id: id.into(),
                });
            }
            return Ok(self.retry(
                pending,
                "Discord acceptance could not be confirmed; retry keeps the same nonce",
            ));
        }
        if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
            return Ok(self.retry(
                pending,
                "Discord temporarily unavailable; delivery may be ambiguous",
            ));
        }
        let reason = match status {
            StatusCode::UNAUTHORIZED => "Discord rejected the bot credentials",
            StatusCode::FORBIDDEN => {
                "Discord bot lacks access or permission to send to the channel"
            }
            StatusCode::NOT_FOUND => "Discord channel was not found",
            StatusCode::BAD_REQUEST => "Discord rejected the message payload",
            _ => "Discord returned a non-retryable response",
        };
        Ok(Delivery::Permanent {
            reason: format!("{reason} (HTTP {})", status.as_u16()),
        })
    }

    /// Persist this delay after every response, including a successful send that
    /// exhausts its route bucket, so a restart cannot discard the known limiter.
    pub fn cooldown_secs(&self) -> Option<u64> {
        let remaining = self.next_send.saturating_duration_since(Instant::now());
        (!remaining.is_zero()).then(|| ceil_seconds(remaining))
    }

    fn set_cooldown(&mut self, delay: Duration) {
        if let Some(deadline) = Instant::now().checked_add(delay) {
            self.next_send = self.next_send.max(deadline);
        } else {
            // Invalid astronomical server values still must not trigger immediate retries.
            self.next_send = self
                .next_send
                .max(Instant::now() + Duration::from_secs(86_400));
        }
    }

    fn retry(&mut self, pending: &PendingAlert, reason: &str) -> Delivery {
        let delay = Duration::from_secs(2_u64.pow(pending.attempts.min(7)).max(2));
        self.set_cooldown(delay);
        Delivery::Retry {
            after_secs: ceil_seconds(self.next_send.saturating_duration_since(Instant::now())),
            reason: reason.into(),
        }
    }
}

fn clone_config(config: &DiscordConfig) -> DiscordConfig {
    DiscordConfig {
        channel_id: config.channel_id.clone(),
        token_env: config.token_env.clone(),
        mention_user_ids: config.mention_user_ids.clone(),
        mention_role_ids: config.mention_role_ids.clone(),
    }
}

/// Produce the exact payload without reading credentials or making a network call.
pub fn preview(pending: &PendingAlert, cfg: &DiscordConfig) -> Value {
    let alert = &pending.payload;
    let display_signal = match alert.kind.as_str() {
        "displayed_price_observed" => Some("Displayed resale price observed"),
        "displayed_price_changed" => Some("Displayed resale price changed"),
        "displayed_details_changed" => Some("Displayed seat details changed"),
        _ => None,
    };
    let display_only = display_signal.is_some();
    let (content, users, roles) = mentions(cfg);
    let mut fields = Vec::new();
    for (index, offer) in alert.offers.iter().take(MAX_OFFERS).enumerate() {
        let label = if display_only { "Seat price" } else { "Offer" };
        let name = match offer.section.as_deref() {
            Some(section) => format!("{label} {} · {}", index + 1, clean(section, 120)),
            None => format!("{label} {}", index + 1),
        };
        let mut lines = Vec::new();
        for (label, value) in [
            ("Type", offer.ticket_type.as_deref()),
            (
                if display_only {
                    "Restriction"
                } else {
                    "Inventory"
                },
                offer.inventory.as_deref(),
            ),
            ("Row", offer.row.as_deref()),
            ("Seats", offer.seats.as_deref()),
            ("Price as shown", offer.price_text.as_deref()),
            ("Currency", offer.currency.as_deref()),
        ] {
            if let Some(value) = value.filter(|s| !s.trim().is_empty()) {
                lines.push(format!("{label}: {}", clean(value, 110)));
            }
        }
        if display_only {
            lines.push("Available quantity: not verified".into());
        } else if let Some(quantity) = offer.quantity {
            lines.push(match offer.quantity_kind {
                QuantityKind::Listing => format!("Tickets in this listing: {quantity}"),
                QuantityKind::Remaining => format!("Reported remaining for this offer: {quantity}"),
                QuantityKind::Selectable => {
                    format!("Selectable order quantity: {quantity} (not remaining stock)")
                }
                QuantityKind::Unknown => format!("Quantity shown: {quantity} (meaning unknown)"),
            });
        } else {
            lines.push("Quantity: not reported".into());
        }
        lines.push(
            match offer.price_scope {
                PriceScope::PerTicket => "Price basis: per ticket",
                PriceScope::Total => "Price basis: total offer price",
                PriceScope::Unknown => "Price basis: not reported",
            }
            .into(),
        );
        lines.push(if display_only {
            "Complete fees: not verified".into()
        } else {
            match offer.fees {
                FeeScope::Included => "Fees: included in the displayed price",
                FeeScope::Excluded => "Fees: extra",
                FeeScope::Unknown => "Fees: not reported",
            }
            .into()
        });
        if display_only {
            lines.push("Availability and adjacency: not verified".into());
        }
        fields.push(
            json!({"name": name, "value": truncate(&lines.join("\n"), 600), "inline": false}),
        );
    }
    if alert.offers.len() > MAX_OFFERS {
        fields.push(if display_only {
            json!({"name": "Additional displayed seats", "value": format!("{} more observations; open the event for current details.", alert.offers.len() - MAX_OFFERS), "inline": false})
        } else {
            json!({"name": "Additional offers", "value": format!("{} more matches; open the event for the current listing.", alert.offers.len() - MAX_OFFERS), "inline": false})
        });
    }
    let description = format!(
        "{}\n{}\nSignal: {}\n\n{}",
        clean(&alert.artist, 150),
        clean(&alert.location, 200),
        clean(display_signal.unwrap_or(&alert.kind), 80),
        clean(&alert.detail, 650)
    );
    let mut embed = json!({
        "title": if display_only { format!("Displayed resale price · {}", clean(&alert.event_name, 220)) } else { clean(&alert.event_name, 250) },
        "description": description,
        "color": 3447003,
        "fields": fields,
        "footer": {"text": if display_only { "Displayed prices only. Availability, quantity, complete fees and adjacency are unverified. Tickets are not reserved." } else { "Observed offers; tickets are not reserved. Total event stock is not reported; offer quantities are not summed." }},
    });
    if let Ok(url) = crate::config::validate_public_url(&alert.event_url) {
        embed["url"] = Value::String(url.to_string());
    }
    if let Some(at) = chrono::DateTime::from_timestamp(alert.observed_at, 0) {
        embed["timestamp"] = Value::String(at.to_rfc3339());
    }
    json!({
        "content": content,
        "embeds": [embed],
        "allowed_mentions": {"parse": [], "users": users, "roles": roles, "replied_user": false},
        "nonce": nonce(&pending.nonce),
        "enforce_nonce": true,
    })
}

fn mentions(cfg: &DiscordConfig) -> (String, Vec<String>, Vec<String>) {
    let mut content = String::new();
    let mut users = Vec::new();
    let mut roles = Vec::new();
    let mut seen = HashSet::new();
    let mut omitted = false;
    for (kind, list) in [
        (false, &cfg.mention_user_ids),
        (true, &cfg.mention_role_ids),
    ] {
        for id in list {
            if !is_snowflake(id) || !seen.insert((kind, id.clone())) {
                continue;
            }
            let target = if kind { &mut roles } else { &mut users };
            let mention = if kind {
                format!("<@&{id}> ")
            } else {
                format!("<@{id}> ")
            };
            if target.len() >= 100 || content.len() + mention.len() > 1800 {
                omitted = true;
                continue;
            }
            content.push_str(&mention);
            target.push(id.clone());
        }
    }
    if omitted {
        content.push_str("Some mention targets omitted because of Discord message limits.");
    }
    (content.trim_end().into(), users, roles)
}

fn is_snowflake(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|n| n > 0)
}

fn nonce(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..24].to_owned()
}

fn clean(value: &str, limit: usize) -> String {
    let mut out = String::new();
    for ch in value.chars().take(limit.saturating_mul(2)) {
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        if "\\*_~`|>[]()#!".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
        if ch == '@' {
            out.push('\u{200b}');
        }
    }
    truncate(&out, limit)
}

fn truncate(value: &str, limit: usize) -> String {
    if value.encode_utf16().count() <= limit {
        return value.into();
    }
    let mut used = 0;
    let mut out = String::new();
    for ch in value.chars() {
        if used + ch.len_utf16() >= limit {
            break;
        }
        used += ch.len_utf16();
        out.push(ch);
    }
    out.push('…');
    out
}

fn finite_duration(seconds: f64) -> Option<Duration> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Duration::try_from_secs_f64(seconds.max(0.001)).ok()
}

fn header_seconds(headers: &header::HeaderMap, key: &str) -> Option<Duration> {
    headers
        .get(key)?
        .to_str()
        .ok()?
        .parse::<f64>()
        .ok()
        .and_then(finite_duration)
}

fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?;
    if let Some(seconds) = value.parse::<f64>().ok().and_then(finite_duration) {
        return Some(seconds);
    }
    let until = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(Duration::from_secs(
        (until.timestamp() - chrono::Utc::now().timestamp()).max(1) as u64,
    ))
}

fn ceil_seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0))
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Alert, Offer};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn config() -> DiscordConfig {
        DiscordConfig {
            channel_id: Some("123456789012345678".into()),
            token_env: "TICKET_TEST_UNUSED_TOKEN".into(),
            mention_user_ids: vec!["111111111111111111".into()],
            mention_role_ids: vec!["222222222222222222".into()],
        }
    }

    fn pending() -> PendingAlert {
        PendingAlert {
            id: 1,
            nonce: "unique-outbox-instance-1".into(),
            attempts: 0,
            payload: Alert {
                watch_id: "korn-pl".into(),
                event_name: "KoRn".into(),
                artist: "KoRn".into(),
                location: "Krakow, PL".into(),
                event_url: "https://www.ticketmaster.pl/event/korn-bilety/496543017".into(),
                observed_at: 1794220800,
                kind: "availability_observed".into(),
                detail: "A matching offer appeared".into(),
                offers: vec![Offer {
                    key: "row-6".into(),
                    section: Some("TR-A9".into()),
                    row: Some("6".into()),
                    seats: None,
                    ticket_type: Some("Official Platinum".into()),
                    inventory: None,
                    price_text: Some("910,80 PLN".into()),
                    price_minor: Some(91080),
                    currency: Some("PLN".into()),
                    quantity: Some(2),
                    quantity_kind: QuantityKind::Remaining,
                    price_scope: PriceScope::PerTicket,
                    ..Offer::default()
                }],
            },
        }
    }

    #[test]
    fn payload_has_only_explicit_mentions_and_preserves_source_uncertainty() {
        let mut pending = pending();
        pending.payload.event_name = "@everyone <@999999999999999999> **KoRn**".into();
        let payload = preview(&pending, &config());
        assert_eq!(payload["allowed_mentions"]["parse"], json!([]));
        assert_eq!(
            payload["allowed_mentions"]["users"],
            json!(["111111111111111111"])
        );
        assert!(
            !payload["content"]
                .as_str()
                .unwrap()
                .contains("999999999999999999")
        );
        assert!(
            !payload["embeds"][0]["title"]
                .as_str()
                .unwrap()
                .contains("@everyone")
        );
        let offer = payload["embeds"][0]["fields"][0]["value"].as_str().unwrap();
        assert!(offer.contains("Reported remaining for this offer: 2"));
        assert!(offer.contains("910,80 PLN"));
        assert!(!offer.contains("resale"));
        assert!(
            payload["embeds"][0]["footer"]["text"]
                .as_str()
                .unwrap()
                .contains("not reserved")
        );
    }

    #[test]
    fn displayed_price_alerts_keep_stock_and_complete_fees_unverified() {
        let mut pending = pending();
        pending.payload.event_name = "KoRn @everyone".into();
        pending.payload.event_url = "https://www.ticketmaster.cz/event/korn/1472650657".into();
        pending.payload.offers[0].inventory = Some("Omezený výhled".into());
        pending.payload.offers[0].price_text = Some("2 438,00 Kč za kus".into());
        pending.payload.offers[0].currency = Some("CZK".into());
        // Display-only kinds cannot accidentally promote these legacy fields.
        pending.payload.offers[0].quantity = Some(7);
        pending.payload.offers[0].quantity_kind = QuantityKind::Remaining;
        pending.payload.offers[0].fees = FeeScope::Included;
        for (kind, signal) in [
            (
                "displayed_price_observed",
                "Displayed resale price observed",
            ),
            ("displayed_price_changed", "Displayed resale price changed"),
            (
                "displayed_details_changed",
                "Displayed seat details changed",
            ),
        ] {
            pending.payload.kind = kind.into();
            let payload = preview(&pending, &config());
            let embed = &payload["embeds"][0];
            assert!(
                embed["title"]
                    .as_str()
                    .unwrap()
                    .starts_with("Displayed resale price")
            );
            assert!(!embed["title"].as_str().unwrap().contains("@everyone"));
            assert!(embed["description"].as_str().unwrap().contains(signal));
            assert!(
                embed["fields"][0]["name"]
                    .as_str()
                    .unwrap()
                    .starts_with("Seat price")
            );
            let row = embed["fields"][0]["value"].as_str().unwrap();
            assert!(row.contains("Restriction: Omezený výhled"));
            assert!(row.contains("2 438,00 Kč za kus"));
            assert!(row.contains("Currency: CZK"));
            assert!(row.contains("Price basis: per ticket"));
            assert!(row.contains("Available quantity: not verified"));
            assert!(row.contains("Complete fees: not verified"));
            assert!(!row.contains("Reported remaining") && !row.contains("Fees: included"));
            assert!(
                embed["footer"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("adjacency are unverified")
            );
            assert_eq!(payload["allowed_mentions"]["parse"], json!([]));
            assert_eq!(
                payload["allowed_mentions"]["users"],
                json!(["111111111111111111"])
            );
            assert_eq!(payload["enforce_nonce"], true);
            assert_eq!(payload["nonce"], preview(&pending, &config())["nonce"]);
        }
    }

    #[test]
    fn selectable_quantities_and_total_prices_are_labelled_without_stock_totals() {
        let mut pending = pending();
        pending.payload.offers[0].quantity_kind = QuantityKind::Selectable;
        pending.payload.offers[0].price_scope = PriceScope::Total;
        pending.payload.offers[0].fees = FeeScope::Excluded;
        let payload = preview(&pending, &config());
        let row = payload["embeds"][0]["fields"][0]["value"].as_str().unwrap();
        assert!(row.contains("Selectable order quantity: 2 (not remaining stock)"));
        assert!(row.contains("Price basis: total offer price"));
        assert!(row.contains("Fees: extra"));
        assert!(!row.contains("Reported remaining"));
        assert!(
            payload["embeds"][0]["footer"]["text"]
                .as_str()
                .unwrap()
                .contains("not summed")
        );
    }

    #[test]
    fn payload_fits_discord_limits_with_large_unicode_fields_and_mentions() {
        let mut pending = pending();
        let huge = "🦀@everyone**".repeat(3000);
        pending.payload.event_name = huge.clone();
        pending.payload.artist = huge.clone();
        pending.payload.location = huge.clone();
        pending.payload.detail = huge.clone();
        let mut offer = pending.payload.offers[0].clone();
        offer.section = Some(huge.clone());
        offer.row = Some(huge.clone());
        offer.price_text = Some(huge.clone());
        offer.seats = Some(huge.clone());
        offer.ticket_type = Some(huge.clone());
        offer.inventory = Some(huge);
        pending.payload.offers = vec![offer; 100];
        let mut cfg = config();
        cfg.mention_user_ids = (1..=300)
            .map(|n| (1000000000000000000_u64 + n).to_string())
            .collect();
        let payload = preview(&pending, &cfg);
        let embed = &payload["embeds"][0];
        let len = |value: &Value| value.as_str().unwrap().encode_utf16().count();
        assert!(len(&payload["content"]) <= 2000);
        assert!(len(&embed["title"]) <= 256);
        assert!(len(&embed["description"]) <= 4096);
        assert!(
            payload["allowed_mentions"]["users"]
                .as_array()
                .unwrap()
                .len()
                <= 100
        );
        let fields = embed["fields"].as_array().unwrap();
        assert!(fields.len() <= 25);
        let mut aggregate =
            len(&embed["title"]) + len(&embed["description"]) + len(&embed["footer"]["text"]);
        for field in fields {
            assert!(len(&field["name"]) <= 256);
            assert!(len(&field["value"]) <= 1024);
            aggregate += len(&field["name"]) + len(&field["value"]);
        }
        assert!(aggregate <= 6000, "aggregate {aggregate}");
    }

    #[test]
    fn nonce_is_stable_short_and_distinguishes_long_shared_prefixes() {
        let first = nonce(&format!("{}1", "a".repeat(100)));
        let second = nonce(&format!("{}2", "a".repeat(100)));
        assert_eq!(first.len(), 24);
        assert_ne!(first, second);
        assert_eq!(first, nonce(&format!("{}1", "a".repeat(100))));
        assert_eq!(preview(&pending(), &config())["enforce_nonce"], true);
    }

    #[test]
    fn private_checkout_links_are_never_embedded() {
        let mut pending = pending();
        pending.payload.event_url =
            "https://secure.ticketmaster.pl/payment/?t=private-session".into();
        assert!(
            preview(&pending, &config())["embeds"][0]
                .get("url")
                .is_none()
        );
    }

    #[test]
    fn retry_values_are_conservative_and_validate_floats() {
        assert_eq!(ceil_seconds(Duration::from_millis(1500)), 2);
        assert!(finite_duration(f64::NAN).is_none());
        assert!(finite_duration(-1.0).is_none());
        let mut headers = header::HeaderMap::new();
        headers.insert(header::RETRY_AFTER, "3.25".parse().unwrap());
        assert_eq!(retry_after(&headers).unwrap(), Duration::from_millis(3250));
    }

    async fn mock(
        status: &str,
        headers: &str,
        body: &str,
    ) -> (Discord, tokio::task::JoinHandle<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let body_start;
            let body_len;
            loop {
                let mut chunk = [0u8; 4096];
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    body_start = index + 4;
                    let header_text = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                    body_len = header_text
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break;
                }
            }
            while bytes.len() < body_start + body_len {
                let mut chunk = [0u8; 4096];
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let payload =
                serde_json::from_slice(&bytes[body_start..body_start + body_len]).unwrap();
            stream.write_all(response.as_bytes()).await.unwrap();
            payload
        });
        let discord = Discord::for_test(&config(), format!("http://{addr}/messages"));
        (discord, task)
    }

    #[tokio::test]
    async fn global_429_cooldown_prevents_another_network_attempt() {
        let (mut discord, task) = mock(
            "429 Too Many Requests",
            "Retry-After: 4\r\n",
            r#"{"retry_after":2.5,"global":true}"#,
        )
        .await;
        let result = discord.send(&pending()).await.unwrap();
        assert!(matches!(result, Delivery::Retry { after_secs: 4, .. }));
        let again = discord.send(&pending()).await.unwrap();
        assert!(matches!(again, Delivery::Retry { .. }));
        let payload = task.await.unwrap();
        assert_eq!(payload["nonce"], nonce(&pending().nonce));
    }

    #[tokio::test]
    async fn success_records_message_id_and_exhausted_bucket_blocks_next_send() {
        let (mut discord, task) = mock(
            "200 OK",
            "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 3.5\r\n",
            r#"{"id":"123456789012345678"}"#,
        )
        .await;
        assert_eq!(discord.cooldown_secs(), None);
        assert_eq!(
            discord.send(&pending()).await.unwrap(),
            Delivery::Sent {
                message_id: "123456789012345678".into()
            }
        );
        assert_eq!(discord.cooldown_secs(), Some(4));
        assert!(matches!(
            discord.send(&pending()).await.unwrap(),
            Delivery::Retry { after_secs: 4, .. }
        ));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn permission_failure_is_permanent_and_body_secrets_are_not_echoed() {
        let (mut discord, task) = mock(
            "403 Forbidden",
            "",
            r#"{"message":"SECRET_SHOULD_NOT_ESCAPE"}"#,
        )
        .await;
        let delivery = discord.send(&pending()).await.unwrap();
        assert!(matches!(delivery, Delivery::Permanent { .. }));
        assert!(!format!("{delivery:?}").contains("SECRET"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn unconfirmed_success_is_retryable_without_changing_nonce() {
        let (mut discord, task) = mock("200 OK", "", r#"{}"#).await;
        assert!(matches!(
            discord.send(&pending()).await.unwrap(),
            Delivery::Retry { .. }
        ));
        let payload = task.await.unwrap();
        assert_eq!(payload["nonce"], nonce(&pending().nonce));
    }
}
