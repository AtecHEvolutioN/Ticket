//! Public Discovery metadata only. This adapter does not establish ticket inventory.
//!
//! The API key is held only in memory, sent only to the fixed HTTPS API origin,
//! and deliberately excluded from errors, returned data, and Debug implementations.
use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, StatusCode, header::HeaderMap, redirect::Policy};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::{Instant, sleep_until};
use url::Url;

const API_ROOT: &str = "https://app.ticketmaster.com/discovery/v2/";
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(500);
const METADATA_NOTICE: &str = "Discovery returns event metadata and sale schedules. Event status, including onsale, does not confirm available tickets, seats, quantities, or prices.";

pub struct Discovery {
    api_key: String,
    client: Client,
    base_url: Url,
    next_request: Instant,
    provider_cooldown: Option<Instant>,
    blocked_reason: Option<&'static str>,
}

impl Discovery {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("TICKETMASTER_API_KEY")
            .map_err(|_| anyhow!("Set TICKETMASTER_API_KEY in the environment; do not put the key in command arguments"))?;
        if api_key.is_empty() || api_key.len() > 512 || api_key.chars().any(char::is_whitespace) {
            bail!("TICKETMASTER_API_KEY must be a nonempty key without whitespace");
        }
        let client = Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .redirect(Policy::none())
            .user_agent("TicketWatch/0.1 (Ticketmaster Discovery metadata)")
            .build()
            .map_err(|_| anyhow!("Could not initialize Discovery HTTPS client"))?;
        Ok(Self {
            api_key,
            client,
            base_url: Url::parse(API_ROOT).expect("fixed valid API URL"),
            next_request: Instant::now(),
            provider_cooldown: None,
            blocked_reason: None,
        })
    }

    /// Provider backoff deadline, rounded up to UTC seconds for durable scheduling.
    /// Ordinary 500ms client pacing is deliberately excluded.
    pub fn cooldown_until(&self) -> Option<i64> {
        let remaining = self
            .provider_cooldown?
            .checked_duration_since(Instant::now())?;
        if remaining.is_zero() {
            return None;
        }
        let now = chrono::Utc::now();
        let fractional_nanos =
            now.timestamp_subsec_nanos() as u64 + remaining.subsec_nanos() as u64;
        let extra_seconds = fractional_nanos.div_ceil(1_000_000_000) as i64;
        Some(
            now.timestamp()
                .saturating_add(remaining.as_secs().min(i64::MAX as u64) as i64)
                .saturating_add(extra_seconds),
        )
    }

    /// An authentication failure or unusable rate-limit deadline needs operator attention.
    pub fn is_paused(&self) -> bool {
        self.blocked_reason.is_some()
    }

    /// One bounded artist-search page. IDs must be selected explicitly by the user.
    pub async fn artists(&mut self, query: &str) -> Result<Value> {
        self.artists_page(query, 0).await
    }

    pub async fn artists_page(&mut self, query: &str, page: u32) -> Result<Value> {
        if page >= 50 {
            bail!(
                "Artist page must be 0–49; Discovery deep paging is limited to the first 1,000 results"
            );
        }
        let query = input_text(query, "Artist search", 200)?;
        let raw = self
            .request(
                "attractions.json",
                &[
                    ("keyword", query.clone()),
                    ("size", "20".into()),
                    ("page", page.to_string()),
                ],
            )
            .await?;
        let artists = embedded_items(&raw, "attractions")?
            .iter()
            .take(20)
            .map(|item| {
                json!({
                    "id": text(item.get("id"), 160),
                    "name": text(item.get("name"), 400),
                    "url": public_url(item.get("url")),
                })
            })
            .collect::<Vec<_>>();
        let mut output = envelope(&raw);
        output["kind"] = json!("artist_search");
        output["query"] = json!(query);
        output["artists"] = json!(artists);
        Ok(output)
    }

    /// One bounded event-search page, scoped to Ticketmaster metadata.
    /// No returned event is automatically activated as a website watch.
    pub async fn events(
        &mut self,
        attraction_id: &str,
        country: Option<&str>,
        city: Option<&str>,
    ) -> Result<Value> {
        self.events_page(attraction_id, country, city, 0).await
    }

    pub async fn events_page(
        &mut self,
        attraction_id: &str,
        country: Option<&str>,
        city: Option<&str>,
        page: u32,
    ) -> Result<Value> {
        self.events_query_page(Some(attraction_id), country, city, page, None)
            .await
    }

    /// One bounded country-wide catalog page, without an artist filter.
    /// The country's whole catalog can exceed Discovery's deep-paging ceiling.
    pub async fn events_country_page(
        &mut self,
        country: &str,
        city: Option<&str>,
        page: u32,
    ) -> Result<Value> {
        self.events_query_page(None, Some(country), city, page, None)
            .await
    }

    /// One UTC date-window page. Provider paging/quotas still apply to each
    /// window. Missing/TBA dates and dates outside the window are not covered.
    pub async fn events_country_window_page(
        &mut self,
        country: &str,
        city: Option<&str>,
        page: u32,
        start: i64,
        end: i64,
    ) -> Result<Value> {
        if start > end {
            bail!("Country discovery date window must be ordered");
        }
        let format = |timestamp| {
            chrono::DateTime::from_timestamp(timestamp, 0)
                .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                .ok_or_else(|| anyhow!("Country discovery date window is outside supported dates"))
        };
        // The provider describes these filters as "after"/"before" without
        // promising inclusivity. Overlap by a second so exact split/day
        // boundaries cannot fall between strict wire comparisons. Catalog
        // merging deduplicates events returned by adjacent windows.
        let query_start = format(
            start
                .checked_sub(1)
                .context("Country date window underflow")?,
        )?;
        let query_end = format(end.checked_add(1).context("Country date window overflow")?)?;
        let mut output = self
            .events_query_page(
                None,
                Some(country),
                city,
                page,
                Some((query_start.clone(), query_end.clone())),
            )
            .await?;
        output["date_window"] = json!({"start":format(start)?,"end":format(end)?,
            "request_start":query_start,"request_end":query_end,
            "include_tba":false,"include_tbd":false});
        Ok(output)
    }

    async fn events_query_page(
        &mut self,
        attraction_id: Option<&str>,
        country: Option<&str>,
        city: Option<&str>,
        page: u32,
        window: Option<(String, String)>,
    ) -> Result<Value> {
        if page >= 10 {
            bail!(
                "Event page must be 0–9; Discovery deep paging is limited to the first 1,000 results"
            );
        }
        let attraction_id = attraction_id
            .map(|value| -> Result<String> {
                let id = input_text(value, "Attraction ID", 160)?;
                if !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                {
                    bail!(
                        "Attraction ID must contain only letters, digits, hyphens, or underscores"
                    );
                }
                Ok(id)
            })
            .transpose()?;
        let mut params = vec![
            ("source", "ticketmaster".into()),
            ("size", "100".into()),
            ("page", page.to_string()),
        ];
        if let Some(id) = &attraction_id {
            params.push(("attractionId", id.clone()));
        } else {
            if country.is_none() {
                bail!("Country-wide event discovery requires an explicit country");
            }
            // Stable date ordering keeps a bounded scan useful without implying
            // that events beyond the provider's first 1,000 results were read.
            params.push(("sort", "date,asc".into()));
        }
        if let Some(country) = country {
            if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
                bail!("Country must be a two-letter uppercase code such as PL");
            }
            params.push(("countryCode", country.into()));
        }
        if let Some(city) = city {
            params.push(("city", input_text(city, "City", 200)?));
        }
        if let Some((start, end)) = &window {
            params.push(("startDateTime", start.clone()));
            params.push(("endDateTime", end.clone()));
            params.push(("includeTBA", "no".into()));
            params.push(("includeTBD", "no".into()));
        }
        let raw = self.request("events.json", &params).await?;
        let items = embedded_items(&raw, "events")?;
        if items.len() > 100 {
            bail!(
                "Discovery event page exceeds the requested 100-item bound; completeness is unknown"
            );
        }
        let events = items.iter().map(normalize_event).collect::<Vec<_>>();
        let mut output = envelope(&raw);
        output["kind"] = json!("event_search");
        output["selection_scope"] = json!(if attraction_id.is_some() {
            "artist"
        } else {
            "country"
        });
        output["attraction_id"] = json!(attraction_id);
        output["country"] = json!(country);
        output["city"] = json!(city);
        if let Some((start, end)) = window {
            output["date_window"] = json!({"start": start, "end": end});
        }
        output["events"] = json!(events);
        Ok(output)
    }

    async fn request(&mut self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        if let Some(reason) = self.blocked_reason {
            bail!(
                "Discovery client paused: {reason}; verify account access or the quota reset before starting another client"
            );
        }
        sleep_until(self.next_request).await;
        self.next_request = Instant::now() + MIN_REQUEST_INTERVAL;
        let endpoint = self.base_url.join(path).expect("fixed relative API path");
        let mut response = self
            .client
            .get(endpoint)
            .query(params)
            .query(&[("apikey", self.api_key.as_str())])
            .send()
            .await
            .map_err(redacted_transport_error)?;
        let status = response.status();
        let retry_after = retry_after_seconds(response.headers());
        if let Some(seconds) = retry_after.filter(|seconds| *seconds > 0) {
            self.defer(Duration::from_secs(seconds));
        }
        let quota_remaining = numeric_header(response.headers(), "rate-limit-available");
        let mut quota_reset_known = false;
        if quota_remaining == Some(0)
            && let Some(reset_millis) = numeric_header(response.headers(), "rate-limit-reset")
        {
            let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
            if reset_millis > now {
                self.defer(Duration::from_millis(reset_millis - now));
                quota_reset_known = true;
            }
        }
        if (status == StatusCode::TOO_MANY_REQUESTS || quota_remaining == Some(0))
            && retry_after.is_none_or(|seconds| seconds == 0)
            && !quota_reset_known
        {
            self.blocked_reason = Some("rate limited and no usable retry/reset time was supplied");
        }
        if !status.is_success() {
            // Error bodies and reqwest URL-bearing errors may contain the API key. Never include them.
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                self.blocked_reason = Some("authentication or API access was rejected");
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                bail!(
                    "Discovery HTTP 429: rate limited; retry_after_seconds={}; quota_remaining={}. No automatic retry was made",
                    retry_after.map_or_else(|| "unknown".into(), |v| v.to_string()),
                    quota_remaining.map_or_else(|| "unknown".into(), |v| v.to_string())
                );
            }
            bail!(
                "Discovery HTTP {}: request failed; response details omitted to protect credentials",
                status.as_u16()
            );
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BODY_BYTES as u64)
        {
            bail!("Discovery response exceeds the 4 MiB limit");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(redacted_transport_error)? {
            if chunk.len() > MAX_BODY_BYTES.saturating_sub(body.len()) {
                bail!("Discovery response exceeds the 4 MiB limit");
            }
            body.extend_from_slice(&chunk);
        }
        let raw: Value = serde_json::from_slice(&body).map_err(|_| {
            anyhow!(
                "Discovery returned invalid JSON; response details omitted to protect credentials"
            )
        })?;
        if !raw.is_object() || raw.get("fault").is_some() || raw.get("errors").is_some() {
            bail!(
                "Discovery returned an unexpected response; response details omitted to protect credentials"
            );
        }
        Ok(raw)
    }

    fn defer(&mut self, duration: Duration) {
        match Instant::now().checked_add(duration) {
            Some(deadline) => {
                self.next_request = self.next_request.max(deadline);
                self.provider_cooldown = Some(
                    self.provider_cooldown
                        .map_or(deadline, |old| old.max(deadline)),
                );
            }
            None => {
                self.blocked_reason =
                    Some("the supplied rate-limit deadline cannot be represented safely")
            }
        }
    }
}

fn input_text(input: &str, label: &str, limit: usize) -> Result<String> {
    let input = input.trim();
    if input.is_empty() || input.chars().count() > limit || input.chars().any(char::is_control) {
        bail!(
            "{label} must be nonempty, contain no control characters, and be at most {limit} characters"
        );
    }
    Ok(input.into())
}

fn redacted_transport_error(error: reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow!("Discovery request timed out; request URL and credentials omitted")
    } else if error.is_connect() {
        anyhow!("Could not connect to Discovery; request URL and credentials omitted")
    } else {
        anyhow!("Discovery transport failed; request URL and credentials omitted")
    }
}

fn numeric_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

fn retry_after_seconds(headers: &HeaderMap) -> Option<u64> {
    let raw = headers.get("retry-after")?.to_str().ok()?;
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(seconds);
    }
    if let Ok(seconds) = raw.parse::<f64>()
        && seconds.is_finite()
        && seconds >= 0.0
        && seconds <= u64::MAX as f64
    {
        return Some(seconds.ceil() as u64);
    }
    chrono::DateTime::parse_from_rfc2822(raw)
        .ok()
        .map(|date| (date.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64)
}

fn embedded_items<'a>(raw: &'a Value, key: &str) -> Result<&'a [Value]> {
    if let Some(items) = raw.get("_embedded").and_then(|v| v.get(key)) {
        return items
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("Discovery returned malformed result items"));
    }
    if raw
        .get("page")
        .and_then(|p| p.get("totalElements"))
        .and_then(Value::as_u64)
        == Some(0)
    {
        return Ok(&[]);
    }
    bail!("Discovery response is missing result items; completeness is unknown")
}

fn text(value: Option<&Value>, max_chars: usize) -> Value {
    value
        .and_then(Value::as_str)
        .map(|s| {
            Value::String(
                s.chars()
                    .filter(|c| !c.is_control())
                    .take(max_chars)
                    .collect(),
            )
        })
        .unwrap_or(Value::Null)
}

fn public_url(value: Option<&Value>) -> Value {
    let Some(raw) = value.and_then(Value::as_str) else {
        return Value::Null;
    };
    let Ok(mut url) = Url::parse(raw) else {
        return Value::Null;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Value::Null;
    }
    if url.query_pairs().any(|(key, _)| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "apikey" | "api_key" | "token" | "session" | "sessionid" | "jwt" | "t"
        )
    }) {
        return Value::Null;
    }
    url.set_fragment(None);
    let url: String = url.into();
    if url.len() > 2048 {
        Value::Null
    } else {
        Value::String(url)
    }
}

fn selected_fields(value: Option<&Value>, fields: &[&str]) -> Value {
    let Some(value) = value.and_then(Value::as_object) else {
        return Value::Null;
    };
    let mut output = serde_json::Map::new();
    for name in fields {
        if let Some(field) = value.get(*name) {
            if field.is_string() {
                output.insert((*name).into(), text(Some(field), 400));
            } else if field.is_boolean() || field.is_number() {
                output.insert((*name).into(), field.clone());
            }
        }
    }
    Value::Object(output)
}

fn public_event_url(value: Option<&Value>) -> Value {
    // Reject secret-bearing URLs before removing ordinary tracking parameters.
    let safe = public_url(value);
    let Some(raw) = safe.as_str() else {
        return Value::Null;
    };
    let Ok(mut url) = Url::parse(raw) else {
        return Value::Null;
    };
    url.set_query(None);
    url.set_fragment(None);
    if !url.path().contains("/event/") || crate::config::validate_public_url(url.as_str()).is_err()
    {
        return Value::Null;
    }
    Value::String(url.into())
}

fn normalize_event(event: &Value) -> Value {
    let attraction_ids = event
        .pointer("/_embedded/attractions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(100)
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 160
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        })
        .collect::<std::collections::BTreeSet<_>>();
    let attractions = event
        .pointer("/_embedded/attractions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(100)
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?;
            attraction_ids
                .contains(id)
                .then(|| json!({"id": id, "name": text(item.get("name"), 400)}))
        })
        .collect::<Vec<_>>();
    let price_ranges = event
        .get("priceRanges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(50)
        .filter_map(|range| {
            let currency = range.get("currency")?.as_str()?.to_ascii_uppercase();
            if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
                return None;
            }
            let price = |name: &str| {
                range
                    .get(name)
                    .filter(|value| value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0))
            };
            let min = price("min");
            let max = price("max");
            if (min.is_none() && max.is_none())
                || min
                    .zip(max)
                    .is_some_and(|(min, max)| min.as_f64() > max.as_f64())
            {
                return None;
            }
            Some(
                json!({"type":text(range.get("type"),100), "currency":currency,
                "min":min, "max":max}),
            )
        })
        .collect::<Vec<_>>();
    let dates = event.get("dates");
    let status = dates
        .and_then(|d| d.get("status"))
        .and_then(|s| s.get("code"));
    let sales = event.get("sales");
    let presales = sales
        .and_then(|s| s.get("presales"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(100)
                .map(|sale| {
                    let mut normalized = selected_fields(
                        Some(sale),
                        &["name", "description", "startDateTime", "endDateTime"],
                    );
                    normalized["url"] = public_url(sale.get("url"));
                    normalized
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let venues = event
        .get("_embedded")
        .and_then(|e| e.get("venues"))
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().take(20).map(|venue| json!({
            "id": text(venue.get("id"), 160), "name": text(venue.get("name"), 400),
            "url": public_url(venue.get("url")), "timezone": text(venue.get("timezone"), 100),
            "city": text(venue.get("city").and_then(|c| c.get("name")), 200),
            "country": selected_fields(venue.get("country"), &["name", "countryCode"]),
        })).collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "id": text(event.get("id"), 160), "name": text(event.get("name"), 400),
        "attraction_ids": attraction_ids,
        "attractions": attractions,
        "url": public_event_url(event.get("url")), "event_status": text(status, 100),
        "inventory_verified": false,
        "price_ranges": price_ranges,
        "price_ranges_notice": "Discovery catalog price ranges are metadata. They do not confirm currently purchasable ticket prices, quantities, or included fees.",
        "dates": {
            "start": selected_fields(dates.and_then(|d| d.get("start")), &["localDate", "localTime", "dateTime", "dateTBD", "dateTBA", "timeTBA", "noSpecificTime"]),
            "end": selected_fields(dates.and_then(|d| d.get("end")), &["localDate", "localTime", "dateTime", "approximate", "noSpecificTime"]),
            "timezone": text(dates.and_then(|d| d.get("timezone")), 100),
        },
        "sales": {
            "public": selected_fields(sales.and_then(|s| s.get("public")), &["startDateTime", "endDateTime", "startTBD"]),
            "presales": presales,
        },
        "venues": venues,
    })
}

fn envelope(raw: &Value) -> Value {
    let page = raw.get("page");
    let field = |name: &str| page.and_then(|p| p.get(name)).and_then(Value::as_u64);
    let number = field("number");
    let size = field("size");
    let total_pages = field("totalPages");
    let has_more = number
        .zip(total_pages)
        .map(|(number, total)| number.saturating_add(1) < total)
        .or_else(|| {
            raw.get("_links")
                .and_then(|links| links.get("next"))
                .map(|_| true)
        });
    let candidate_page = has_more
        .filter(|more| *more)
        .and_then(|_| number.and_then(|n| n.checked_add(1)));
    let next_page = candidate_page.filter(|next| {
        size.and_then(|size| next.checked_mul(size))
            .is_some_and(|start| start < 1000)
    });
    let deep_paging_limit_reached =
        candidate_page.is_some() && size.is_some() && next_page.is_none();
    json!({
        "source": "ticketmaster_discovery",
        "inventory_verified": false,
        "metadata_notice": METADATA_NOTICE,
        "page": {"number": number, "size": size, "total_elements": field("totalElements"), "total_pages": total_pages},
        "has_more": has_more,
        "next_page": next_page,
        "deep_paging_limit_reached": deep_paging_limit_reached,
        "pagination_notice": "Only this page is returned. Request next_page explicitly, or narrow country/city/artist filters. Deep paging is limited to the first 1,000 results; this is not an exhaustive worldwide event list.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
    };

    const FAKE_KEY: &str = "test-secret+with&reserved=characters";

    async fn mock(
        status: &str,
        headers: &str,
        body: &str,
    ) -> (Discovery, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = oneshot::channel();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        );
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
            }
            let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let discovery = Discovery {
            api_key: FAKE_KEY.into(),
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .redirect(Policy::none())
                .build()
                .unwrap(),
            base_url: Url::parse(&format!("http://{address}/discovery/v2/")).unwrap(),
            next_request: Instant::now(),
            provider_cooldown: None,
            blocked_reason: None,
        };
        (discovery, receiver)
    }

    fn request_params(request: &str) -> BTreeMap<String, String> {
        let target = request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        Url::parse(&format!("http://localhost{target}"))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn artist_query_is_encoded_and_output_omits_internal_fields() {
        let body = json!({"_embedded":{"attractions":[{"id":"K8Artist", "name":"KoRn & Friends", "url":"https://www.ticketmaster.pl/artist/775700", "internal":"not for output"}]},"page":{"number":0,"size":20,"totalElements":25,"totalPages":2}}).to_string();
        let (mut client, request) = mock("200 OK", "", &body).await;
        let output = client.artists("KoRn & Friends").await.unwrap();
        let params = request_params(&request.await.unwrap());
        assert_eq!(params["keyword"], "KoRn & Friends");
        assert_eq!(params["apikey"], FAKE_KEY);
        assert_eq!(params["size"], "20");
        assert_eq!(output["artists"][0]["id"], "K8Artist");
        assert_eq!(output["has_more"], true);
        assert_eq!(output["next_page"], 1);
        assert_eq!(output["inventory_verified"], false);
        assert!(!output.to_string().contains(FAKE_KEY));
        assert!(output["artists"][0].get("internal").is_none());
    }

    #[tokio::test]
    async fn event_query_filters_and_normalizes_metadata_without_inventory_claims() {
        let body = json!({"_embedded":{"events":[{
            "id":"event-1", "name":"KoRn", "url":"https://www.ticketmaster.pl/event/korn-bilety/496543017",
            "dates":{"start":{"dateTime":"2026-11-17T17:00:00Z","dateTBD":false},"timezone":"Europe/Warsaw","status":{"code":"onsale"}},
            "sales":{"public":{"startDateTime":"2026-01-01T09:00:00Z","startTBD":false},"presales":[{"name":"Fan club","startDateTime":"2025-12-31T09:00:00Z","url":"https://www.ticketmaster.pl/event/496543017"}]},
            "_embedded":{"venues":[{"id":"venue-1","name":"TAURON Arena","city":{"name":"Kraków"},"country":{"name":"Poland","countryCode":"PL"},"timezone":"Europe/Warsaw"}]},
            "secret":FAKE_KEY,
        }]},"page":{"number":0,"size":100,"totalElements":1,"totalPages":1}}).to_string();
        let (mut client, request) = mock("200 OK", "", &body).await;
        let output = client
            .events("K8Artist", Some("PL"), Some("Kraków"))
            .await
            .unwrap();
        let params = request_params(&request.await.unwrap());
        assert_eq!(params["attractionId"], "K8Artist");
        assert_eq!(params["countryCode"], "PL");
        assert_eq!(params["city"], "Kraków");
        assert_eq!(params["source"], "ticketmaster");
        assert_eq!(output["events"][0]["event_status"], "onsale");
        assert_eq!(
            output["events"][0]["venues"][0]["country"]["countryCode"],
            "PL"
        );
        assert_eq!(output["inventory_verified"], false);
        assert_eq!(output["has_more"], false);
        assert!(!output.to_string().contains(FAKE_KEY));
    }

    #[tokio::test]
    async fn country_catalog_query_omits_artist_filter_and_reports_bounded_scope() {
        let body = json!({"_embedded":{"events":[{
            "id":"nl-event", "name":"Synthetic Dutch festival",
            "url":"https://www.ticketmaster.nl/event/synthetic/12345678",
            "_embedded":{"attractions":[{"id":"artist-1","name":"First Artist"},{"id":"artist-2","name":"Second Artist"}]},
            "internal":FAKE_KEY
        }]},"page":{"number":3,"size":100,"totalElements":1500,"totalPages":15}}).to_string();
        let (mut client, request) = mock("200 OK", "", &body).await;
        let output = client
            .events_country_page("NL", Some("Amsterdam"), 3)
            .await
            .unwrap();
        let params = request_params(&request.await.unwrap());
        assert_eq!(params["countryCode"], "NL");
        assert_eq!(params["city"], "Amsterdam");
        assert_eq!(params["source"], "ticketmaster");
        assert_eq!(params["size"], "100");
        assert_eq!(params["page"], "3");
        assert_eq!(params["sort"], "date,asc");
        assert!(!params.contains_key("attractionId"));
        assert_eq!(output["selection_scope"], "country");
        assert!(output["attraction_id"].is_null());
        assert_eq!(output["has_more"], true);
        assert_eq!(output["next_page"], 4);
        assert_eq!(output["inventory_verified"], false);
        assert_eq!(
            output["events"][0]["attractions"],
            json!([
                {"id":"artist-1","name":"First Artist"},
                {"id":"artist-2","name":"Second Artist"}
            ])
        );
        assert!(!output.to_string().contains(FAKE_KEY));
    }

    #[tokio::test]
    async fn country_catalog_validation_never_requests_an_unbounded_query() {
        let (mut client, mut request) = mock("200 OK", "", "{}").await;
        for country in ["", "nl", "Netherlands", "NL&apikey=other", " NL"] {
            assert!(client.events_country_page(country, None, 0).await.is_err());
        }
        assert!(client.events_country_page("NL", None, 10).await.is_err());
        assert!(
            client
                .events_country_page("NL", Some(" "), 0)
                .await
                .is_err()
        );
        assert!(
            client
                .events_query_page(None, None, None, 0, None)
                .await
                .is_err()
        );
        // The unchanged artist API must never interpret an empty ID as all artists.
        assert!(client.events_page("", Some("NL"), None, 0).await.is_err());
        assert!(matches!(
            request.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn country_event_artist_names_are_bounded_metadata() {
        let mut artists = vec![
            json!({"id":"valid-artist", "name":"Å".repeat(500), "private":FAKE_KEY}),
            json!({"id":"control-name", "name":"First\nArtist\tName"}),
            json!({"id":"invalid&id", "name":"Reject invalid identity"}),
            json!({"id":"", "name":"Reject empty identity"}),
            json!({"id":"missing-name"}),
        ];
        artists.extend(
            (0..105).map(
                |index| json!({"id":format!("artist-{index}"),"name":format!("Artist {index}")}),
            ),
        );
        let event = normalize_event(&json!({"_embedded":{"attractions":artists}}));
        let names = event["attractions"].as_array().unwrap();
        assert!(names.len() <= 100);
        assert_eq!(names[0]["name"].as_str().unwrap().chars().count(), 400);
        assert_eq!(names[1]["name"], "FirstArtistName");
        assert!(
            names
                .iter()
                .all(|artist| artist["id"] != "invalid&id" && artist["id"] != "")
        );
        assert!(
            names
                .iter()
                .find(|artist| artist["id"] == "missing-name")
                .unwrap()["name"]
                .is_null()
        );
        assert!(!event.to_string().contains(FAKE_KEY));
        assert_eq!(normalize_event(&json!({}))["attractions"], json!([]));
    }

    #[test]
    fn event_identity_and_catalog_prices_are_bounded_metadata() {
        let event = normalize_event(&json!({
            "id":"event-1", "name":"KoRn",
            "_embedded":{"attractions":[{"id":"K8Artist"},{"id":"K8Artist"},{"id":"K8Guest"},{"id":"invalid&id"},{"id":""}]},
            "priceRanges":[
                {"type":"standard","currency":"pln","min":435.85,"max":910.80},
                {"type":"broken","currency":"PLN","min":100,"max":50},
                {"type":"bad currency","currency":"Poland","min":50},
                {"type":"unknown","currency":"PLN","min":"call us"}
            ]
        }));
        assert_eq!(event["attraction_ids"], json!(["K8Artist", "K8Guest"]));
        assert_eq!(
            event["price_ranges"],
            json!([{"type":"standard","currency":"PLN","min":435.85,"max":910.80}])
        );
        assert_eq!(event["inventory_verified"], false);
        assert!(
            event["price_ranges_notice"]
                .as_str()
                .unwrap()
                .contains("metadata")
        );
        assert!(event.get("offers").is_none());
        assert!(event.get("availability").is_none());
        let empty = normalize_event(&json!({}));
        assert_eq!(empty["attraction_ids"], json!([]));
        assert_eq!(empty["price_ranges"], json!([]));
    }

    #[tokio::test]
    async fn authentication_failure_pauses_future_requests_without_echoing_secrets() {
        for status in ["401 Unauthorized", "403 Forbidden"] {
            let (mut client, request) = mock(status, "", FAKE_KEY).await;
            let error = client.artists("KoRn").await.unwrap_err().to_string();
            assert!(error.contains(&status[..3]));
            assert!(!error.contains(FAKE_KEY));
            assert!(request.await.is_ok());
            assert!(client.is_paused());
            let error = client
                .events("K8Artist", None, None)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("client paused"));
            assert!(!error.contains(FAKE_KEY));
        }
    }

    #[tokio::test]
    async fn exposed_cooldown_excludes_normal_request_pacing() {
        let body = r#"{"page":{"number":0,"size":20,"totalElements":0,"totalPages":0}}"#;
        let (mut client, _) = mock("200 OK", "", body).await;
        client.artists("KoRn").await.unwrap();
        assert!(client.cooldown_until().is_none());
        assert!(!client.is_paused());

        let (mut client, _) = mock("429 Too Many Requests", "Retry-After: 172800\r\n", "{}").await;
        let before = chrono::Utc::now().timestamp();
        assert!(client.artists("KoRn").await.is_err());
        let until = client.cooldown_until().unwrap();
        assert!(until >= before + 172800);
        assert!(until <= chrono::Utc::now().timestamp() + 172801);
        assert!(!client.is_paused());
        client.provider_cooldown = Some(Instant::now() - Duration::from_secs(1));
        assert!(client.cooldown_until().is_none());
    }

    #[tokio::test]
    async fn expired_rate_limit_reset_does_not_authorize_more_requests() {
        let (mut client, _) = mock(
            "429 Too Many Requests",
            "Rate-Limit-Available: 0\r\nRate-Limit-Reset: 1\r\nRetry-After: 0\r\n",
            "{}",
        )
        .await;
        assert!(client.artists("KoRn").await.is_err());
        assert!(client.is_paused());
        assert!(client.cooldown_until().is_none());
    }

    #[tokio::test]
    async fn rate_limit_errors_do_not_echo_credentials_or_response_body() {
        let (mut client, request) = mock(
            "429 Too Many Requests",
            "Retry-After: 12\r\nRate-Limit-Available: 0\r\n",
            &format!("{{\"fault\":\"apikey={FAKE_KEY}\"}}"),
        )
        .await;
        let error = client.artists("KoRn").await.unwrap_err().to_string();
        assert!(error.contains("HTTP 429"));
        assert!(error.contains("retry_after_seconds=12"));
        assert!(!error.contains(FAKE_KEY));
        assert!(client.next_request > Instant::now() + Duration::from_secs(10));
        assert!(request.await.is_ok());
    }

    #[tokio::test]
    async fn unknown_rate_limit_reset_pauses_client_without_another_request() {
        let (mut client, _) = mock("429 Too Many Requests", "", FAKE_KEY).await;
        assert!(client.artists("KoRn").await.is_err());
        let error = client.artists("KoRn").await.unwrap_err().to_string();
        assert!(error.contains("client paused"));
        assert!(!error.contains(FAKE_KEY));
    }

    #[tokio::test]
    async fn cooldown_longer_than_a_day_is_not_shortened() {
        let (mut client, _) = mock("429 Too Many Requests", "Retry-After: 172800\r\n", "{}").await;
        assert!(client.artists("KoRn").await.is_err());
        assert!(client.next_request > Instant::now() + Duration::from_secs(172_790));
    }

    #[tokio::test]
    async fn malformed_json_error_is_redacted() {
        let (mut client, _) = mock("200 OK", "", &format!("not JSON {FAKE_KEY}")).await;
        let error = client.artists("KoRn").await.unwrap_err().to_string();
        assert!(error.contains("invalid JSON"));
        assert!(!error.contains(FAKE_KEY));
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let (mut client, _) = mock(
            "302 Found",
            "Location: https://example.invalid/collect\r\n",
            FAKE_KEY,
        )
        .await;
        let error = client.artists("KoRn").await.unwrap_err().to_string();
        assert!(error.contains("HTTP 302"));
        assert!(!error.contains(FAKE_KEY));
    }

    #[tokio::test]
    async fn missing_results_are_only_empty_when_total_is_zero() {
        let (mut client, _) = mock(
            "200 OK",
            "",
            r#"{"page":{"number":0,"size":100,"totalElements":0,"totalPages":0}}"#,
        )
        .await;
        let output = client.events("K8Artist", None, None).await.unwrap();
        assert_eq!(output["events"], json!([]));
        let (mut client, _) = mock(
            "200 OK",
            "",
            r#"{"page":{"number":0,"size":100,"totalElements":4,"totalPages":1}}"#,
        )
        .await;
        assert!(client.events("K8Artist", None, None).await.is_err());
    }

    #[tokio::test]
    async fn request_validation_rejects_invalid_country_and_empty_query() {
        let (mut client, _) = mock("200 OK", "", "{}").await;
        assert!(client.artists("  ").await.is_err());
        assert!(
            client
                .events("K8Artist", Some("poland"), None)
                .await
                .is_err()
        );
        assert!(
            client
                .events("K8Artist&apikey=other", None, None)
                .await
                .is_err()
        );
        assert!(client.artists_page("KoRn", 50).await.is_err());
        assert!(
            client
                .events_page("K8Artist", None, None, 10)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn explicit_page_is_encoded_and_next_page_is_reported() {
        let body = json!({"_embedded":{"events":[]},"page":{"number":2,"size":100,"totalElements":500,"totalPages":5}}).to_string();
        let (mut client, request) = mock("200 OK", "", &body).await;
        let output = client.events_page("K8Artist", None, None, 2).await.unwrap();
        assert_eq!(request_params(&request.await.unwrap())["page"], "2");
        assert_eq!(output["next_page"], 3);
        assert_eq!(output["deep_paging_limit_reached"], false);
    }

    #[tokio::test]
    async fn country_date_window_is_encoded_without_artist_or_secret_leakage() {
        let body = json!({"_embedded":{"events":[]},"page":{"number":1,"size":100,"totalElements":150,"totalPages":2}}).to_string();
        let (mut client, request) = mock("200 OK", "", &body).await;
        let output = client
            .events_country_window_page("NL", Some("Amsterdam"), 1, 1_893_456_000, 1_893_542_399)
            .await
            .unwrap();
        let params = request_params(&request.await.unwrap());
        assert_eq!(params["startDateTime"], "2029-12-31T23:59:59Z");
        assert_eq!(params["endDateTime"], "2030-01-02T00:00:00Z");
        assert_eq!(params["page"], "1");
        assert_eq!(params["sort"], "date,asc");
        assert_eq!(params["countryCode"], "NL");
        assert_eq!(params["includeTBA"], "no");
        assert_eq!(params["includeTBD"], "no");
        assert!(!params.contains_key("attractionId"));
        assert_eq!(output["date_window"]["start"], "2030-01-01T00:00:00Z");
        assert_eq!(
            output["date_window"]["request_start"],
            params["startDateTime"]
        );
        assert!(!output.to_string().contains(FAKE_KEY));
    }

    #[tokio::test]
    async fn country_date_window_rejects_reversed_or_unrepresentable_bounds() {
        let (mut client, _) = mock("200 OK", "", "{}").await;
        assert!(
            client
                .events_country_window_page("NL", None, 0, 10, 9)
                .await
                .is_err()
        );
        assert!(
            client
                .events_country_window_page("NL", None, 0, i64::MIN, i64::MAX)
                .await
                .is_err()
        );
        assert!(
            client
                .events_country_window_page("NL", None, 10, 0, 1)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn oversized_terminal_event_page_is_rejected_before_catalog_can_claim_completion() {
        use crate::{
            catalog::{self, Selection, Snapshot},
            config::{Config, Watch},
            database::Db,
        };
        let body = json!({"_embedded":{"events":(0..101).map(|index| json!({
            "id":format!("oversized-{index}"),"name":"synthetic must not be silently truncated",
            "url":format!("https://www.ticketmaster.nl/event/synthetic/{}",20000000+index)
        })).collect::<Vec<_>>()},"page":{"number":0,"size":100,"totalElements":101,"totalPages":1}})
        .to_string();
        let (mut client, _) = mock("200 OK", "", &body).await;
        let config: Config =
            toml::from_str(include_str!("../examples/netherlands-all.example.toml")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let db = Db::open(&directory.path().join("oversized-catalog.sqlite")).unwrap();
        let cached = Watch {
            id: "cached-existing".into(),
            url: "https://www.ticketmaster.nl/event/synthetic/10000001".into(),
            ..Watch::default()
        };
        let saved = Snapshot {
            fingerprint: catalog::fingerprint(&config).unwrap(),
            selections: vec![Selection {
                id: config.subscriptions[0].id.clone(),
                watches: vec![cached],
                complete: true,
                ..Selection::default()
            }],
            ..Snapshot::default()
        };
        db.call(move |store| store.catalog_save(&serde_json::to_value(saved)?))
            .await
            .unwrap();
        let result = catalog::refresh(&config, &db, &mut client).await.unwrap();
        let selection = &result.selections[0];
        assert_eq!(result.requests_attempted, 1);
        assert!(!selection.complete);
        assert_eq!(selection.watches.len(), 1);
        assert_eq!(selection.watches[0].id, "cached-existing");
        assert!(
            selection.metadata.is_empty(),
            "no silently truncated page is merged"
        );
        let scan = selection.country_scan.as_ref().unwrap();
        assert_eq!(scan.completed_windows, 0);
        assert_eq!(scan.pending.front().unwrap().start, scan.start);
        assert_eq!(scan.pending.front().unwrap().page, 0);
        assert!(
            selection
                .notes
                .iter()
                .any(|note| note.contains("100-item bound"))
        );
        assert!(!serde_json::to_string(&result).unwrap().contains(FAKE_KEY));
    }

    #[tokio::test]
    async fn adjacent_date_windows_cover_split_and_day_boundaries_with_strict_filters() {
        let midnight = 1_893_456_000;
        let split = midnight + 43_200;
        let body = r#"{"page":{"number":0,"size":100,"totalElements":0,"totalPages":0}}"#;
        for (start, end, edges) in [
            (midnight, split - 1, vec![midnight, split - 1]),
            (split, midnight + 86_399, vec![split, midnight + 86_399]),
            (
                midnight + 86_400,
                midnight + 172_799,
                vec![midnight + 86_400],
            ),
        ] {
            let (mut client, request) = mock("200 OK", "", body).await;
            client
                .events_country_window_page("NL", None, 0, start, end)
                .await
                .unwrap();
            let params = request_params(&request.await.unwrap());
            let wire_start = chrono::DateTime::parse_from_rfc3339(&params["startDateTime"])
                .unwrap()
                .timestamp();
            let wire_end = chrono::DateTime::parse_from_rfc3339(&params["endDateTime"])
                .unwrap()
                .timestamp();
            for exact_boundary in edges {
                assert!(
                    wire_start < exact_boundary && exact_boundary < wire_end,
                    "exclusive provider filters must include logical edge {exact_boundary}"
                );
            }
        }
    }

    #[test]
    fn unsafe_returned_urls_are_not_exposed() {
        for url in [
            "https://example.test/?apikey=secret",
            "https://user:pass@example.test/",
            "javascript:alert(1)",
        ] {
            assert_eq!(public_url(Some(&json!(url))), Value::Null);
        }
    }

    #[test]
    fn unknown_pagination_is_explicit() {
        let output = envelope(&json!({"_embedded":{"attractions":[]}}));
        assert!(output["has_more"].is_null());
        assert!(output["next_page"].is_null());
    }

    #[test]
    fn deep_paging_limit_does_not_advertise_an_unusable_next_page() {
        let output =
            envelope(&json!({"page":{"number":9,"size":100,"totalElements":2000,"totalPages":20}}));
        assert_eq!(output["has_more"], true);
        assert!(output["next_page"].is_null());
        assert_eq!(output["deep_paging_limit_reached"], true);
    }

    #[test]
    fn returned_event_urls_match_watch_rules_and_drop_tracking() {
        assert_eq!(
            public_event_url(Some(&json!(
                "https://www.ticketmaster.pl/event/korn-bilety/496543017?utm_source=discovery&language=en#seats"
            ))),
            json!("https://www.ticketmaster.pl/event/korn-bilety/496543017")
        );
        for raw in [
            "https://www.ticketmaster.pl/event/496543017?session=secret",
            "https://secure.ticketmaster.pl/payment/?t=secret",
            "https://example.org/event/korn",
            "https://www.ticketmaster.pl/artist/775700",
        ] {
            assert!(public_event_url(Some(&json!(raw))).is_null());
        }
    }
}
