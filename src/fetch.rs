//! Transport only: this module never interprets a failed request as sold out.
use crate::config::{BrowserConfig, Config, FetchMode, PageAdapter, Watch, validate_public_url};
use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

#[derive(Debug, Serialize)]
pub struct FetchResult {
    pub status: u16,
    pub html: String,
    pub retry_after_secs: Option<u64>,
    pub elapsed_ms: u64,
    pub final_url: String,
    pub detail: String,
    pub adapter_state: Option<String>,
    pub map_inspection: Option<crate::map_inspection::MapInspection>,
    pub map_scan: Option<crate::map_scan::BrowserMapScan>,
    pub cart_controls: Option<serde_json::Value>,
}

pub struct Fetcher {
    client: Client,
    timeout: Duration,
    max_body_bytes: usize,
    browser_config: BrowserConfig,
    secret_env: String,
    browser: Mutex<Option<BrowserProcess>>,
}

impl Fetcher {
    pub fn new(config: &Config) -> Result<Self> {
        let client = Client::builder()
            .user_agent("TicketWatch/0.1 (ticket availability monitor)")
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .connect_timeout(Duration::from_secs(config.request_timeout_secs.min(10)))
            .pool_idle_timeout(Duration::from_secs(90))
            .redirect(Policy::custom(|attempt| {
                let same_host = attempt
                    .previous()
                    .first()
                    .is_none_or(|origin| origin.host_str() == attempt.url().host_str());
                if attempt.previous().len() >= 5
                    || !same_host
                    || validate_public_url(attempt.url().as_str()).is_err()
                {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .context("Initialize HTTPS client")?;
        Ok(Self {
            client,
            timeout: Duration::from_secs(config.request_timeout_secs),
            max_body_bytes: config.max_body_bytes,
            browser_config: config.browser.clone(),
            secret_env: config.discord.token_env.clone(),
            browser: Mutex::new(None),
        })
    }

    pub async fn fetch(&self, watch: &Watch) -> Result<FetchResult> {
        let safe_url = validate_public_url(&watch.url)?;
        let page = match watch.mode {
            FetchMode::Http => self.http(safe_url.as_str()).await?,
            FetchMode::Browser => {
                self.browser(
                    safe_url.as_str(),
                    watch.parser.adapter,
                    BrowserTarget::default(),
                )
                .await?
            }
        };
        if (200..300).contains(&page.status)
            && !crate::event_identity::same_event(&watch.url, &page.final_url)
        {
            bail!("Fetched page does not match the watched event; inventory unknown");
        }
        Ok(page)
    }

    pub async fn fetch_url(&self, url: &str, mode: FetchMode) -> Result<FetchResult> {
        let mut safe_url = validate_public_url(url)?;
        safe_url.set_fragment(None);
        match mode {
            FetchMode::Http => self.http(safe_url.as_str()).await,
            FetchMode::Browser => {
                self.browser(
                    safe_url.as_str(),
                    PageAdapter::Selectors,
                    BrowserTarget::default(),
                )
                .await
            }
        }
    }

    /// Inspect the event's map display without switching to ticket offers.
    pub async fn fetch_map(&self, url: &str) -> Result<FetchResult> {
        self.fetch_map_target(url, None, None, None).await
    }

    /// Open one supported map section and optionally hover one seat; never selects it.
    /// The Netherlands adapter remains a draft pending live access validation.
    pub async fn fetch_map_section(
        &self,
        url: &str,
        section_id: &str,
        seat_id: Option<&str>,
    ) -> Result<FetchResult> {
        if !crate::event_identity::identity(url)
            .is_some_and(|(host, _)| matches!(host.as_str(), "ticketmaster.cz" | "ticketmaster.nl"))
        {
            bail!("Section inspection requires a public Czech or Netherlands event URL");
        }
        if !map_identifier(section_id) || seat_id.is_some_and(|id| !map_identifier(id)) {
            bail!("Map section and seat IDs must be bounded ASCII identifiers");
        }
        self.fetch_map_target(url, Some(section_id), seat_id, None)
            .await
    }

    /// Inspect a bounded set of displayed resale prices in one Czech section.
    pub async fn fetch_map_scan(
        &self,
        url: &str,
        section_id: &str,
        limit: u32,
    ) -> Result<FetchResult> {
        if !crate::event_identity::identity(url).is_some_and(|(host, _)| host == "ticketmaster.cz")
        {
            bail!("Map scanning requires a public Czech event URL");
        }
        if !map_identifier(section_id) || !(1..=50).contains(&limit) {
            bail!("Map scanning requires a bounded section ID and limit from 1 to 50");
        }
        self.fetch_map_target(url, Some(section_id), None, Some(limit))
            .await
    }

    /// Inspect local quantity/filter controls in an isolated, network-fenced context.
    /// This does not submit Find tickets or create a reservation.
    pub async fn fetch_cart_controls(
        &self,
        url: &str,
        category_id: &str,
        demo_quantity: u32,
    ) -> Result<FetchResult> {
        self.fetch_cart_target(url, category_id, demo_quantity, None)
            .await
    }

    /// Rehearse explicit configured filters in the same isolated offline flow.
    /// Metadata IDs are inputs, not stock or a reservation authorization.
    pub async fn fetch_cart_preparation(
        &self,
        url: &str,
        category_id: &str,
        quantity: u32,
        price_id: &str,
        section_ids: &[String],
    ) -> Result<FetchResult> {
        if !map_identifier(price_id)
            || section_ids.len() > 128
            || section_ids.iter().any(|id| !map_identifier(id))
            || section_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != section_ids.len()
        {
            bail!("Cart preparation requires bounded unique configured price and section IDs");
        }
        self.fetch_cart_target(
            url,
            category_id,
            quantity,
            Some(BrowserCartSelection {
                price_id,
                section_ids,
            }),
        )
        .await
    }

    async fn fetch_cart_target(
        &self,
        url: &str,
        category_id: &str,
        demo_quantity: u32,
        selection: Option<BrowserCartSelection<'_>>,
    ) -> Result<FetchResult> {
        let mut safe_url = validate_public_url(url)?;
        safe_url.set_fragment(None);
        if !crate::event_identity::identity(safe_url.as_str())
            .is_some_and(|(host, _)| host == "ticketmaster.cz")
        {
            bail!("Cart controls inspection requires a public Czech event URL");
        }
        if !map_identifier(category_id) || !(1..=20).contains(&demo_quantity) {
            bail!(
                "Cart controls inspection requires a bounded category ID and demo quantity from 1 to 20"
            );
        }
        let page = self
            .browser(
                safe_url.as_str(),
                PageAdapter::Ticketmaster,
                BrowserTarget {
                    display: Some(BrowserDisplay::CartControls),
                    cart_category_id: Some(category_id),
                    cart_demo_quantity: Some(demo_quantity),
                    cart_selection: selection,
                    ..BrowserTarget::default()
                },
            )
            .await?;
        if (200..300).contains(&page.status)
            && !crate::event_identity::same_event(url, &page.final_url)
        {
            bail!("Fetched cart controls do not match the requested event");
        }
        Ok(page)
    }

    async fn fetch_map_target(
        &self,
        url: &str,
        section_id: Option<&str>,
        seat_id: Option<&str>,
        scan_limit: Option<u32>,
    ) -> Result<FetchResult> {
        let safe_url = validate_public_url(url)?;
        if !crate::event_identity::same_event(url, url) {
            bail!("Map inspection requires a supported public event URL");
        }
        let page = self
            .browser(
                safe_url.as_str(),
                PageAdapter::Ticketmaster,
                BrowserTarget {
                    display: Some(BrowserDisplay::Map),
                    map_section_id: section_id,
                    map_seat_id: seat_id,
                    map_scan_limit: scan_limit,
                    ..BrowserTarget::default()
                },
            )
            .await?;
        if (200..300).contains(&page.status)
            && !crate::event_identity::same_event(url, &page.final_url)
        {
            bail!("Fetched map does not match the requested event");
        }
        Ok(page)
    }

    async fn http(&self, url: &str) -> Result<FetchResult> {
        let started = Instant::now();
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(redacted_http_error)?;
        read_response(response, self.max_body_bytes, started).await
    }

    async fn browser(
        &self,
        url: &str,
        adapter: PageAdapter,
        target: BrowserTarget<'_>,
    ) -> Result<FetchResult> {
        // The child process is reused and captures are serialized. Ordinary
        // reads reuse a context; cart-controls demos use a fresh fenced context.
        let mut guard = self.browser.lock().await;
        if guard.is_none() {
            *guard = Some(BrowserProcess::spawn(
                &self.browser_config,
                &self.secret_env,
            )?);
        }
        let worker = guard.as_mut().expect("worker exists");
        let request = BrowserRequest {
            id: worker.next_id,
            url,
            timeout_ms: self.timeout.as_millis() as u64,
            max_body_bytes: self.max_body_bytes,
            render_wait_ms: self.browser_config.render_wait_ms,
            adapter,
            display: target.display,
            map_section_id: target.map_section_id,
            map_seat_id: target.map_seat_id,
            map_scan_limit: target.map_scan_limit,
            cart_category_id: target.cart_category_id,
            cart_demo_quantity: target.cart_demo_quantity,
            cart_selection: target.cart_selection,
        };
        worker.next_id += 1;
        // Include a bounded browser-startup allowance in addition to the navigation deadline.
        let result = tokio::time::timeout(
            self.timeout + Duration::from_secs(15),
            worker.capture(&request),
        )
        .await;
        match result {
            Ok(Ok(reply)) => Ok(reply),
            other => {
                if let Some(mut failed) = guard.take() {
                    failed.terminate().await;
                }
                match other {
                    Ok(Err(error)) => Err(error),
                    Err(_) => bail!(
                        "Browser worker timed out; it was stopped and will restart on a later check"
                    ),
                    Ok(Ok(_)) => unreachable!(),
                }
            }
        }
    }

    pub async fn shutdown(&self) {
        if let Some(mut worker) = self.browser.lock().await.take() {
            worker.terminate().await;
        }
    }
}

fn map_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn redacted_http_error(error: reqwest::Error) -> anyhow::Error {
    // reqwest's Display includes the full URL, so do not propagate it into diagnostics.
    let detail = if error.is_timeout() {
        "request timeout"
    } else if error.is_connect() {
        "connection or TLS failure"
    } else if error.is_decode() {
        "invalid response encoding"
    } else if error.is_body() {
        "incomplete response body"
    } else {
        "request failed"
    };
    anyhow::anyhow!("HTTPS fetch: {detail}")
}

async fn read_response(
    mut response: reqwest::Response,
    max_bytes: usize,
    started: Instant,
) -> Result<FetchResult> {
    let status = response.status();
    let retry_after_secs = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_retry_after(value, chrono::Utc::now().timestamp()));
    let server = response
        .headers()
        .get(reqwest::header::SERVER)
        .and_then(|value| value.to_str().ok())
        .map(safe_header)
        .unwrap_or_else(|| "not supplied".into());
    let response_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(safe_header)
        .unwrap_or_else(|| "not supplied".into());
    let final_url = response.url().to_string();
    // Error pages are never inventory evidence, and there is no need to download a large challenge body.
    if !status.is_success() {
        let mut detail = format!(
            "{}; server={server}; content-type={response_type}",
            status_detail(status)
        );
        // Match only the observed status/header combination. The marker does not
        // disclose the underlying risk rule and arbitrary header values stay private.
        if status == StatusCode::FORBIDDEN
            && response.headers().get_all("tm-bl").iter().count() == 1
            && response
                .headers()
                .get("tm-bl")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                == Some("1")
        {
            detail.push_str("; manual intervention required: ");
            detail.push_str(ManualIntervention::TicketmasterProtection.detail());
        }
        return Ok(FetchResult {
            status: status.as_u16(),
            html: String::new(),
            retry_after_secs,
            elapsed_ms: started.elapsed().as_millis() as u64,
            final_url,
            detail,
            adapter_state: None,
            map_inspection: None,
            map_scan: None,
            cart_controls: None,
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        bail!("Response exceeds configured body limit");
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !content_type.is_empty()
        && !content_type.starts_with("text/html")
        && !content_type.starts_with("application/xhtml+xml")
    {
        bail!("Expected an HTML event page; received another content type");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(redacted_http_error)? {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            bail!("Response exceeds configured body limit");
        }
        body.extend_from_slice(&chunk);
    }
    let html = String::from_utf8(body).context("Event page is not valid UTF-8")?;
    Ok(FetchResult {
        status: status.as_u16(),
        html,
        retry_after_secs,
        elapsed_ms: started.elapsed().as_millis() as u64,
        final_url,
        detail: "HTTP page received".into(),
        adapter_state: None,
        map_inspection: None,
        map_scan: None,
        cart_controls: None,
    })
}

fn safe_header(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(120)
        .collect()
}

fn status_detail(status: StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "Authentication required; inventory unknown",
        403 => "Access blocked (HTTP 403); inventory unknown",
        429 => "Rate limited (HTTP 429); inventory unknown",
        300..=399 => {
            "Redirect not followed: destination or redirect count is outside the public-page policy"
        }
        400..=499 => "Event page unavailable; inventory unknown",
        _ => "Upstream error; inventory unknown",
    }
}

pub fn parse_retry_after(value: &str, now: i64) -> Option<u64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|time| time.timestamp().saturating_sub(now).max(0) as u64)
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum BrowserDisplay {
    Map,
    CartControls,
}

#[derive(Default)]
struct BrowserTarget<'a> {
    display: Option<BrowserDisplay>,
    map_section_id: Option<&'a str>,
    map_seat_id: Option<&'a str>,
    map_scan_limit: Option<u32>,
    cart_category_id: Option<&'a str>,
    cart_demo_quantity: Option<u32>,
    cart_selection: Option<BrowserCartSelection<'a>>,
}

#[derive(Serialize)]
struct BrowserCartSelection<'a> {
    price_id: &'a str,
    section_ids: &'a [String],
}

#[derive(Serialize)]
struct BrowserRequest<'a> {
    id: u64,
    url: &'a str,
    timeout_ms: u64,
    max_body_bytes: usize,
    render_wait_ms: u64,
    adapter: PageAdapter,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<BrowserDisplay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    map_section_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    map_seat_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    map_scan_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cart_category_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cart_demo_quantity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cart_selection: Option<BrowserCartSelection<'a>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManualIntervention {
    Captcha,
    Queue,
    SessionExpired,
    AuthenticationRequired,
    AccessBlocked,
    Verification,
    TicketmasterProtection,
    IdentificationIncomplete,
    #[serde(other)]
    Unknown,
}

impl ManualIntervention {
    fn detail(&self) -> &'static str {
        match self {
            Self::Captcha => "human verification",
            Self::Queue => "website queue",
            Self::SessionExpired => "expired session",
            Self::AuthenticationRequired => "authentication required",
            Self::AccessBlocked => "access blocked",
            Self::Verification => "website access check",
            Self::TicketmasterProtection => {
                "Ticketmaster access-protection block; exact trigger undisclosed"
            }
            Self::IdentificationIncomplete => {
                "Ticketmaster identification incomplete; last confirmed error response retained"
            }
            Self::Unknown => "unrecognized browser restriction",
        }
    }
}

#[derive(Deserialize)]
struct BrowserReply {
    id: Option<u64>,
    #[serde(default)]
    browser_pid: Option<u32>,
    status: Option<u16>,
    html: Option<String>,
    final_url: Option<String>,
    retry_after_secs: Option<u64>,
    elapsed_ms: Option<u64>,
    error: Option<String>,
    server: Option<String>,
    content_type: Option<String>,
    adapter_state: Option<String>,
    adapter_reason: Option<String>,
    manual_intervention: Option<ManualIntervention>,
    map_inspection: Option<crate::map_inspection::MapInspection>,
    map_scan: Option<crate::map_scan::BrowserMapScan>,
    cart_controls: Option<serde_json::Value>,
}

struct BrowserProcess {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    next_id: u64,
    browser_pid: Option<u32>,
}

impl BrowserProcess {
    fn spawn(config: &BrowserConfig, secret_env: &str) -> Result<Self> {
        let script: PathBuf = std::fs::canonicalize(&config.script)
            .context("Browser worker missing; check browser.script")?;
        let mut command = Command::new(&config.node);
        command
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_remove(secret_env)
            .env_remove("DISCORD_BOT_TOKEN")
            .env_remove("TICKETMASTER_API_KEY")
            .env(
                "TICKET_BROWSER_HEADED",
                if config.headed { "1" } else { "0" },
            )
            .kill_on_drop(true);
        if let Some(executable) = &config.executable_path {
            command.env("CHROMIUM_PATH", executable);
        }
        let mut child = command
            .spawn()
            .context("Start browser worker; install Node and npm dependencies first")?;
        let input = child.stdin.take().context("Browser stdin unavailable")?;
        let output = BufReader::new(child.stdout.take().context("Browser stdout unavailable")?);
        Ok(Self {
            child,
            input: Some(input),
            output,
            next_id: 1,
            browser_pid: None,
        })
    }

    async fn capture(&mut self, request: &BrowserRequest<'_>) -> Result<FetchResult> {
        let mut encoded = serde_json::to_vec(request)?;
        encoded.push(b'\n');
        let input = self
            .input
            .as_mut()
            .context("Browser worker input is closed")?;
        input
            .write_all(&encoded)
            .await
            .context("Browser worker stopped accepting requests")?;
        input.flush().await.context("Browser worker input failed")?;
        let wire_limit = request
            .max_body_bytes
            .saturating_mul(6)
            .saturating_add(65_536);
        loop {
            let line = bounded_line(&mut self.output, wire_limit).await?;
            let reply: BrowserReply =
                serde_json::from_slice(&line).context("Invalid browser worker response")?;
            if reply.browser_pid.is_some() {
                self.browser_pid = reply.browser_pid;
            }
            if reply.id.is_none() && reply.browser_pid.is_some() {
                continue;
            }
            if reply.id != Some(request.id) {
                bail!("Browser response ID mismatch");
            }
            if let Some(error) = reply.error {
                // Only forward fixed worker error codes; arbitrary browser messages may contain secrets.
                let known = [
                    "browser_launch_failed",
                    "navigation_timeout",
                    "navigation_failed",
                    "unsafe_redirect",
                    "queue_redirect",
                    "body_too_large",
                    "invalid_request",
                    "capture_failed",
                    "unsupported_charset",
                ];
                let safe = if known.contains(&error.as_str()) {
                    error.as_str()
                } else {
                    "worker_failed"
                };
                bail!("Browser capture: {safe}");
            }
            let html = reply.html.unwrap_or_default();
            if html.len() > request.max_body_bytes {
                bail!("Browser response exceeds configured body limit");
            }
            let final_url = reply
                .final_url
                .context("Browser response omitted final URL")?;
            let safe = validate_public_url(&final_url)?;
            if safe.host_str() != validate_public_url(request.url)?.host_str() {
                bail!("Unexpected browser redirect");
            }
            let status = reply
                .status
                .context("Browser response omitted HTTP status")?;
            let detail = if (200..300).contains(&status) {
                "Rendered browser page received"
            } else {
                status_detail(StatusCode::from_u16(status).context("Invalid HTTP status")?)
            };
            let detail = format!(
                "{detail}; server={}; content-type={}",
                reply
                    .server
                    .as_deref()
                    .map(safe_header)
                    .unwrap_or_else(|| "not supplied".into()),
                reply
                    .content_type
                    .as_deref()
                    .map(safe_header)
                    .unwrap_or_else(|| "not supplied".into())
            );
            let mut adapter_state = match request.adapter {
                PageAdapter::Selectors => None,
                PageAdapter::Ticketmaster => Some(match reply.adapter_state.as_deref() {
                    Some(state @ ("ready" | "unsupported" | "blocked" | "no_offers")) => {
                        state.to_string()
                    }
                    _ => "unsupported".to_string(),
                }),
            };
            // A verification page is never inventory evidence, including custom
            // selector mode. Older workers may omit this optional protocol field.
            let detail = if let Some(reason) = reply.manual_intervention {
                adapter_state = Some("blocked".into());
                format!(
                    "{detail}; manual intervention required: {}; inventory unknown",
                    reason.detail()
                )
            } else {
                detail
            };
            let detail = adapter_state
                .as_ref()
                .map(|state| format!("{detail}; browser adapter={state}"))
                .unwrap_or(detail);
            let detail = if request.cart_category_id.is_some() {
                match reply.adapter_reason.as_deref() {
                    Some(
                        reason @ ("cart_controls_unsupported"
                        | "cart_controls_unrecognized"
                        | "cart_controls_changed"
                        | "cart_controls_timeout"
                        | "cart_controls_network_required"
                        | "cart_controls_modal_blocked"
                        | "cart_controls_ready"),
                    ) => {
                        format!("{detail}; cart controls={reason}")
                    }
                    _ => detail,
                }
            } else {
                detail
            };
            let requested_inspection = request
                .map_section_id
                .filter(|_| request.map_scan_limit.is_none());
            match (requested_inspection, &reply.map_inspection) {
                (None, Some(_)) => bail!("Unsolicited map section inspection"),
                (Some(section_id), Some(inspection)) => {
                    if inspection.section_id != section_id
                        || inspection.seat_id.as_deref() != request.map_seat_id
                    {
                        bail!("Map section inspection target mismatch");
                    }
                    if !(200..300).contains(&status) || adapter_state.as_deref() != Some("ready") {
                        bail!("Map section inspection lacks a ready document");
                    }
                }
                (Some(_), None)
                    if (200..300).contains(&status)
                        && adapter_state.as_deref() == Some("ready") =>
                {
                    bail!("Browser omitted the requested map section inspection");
                }
                _ => {}
            }
            match (request.map_scan_limit, &reply.map_scan) {
                (None, Some(_)) => bail!("Unsolicited map scan"),
                (Some(limit), Some(scan)) => {
                    if Some(scan.section_id.as_str()) != request.map_section_id
                        || scan.limit != limit
                    {
                        bail!("Map scan target or limit mismatch");
                    }
                    if !(200..300).contains(&status) || adapter_state.as_deref() != Some("ready") {
                        bail!("Map scan lacks a ready document");
                    }
                }
                (Some(_), None)
                    if (200..300).contains(&status)
                        && adapter_state.as_deref() == Some("ready") =>
                {
                    bail!("Browser omitted the requested map scan");
                }
                _ => {}
            }
            match (request.cart_category_id, &reply.cart_controls) {
                (None, Some(_)) => bail!("Unsolicited cart controls inspection"),
                (Some(category), Some(inspection)) => {
                    if inspection
                        .get("category_id")
                        .and_then(serde_json::Value::as_str)
                        != Some(category)
                        || inspection
                            .get("demo_quantity")
                            .and_then(serde_json::Value::as_u64)
                            != request.cart_demo_quantity.map(u64::from)
                    {
                        bail!("Cart controls inspection target or quantity mismatch");
                    }
                    if !(200..300).contains(&status) || adapter_state.as_deref() != Some("ready") {
                        bail!("Cart controls inspection lacks a ready document");
                    }
                    match (&request.cart_selection, inspection.get("preparation")) {
                        (None, Some(_)) => bail!("Unsolicited cart filter preparation"),
                        (Some(_), None) => {
                            bail!("Browser omitted the requested cart filter preparation")
                        }
                        (Some(selection), Some(preparation)) => {
                            let sections_match = preparation
                                .get("section_ids")
                                .and_then(serde_json::Value::as_array)
                                .is_some_and(|values| {
                                    values.len() == selection.section_ids.len()
                                        && values
                                            .iter()
                                            .zip(selection.section_ids)
                                            .all(|(value, id)| value.as_str() == Some(id.as_str()))
                                });
                            if preparation
                                .get("price_id")
                                .and_then(serde_json::Value::as_str)
                                != Some(selection.price_id)
                                || !sections_match
                            {
                                bail!("Cart filter preparation target mismatch");
                            }
                        }
                        _ => {}
                    }
                }
                (Some(_), None)
                    if (200..300).contains(&status)
                        && adapter_state.as_deref() == Some("ready") =>
                {
                    bail!("Browser omitted the requested cart controls inspection");
                }
                _ => {}
            }
            return Ok(FetchResult {
                status,
                html,
                final_url,
                retry_after_secs: reply.retry_after_secs,
                elapsed_ms: reply.elapsed_ms.unwrap_or(0),
                detail,
                adapter_state,
                map_inspection: reply.map_inspection,
                map_scan: reply.map_scan,
                cart_controls: reply.cart_controls,
            });
        }
    }

    async fn terminate(&mut self) {
        // Closing stdin lets the worker close Chromium and its context before exiting.
        drop(self.input.take());
        let stopped_cleanly = matches!(
            tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await,
            Ok(Ok(status)) if status.success()
        );
        if !stopped_cleanly {
            if let Some(pid) = self.browser_pid.take() {
                terminate_browser_group(pid);
            }
            let _ = self.child.kill().await;
        }
        self.browser_pid = None;
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        // Playwright launches Chromium in its own process group on Unix. This also cleans up
        // browser children when the Rust runtime is dropped before async shutdown finishes.
        if let Some(pid) = self.browser_pid.take() {
            terminate_browser_group(pid);
        }
    }
}

fn terminate_browser_group(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-TERM", "--", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = pid;
}

async fn bounded_line(reader: &mut BufReader<ChildStdout>, limit: usize) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .context("Read browser worker response")?;
        if available.is_empty() {
            bail!("Browser worker exited before replying");
        }
        let consumed = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(available.len());
        if result.len().saturating_add(consumed) > limit {
            bail!("Browser protocol response exceeds configured limit");
        }
        let finished = available[consumed - 1] == b'\n';
        result.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if finished {
            return Ok(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn response_from(raw: &'static [u8]) -> reqwest::Response {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let _ = stream.read(&mut request).await;
            stream.write_all(raw).await.unwrap();
        });
        Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap()
    }

    #[test]
    fn parses_retry_after_seconds_and_dates() {
        assert_eq!(parse_retry_after("120", 0), Some(120));
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:02:00 GMT", 30),
            Some(90)
        );
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:02:00 GMT", 300),
            Some(0)
        );
        assert_eq!(parse_retry_after("invalid", 0), None);
    }

    #[tokio::test]
    async fn blocked_body_is_not_inventory_and_retains_retry_after() {
        let response = response_from(
            b"HTTP/1.1 403 Forbidden\r\nRetry-After: 120\r\nContent-Length: 7\r\n\r\nInStock",
        )
        .await;
        let result = read_response(response, 100, Instant::now()).await.unwrap();
        assert_eq!(result.status, 403);
        assert_eq!(result.retry_after_secs, Some(120));
        assert!(result.html.is_empty());
    }

    #[tokio::test]
    async fn ticketmaster_protection_header_explains_403_without_reading_error_body() {
        // The declared body is oversized and missing. Reading it would fail;
        // diagnosis must use only the status and response headers.
        let response = response_from(
            b"HTTP/1.1 403 Forbidden\r\nTM-BL: 1\r\nRetry-After: 120\r\nContent-Length: 100000000\r\n\r\n",
        )
        .await;
        let result = read_response(response, 1, Instant::now()).await.unwrap();
        assert_eq!(result.status, 403);
        assert_eq!(result.retry_after_secs, Some(120));
        assert!(result.html.is_empty());
        assert!(result.detail.contains(
            "manual intervention required: Ticketmaster access-protection block; exact trigger undisclosed"
        ));
    }

    #[tokio::test]
    async fn protection_diagnosis_does_not_reclassify_other_responses_or_echo_header_values() {
        let cases: [&'static [u8]; 8] = [
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 403 Forbidden\r\nTM-BL: 0\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 403 Forbidden\r\nTM-BL: private-marker-value\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 403 Forbidden\r\nTM-BL: 1, 1\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 403 Forbidden\r\nTM-BL: 1\r\nTM-BL: 0\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 401 Unauthorized\r\nTM-BL: 1\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 429 Too Many Requests\r\nTM-BL: 1\r\nRetry-After: 90\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 500 Server Error\r\nTM-BL: 1\r\nContent-Length: 0\r\n\r\n",
        ];
        for raw in cases {
            let response = response_from(raw).await;
            let result = read_response(response, 100, Instant::now()).await.unwrap();
            assert!(result.html.is_empty());
            assert!(!result.detail.contains("access-protection"));
            assert!(!result.detail.contains("private-marker-value"));
            if result.status == 429 {
                assert_eq!(result.retry_after_secs, Some(90));
                assert!(result.detail.contains("Rate limited"));
            }
        }
        let response = response_from(
            b"HTTP/1.1 200 OK\r\nTM-BL: 1\r\nContent-Type: text/html\r\nContent-Length: 7\r\n\r\ntickets",
        )
        .await;
        let result = read_response(response, 100, Instant::now()).await.unwrap();
        assert_eq!(result.html, "tickets");
        assert_eq!(result.detail, "HTTP page received");
        assert_eq!(result.adapter_state, None);
    }

    #[tokio::test]
    async fn streaming_body_limit_is_enforced_without_content_length() {
        let response = response_from(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nabcdef\r\n0\r\n\r\n").await;
        assert!(read_response(response, 5, Instant::now()).await.is_err());
    }

    #[tokio::test]
    async fn rejects_non_html_success() {
        let response = response_from(
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 3\r\n\r\nPNG",
        )
        .await;
        assert!(read_response(response, 100, Instant::now()).await.is_err());
    }

    #[tokio::test]
    async fn browser_protocol_reuses_worker_and_shutdown_closes_it() {
        // This stub exercises the Rust child protocol without launching a browser or making requests.
        let mut script = tempfile::NamedTempFile::new().unwrap();
        script
            .write_all(
                br#"
            const readline = require('node:readline');
            const lines = readline.createInterface({input:process.stdin});
            let count = 0;
            lines.on('line', line => {
                const request = JSON.parse(line);
                process.stdout.write(JSON.stringify({id:request.id,status:200,html:String(++count),
                    final_url:request.url,elapsed_ms:1})+'\n');
            });
        "#,
            )
            .unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().into();
        let fetcher = Fetcher::new(&config).unwrap();
        let url = "https://www.ticketmaster.pl/event/synthetic/1";
        let first = fetcher.fetch_url(url, FetchMode::Browser).await.unwrap();
        let second = fetcher.fetch_url(url, FetchMode::Browser).await.unwrap();
        assert_eq!(first.html, "1");
        assert_eq!(second.html, "2");
        fetcher.shutdown().await;
        assert!(fetcher.browser.lock().await.is_none());
    }

    #[tokio::test]
    async fn browser_queue_error_is_fixed_and_does_not_retry_or_expose_unknown_errors() {
        let requests = tempfile::NamedTempFile::new().unwrap();
        let mut script = tempfile::NamedTempFile::new().unwrap();
        let request_path = serde_json::to_string(requests.path()).unwrap();
        let body = format!(
            r#"
            const fs = require('node:fs');
            const lines = require('node:readline').createInterface({{input:process.stdin}});
            lines.on('line', line => {{
                const r = JSON.parse(line);
                fs.appendFileSync({request_path}, 'request\n');
                process.stdout.write(JSON.stringify({{id:r.id,error:r.url.includes('/queue/')
                    ? 'queue_redirect' : 'private-session-error-value'}})+'\n');
            }});
            "#
        );
        script.write_all(body.as_bytes()).unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().into();
        let fetcher = Fetcher::new(&config).unwrap();
        let known = fetcher
            .fetch_url(
                "https://www.ticketmaster.it/event/queue/12345678",
                FetchMode::Browser,
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(known, "Browser capture: queue_redirect");
        assert_eq!(
            std::fs::read_to_string(requests.path())
                .unwrap()
                .lines()
                .count(),
            1
        );
        let unknown = fetcher
            .fetch_url(
                "https://www.ticketmaster.it/event/unknown/12345678",
                FetchMode::Browser,
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(unknown, "Browser capture: worker_failed");
        assert!(!unknown.contains("private-session"));
        assert_eq!(
            std::fs::read_to_string(requests.path())
                .unwrap()
                .lines()
                .count(),
            2
        );
        fetcher.shutdown().await;
    }

    #[tokio::test]
    async fn map_capture_requests_its_own_display_and_still_binds_the_event() {
        let mut script = tempfile::NamedTempFile::new().unwrap();
        script
            .write_all(
                br#"
            const lines=require('node:readline').createInterface({input:process.stdin});
            lines.on('line',line=>{
                const r=JSON.parse(line);
                const mode=r.display||'default';
                const final=r.url.includes('/wrong/')?r.url.replace('12345678','87654321'):r.url;
                process.stdout.write(JSON.stringify({id:r.id,status:200,
                    html:JSON.stringify({adapter:r.adapter,display:mode}),
                    final_url:final,adapter_state:'ready'})+'\n');
            });
        "#,
            )
            .unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().to_owned();
        let fetcher = Fetcher::new(&config).unwrap();
        let result = fetcher
            .fetch_map("https://www.ticketmaster.cz/event/example/12345678")
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.html).unwrap(),
            serde_json::json!({"adapter":"ticketmaster","display":"map"})
        );
        assert!(
            fetcher
                .fetch_map("https://www.ticketmaster.cz/artist/12345678")
                .await
                .is_err()
        );
        assert!(
            fetcher
                .fetch_map("https://www.ticketmaster.cz/event/wrong/12345678")
                .await
                .is_err()
        );
        let ordinary = fetcher
            .fetch_url(
                "https://www.ticketmaster.cz/event/example/12345678",
                FetchMode::Browser,
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&ordinary.html).unwrap(),
            serde_json::json!({"adapter":"selectors","display":"default"})
        );
        fetcher.shutdown().await;
    }

    #[tokio::test]
    async fn map_section_identifiers_and_market_are_checked_before_starting_a_browser() {
        let mut config = Config::default();
        config.browser.script = "missing-browser-worker.cjs".into();
        let fetcher = Fetcher::new(&config).unwrap();
        let url = "https://www.ticketmaster.cz/event/example/12345678";
        for (section, seat) in [
            ("", None),
            ("s_1", Some("")),
            ("s_1] path", None),
            ("s_1", Some("1/2")),
            ("s_ě", None),
            ("s_1", Some("řada")),
        ] {
            let error = fetcher
                .fetch_map_section(url, section, seat)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("bounded ASCII identifiers"));
        }
        let oversized = "x".repeat(129);
        assert!(
            fetcher
                .fetch_map_section(url, &oversized, None)
                .await
                .unwrap_err()
                .to_string()
                .contains("bounded ASCII identifiers")
        );
        assert!(
            fetcher
                .fetch_map_section(url, "s_1", Some(&oversized))
                .await
                .unwrap_err()
                .to_string()
                .contains("bounded ASCII identifiers")
        );
        for url in [
            "https://www.ticketmaster.pl/event/example/12345678",
            "https://www.ticketmaster.cz/artist/12345678",
        ] {
            assert!(
                fetcher
                    .fetch_map_section(url, "s_1", None)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("public Czech or Netherlands event URL")
            );
        }
        assert!(fetcher.browser.lock().await.is_none());
    }

    #[tokio::test]
    async fn map_section_protocol_binds_requested_section_seat_and_event() {
        let mut script = tempfile::NamedTempFile::new().unwrap();
        script
            .write_all(
                br#"
            const lines=require('node:readline').createInterface({input:process.stdin});
            lines.on('line',line=>{
                const r=JSON.parse(line);
                const kind=new URL(r.url).pathname.split('/')[2];
                const evidence=r.map_section_id||kind==='unsolicited'?{
                    section_id:kind==='wrong-section'?'s_other':r.map_section_id||'s_1',
                    section_name:'101',seat_id:kind==='wrong-seat'?'seat_other':r.map_seat_id||null,
                    coverage:'section_only',tooltip:null,stock_verified:false,adjacency_verified:false
                }:undefined;
                process.stdout.write(JSON.stringify({id:r.id,status:200,
                    html:JSON.stringify(r),final_url:kind==='wrong-event'?r.url.replace('12345678','87654321'):r.url,
                    adapter_state:kind==='blocked'?'blocked':'ready',
                    map_inspection:kind==='missing'?undefined:evidence})+'\n');
            });
        "#,
            )
            .unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().to_owned();
        let fetcher = Fetcher::new(&config).unwrap();
        let url = "https://www.ticketmaster.cz/event/example/12345678";
        for seat_id in [None, Some("seat-123")] {
            let result = fetcher
                .fetch_map_section(url, "s_1", seat_id)
                .await
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&result.html).unwrap();
            assert_eq!(request["adapter"], "ticketmaster");
            assert_eq!(request["display"], "map");
            assert_eq!(request["map_section_id"], "s_1");
            assert_eq!(request["map_seat_id"].as_str(), seat_id);
            let evidence = result.map_inspection.unwrap();
            assert_eq!(evidence.section_id, "s_1");
            assert_eq!(evidence.seat_id.as_deref(), seat_id);
        }
        // The draft NL adapter uses the same bounded read-only transport; it
        // does not expand the separate scan or reservation protocol.
        let dutch = "https://www.ticketmaster.nl/event/example/12345678";
        let result = fetcher
            .fetch_map_section(dutch, "s_1", Some("seat-123"))
            .await
            .unwrap();
        let request: serde_json::Value = serde_json::from_str(&result.html).unwrap();
        assert_eq!(request["url"], dutch);
        assert_eq!(request["map_seat_id"], "seat-123");
        for (kind, message) in [
            ("wrong-section", "target mismatch"),
            ("wrong-seat", "target mismatch"),
            ("wrong-event", "does not match the requested event"),
            ("missing", "omitted the requested map section inspection"),
            ("blocked", "lacks a ready document"),
        ] {
            let url = format!("https://www.ticketmaster.cz/event/{kind}/12345678");
            let error = fetcher
                .fetch_map_section(&url, "s_1", Some("seat-123"))
                .await
                .unwrap_err();
            assert!(error.to_string().contains(message), "{kind}: {error}");
        }
        let unsolicited = "https://www.ticketmaster.cz/event/unsolicited/12345678";
        assert!(
            fetcher
                .fetch_map(unsolicited)
                .await
                .unwrap_err()
                .to_string()
                .contains("Unsolicited map section inspection")
        );
        let ordinary = fetcher.fetch_map(url).await.unwrap();
        let request: serde_json::Value = serde_json::from_str(&ordinary.html).unwrap();
        assert!(request.get("map_section_id").is_none());
        assert!(request.get("map_seat_id").is_none());
        assert!(ordinary.map_inspection.is_none());
        fetcher.shutdown().await;
    }

    #[tokio::test]
    async fn map_scan_rejects_invalid_targets_before_starting_a_browser() {
        let mut config = Config::default();
        config.browser.script = "/missing-map-scan-worker.cjs".into();
        let fetcher = Fetcher::new(&config).unwrap();
        let event = "https://www.ticketmaster.cz/event/example/12345678";
        for (section, limit) in [
            ("", 20),
            ("s_1] path", 20),
            ("s_ě", 20),
            ("s_1", 0),
            ("s_1", 51),
        ] {
            let error = fetcher
                .fetch_map_scan(event, section, limit)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("bounded section ID and limit"));
        }
        assert!(
            fetcher
                .fetch_map_scan(event, &"x".repeat(129), 20)
                .await
                .is_err()
        );
        for event in [
            "https://www.ticketmaster.pl/event/example/12345678",
            "https://www.ticketmaster.cz/artist/12345678",
        ] {
            let error = fetcher.fetch_map_scan(event, "s_1", 20).await.unwrap_err();
            assert!(error.to_string().contains("public Czech event URL"));
        }
        assert!(fetcher.browser.lock().await.is_none());
    }

    #[tokio::test]
    async fn map_scan_protocol_requires_matching_exclusive_evidence() {
        let mut script = tempfile::NamedTempFile::new().unwrap();
        script.write_all(br#"
            require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
                const r=JSON.parse(line), kind=new URL(r.url).pathname.split('/')[2];
                const scan={section_id:kind==='wrong-section'?'s_other':r.map_section_id||'s_1',
                    section_name:'101',limit:kind==='wrong-limit'?49:r.map_scan_limit||20,
                    started_at_ms:1000,finished_at_ms:1001,candidate_count:0,visible_candidate_count:0,
                    results:[],stop_reason:'visible_candidates_exhausted',coverage:'partial_rendered_section',
                    stock_verified:false,adjacency_verified:false};
                const inspection={section_id:'s_1',section_name:'101',seat_id:null,coverage:'section_only',
                    tooltip:null,stock_verified:false,adjacency_verified:false};
                process.stdout.write(JSON.stringify({id:r.id,status:kind==='plain-blocked'?403:200,
                    html:JSON.stringify(r),final_url:kind==='wrong-event'?r.url.replace('12345678','87654321'):r.url,
                    adapter_state:['blocked','plain-blocked'].includes(kind)?'blocked':'ready',retry_after_secs:30,
                    map_scan:['missing','plain-blocked'].includes(kind)?undefined:scan,
                    map_inspection:kind==='inspection'?inspection:undefined})+'\n');
            });
        "#).unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().to_owned();
        let fetcher = Fetcher::new(&config).unwrap();
        let result = fetcher
            .fetch_map_scan(
                "https://www.ticketmaster.cz/event/example/12345678",
                "s_1",
                20,
            )
            .await
            .unwrap();
        let request: serde_json::Value = serde_json::from_str(&result.html).unwrap();
        assert_eq!(request["adapter"], "ticketmaster");
        assert_eq!(request["display"], "map");
        assert_eq!(request["map_section_id"], "s_1");
        assert_eq!(request["map_scan_limit"], 20);
        assert!(request.get("map_seat_id").is_none());
        assert!(result.map_inspection.is_none());
        assert_eq!(result.map_scan.unwrap().limit, 20);
        for (kind, message) in [
            ("wrong-section", "target or limit mismatch"),
            ("wrong-limit", "target or limit mismatch"),
            ("wrong-event", "does not match the requested event"),
            ("missing", "omitted the requested map scan"),
            ("blocked", "lacks a ready document"),
            ("inspection", "Unsolicited map section inspection"),
        ] {
            let url = format!("https://www.ticketmaster.cz/event/{kind}/12345678");
            let error = fetcher.fetch_map_scan(&url, "s_1", 20).await.unwrap_err();
            assert!(error.to_string().contains(message), "{kind}: {error}");
        }
        let ordinary = "https://www.ticketmaster.cz/event/unsolicited/12345678";
        for result in [
            fetcher.fetch_map(ordinary).await,
            fetcher.fetch_url(ordinary, FetchMode::Browser).await,
        ] {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("Unsolicited map scan")
            );
        }
        let blocked = fetcher
            .fetch_map_scan(
                "https://www.ticketmaster.cz/event/plain-blocked/12345678",
                "s_1",
                20,
            )
            .await
            .unwrap();
        assert_eq!(blocked.status, 403);
        assert_eq!(blocked.retry_after_secs, Some(30));
        assert_eq!(blocked.adapter_state.as_deref(), Some("blocked"));
        assert!(blocked.map_scan.is_none());
        fetcher.shutdown().await;
    }

    #[tokio::test]
    async fn cart_controls_reject_invalid_targets_before_browser_startup() {
        let fetcher = Fetcher::new(&Config::default()).unwrap();
        let event = "https://www.ticketmaster.cz/event/synthetic/12345678";
        for (category, quantity) in [
            ("", 2),
            ("category] *", 2),
            ("category", 0),
            ("category", 21),
        ] {
            assert!(
                fetcher
                    .fetch_cart_controls(event, category, quantity)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("bounded category ID")
            );
        }
        for event in [
            "https://www.ticketmaster.pl/event/synthetic/12345678",
            "https://www.ticketmaster.cz/artist/12345678",
        ] {
            assert!(
                fetcher
                    .fetch_cart_controls(event, "category", 2)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("public Czech event URL")
            );
        }
        assert!(fetcher.browser.lock().await.is_none());
    }

    #[tokio::test]
    async fn cart_preparation_rejects_invalid_ids_before_browser_startup() {
        let fetcher = Fetcher::new(&Config::default()).unwrap();
        let event = "https://www.ticketmaster.cz/event/synthetic/12345678";
        for (price, sections) in [
            ("".to_string(), vec![]),
            ("price] *".to_string(), vec![]),
            ("p".repeat(129), vec![]),
            ("price".to_string(), vec!["".to_string()]),
            (
                "price".to_string(),
                vec!["1P".to_string(), "1P".to_string()],
            ),
            ("price".to_string(), vec!["section] *".to_string()]),
            ("price".to_string(), vec!["s".repeat(129)]),
            (
                "price".to_string(),
                (0..129).map(|n| format!("section-{n}")).collect(),
            ),
        ] {
            assert!(
                fetcher
                    .fetch_cart_preparation(event, "category", 2, &price, &sections)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("bounded unique configured")
            );
        }
        assert!(
            fetcher
                .fetch_cart_preparation(event, "category", 0, "price", &[])
                .await
                .unwrap_err()
                .to_string()
                .contains("bounded category ID")
        );
        assert!(
            fetcher
                .fetch_cart_preparation(
                    "https://www.ticketmaster.pl/event/synthetic/12345678",
                    "category",
                    2,
                    "price",
                    &[]
                )
                .await
                .unwrap_err()
                .to_string()
                .contains("public Czech event URL")
        );
        assert!(fetcher.browser.lock().await.is_none());
    }

    #[tokio::test]
    async fn cart_preparation_protocol_binds_requested_filters_and_rejects_unsolicited_or_failed_reports()
     {
        let mut script = tempfile::NamedTempFile::new().unwrap();
        script.write_all(br#"
            require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
                const r=JSON.parse(line),kind=new URL(r.url).pathname.split('/')[2];
                const preparation={price_id:r.cart_selection?.price_id||'price-6',section_ids:r.cart_selection?.section_ids||[]};
                if(kind==='wrong-price') preparation.price_id='price-7';
                if(kind==='wrong-sections') preparation.section_ids=['other'];
                if(kind==='string-sections') preparation.section_ids='1P';
                if(kind==='nonstring-section') preparation.section_ids=[1];
                const inspection={category_id:r.cart_category_id||'category',demo_quantity:r.cart_demo_quantity||2,
                    preparation:kind==='missing-preparation'?undefined:kind==='null-preparation'?null:preparation};
                process.stdout.write(JSON.stringify({id:r.id,status:kind==='http-blocked'?403:200,
                    html:JSON.stringify(r),final_url:r.url,retry_after_secs:120,
                    adapter_state:kind==='http-blocked'||kind==='blocked-with-report'?'blocked':kind==='unsupported'?'unsupported':'ready',
                    adapter_reason:kind==='private-reason'?'private-secret-marker':kind==='unsupported'?'cart_controls_changed':'cart_controls_ready',
                    cart_controls:['http-blocked','unsupported','missing-controls'].includes(kind)?undefined:inspection})+'\n');
            });
        "#).unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().to_owned();
        let fetcher = Fetcher::new(&config).unwrap();
        let event = "https://www.ticketmaster.cz/event/synthetic/12345678";
        let sections = vec!["1P".to_string(), "4P".to_string()];
        let page = fetcher
            .fetch_cart_preparation(event, "category", 2, "price-6", &sections)
            .await
            .unwrap();
        let sent: serde_json::Value = serde_json::from_str(&page.html).unwrap();
        assert_eq!(sent["display"], "cart_controls");
        assert_eq!(sent["cart_demo_quantity"], 2);
        assert_eq!(
            sent["cart_selection"],
            serde_json::json!({"price_id":"price-6","section_ids":["1P","4P"]})
        );
        assert!(sent.get("map_scan_limit").is_none());
        assert_eq!(
            page.cart_controls.unwrap()["preparation"]["price_id"],
            "price-6"
        );
        let all_sections = fetcher
            .fetch_cart_preparation(event, "category", 2, "price-6", &[])
            .await
            .unwrap();
        assert_eq!(
            all_sections.cart_controls.unwrap()["preparation"]["section_ids"],
            serde_json::json!([])
        );
        for (kind, reason) in [
            ("wrong-price", "target mismatch"),
            ("wrong-sections", "target mismatch"),
            ("string-sections", "target mismatch"),
            ("nonstring-section", "target mismatch"),
            ("null-preparation", "target mismatch"),
            ("missing-preparation", "omitted the requested cart filter"),
            ("missing-controls", "omitted the requested cart controls"),
            ("blocked-with-report", "lacks a ready document"),
        ] {
            let url = format!("https://www.ticketmaster.cz/event/{kind}/12345678");
            let error = fetcher
                .fetch_cart_preparation(&url, "category", 2, "price-6", &sections)
                .await
                .unwrap_err();
            assert!(error.to_string().contains(reason), "{kind}: {error}");
        }
        assert!(
            fetcher
                .fetch_cart_controls(event, "category", 2)
                .await
                .unwrap_err()
                .to_string()
                .contains("Unsolicited cart filter preparation")
        );
        for kind in ["http-blocked", "unsupported"] {
            let url = format!("https://www.ticketmaster.cz/event/{kind}/12345678");
            let page = fetcher
                .fetch_cart_preparation(&url, "category", 2, "price-6", &sections)
                .await
                .unwrap();
            assert!(page.cart_controls.is_none());
            assert_eq!(page.retry_after_secs, Some(120));
        }
        let page = fetcher
            .fetch_cart_preparation(
                "https://www.ticketmaster.cz/event/private-reason/12345678",
                "category",
                2,
                "price-6",
                &sections,
            )
            .await
            .unwrap();
        assert!(!page.detail.contains("private-secret-marker"));
        assert!(!page.detail.contains("cart controls="));
        fetcher.shutdown().await;
    }

    #[tokio::test]
    async fn cart_controls_protocol_is_exclusive_ready_and_bound_to_requested_demo() {
        let mut script = tempfile::NamedTempFile::new().unwrap();
        script.write_all(br#"
            require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
                const r=JSON.parse(line),kind=new URL(r.url).pathname.split('/')[2];
                const inspection={category_id:kind==='wrong-category'?'other':r.cart_category_id||'category',
                    demo_quantity:kind==='wrong-quantity'?20:r.cart_demo_quantity||2};
                process.stdout.write(JSON.stringify({id:r.id,status:kind==='plain-blocked'?403:200,
                    html:JSON.stringify(r),final_url:kind==='wrong-event'?r.url.replace('12345678','87654321'):r.url,
                    adapter_state:['blocked','plain-blocked'].includes(kind)?'blocked':kind==='unsupported'?'unsupported':'ready',retry_after_secs:120,
                    adapter_reason:kind==='private-reason'?'untrusted-worker-reason-with-secret-marker':kind==='unsupported'?'cart_controls_changed':'cart_controls_ready',
                    cart_controls:['missing','plain-blocked','unsupported'].includes(kind)?undefined:inspection,
                    map_scan:kind==='map'?{section_id:'s_1',section_name:'101',limit:20,
                        started_at_ms:1000,finished_at_ms:1001,candidate_count:0,visible_candidate_count:0,
                        results:[],stop_reason:'visible_candidates_exhausted',coverage:'partial_rendered_section',
                        stock_verified:false,adjacency_verified:false}:undefined})+'\n');
            });
        "#).unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().to_owned();
        let fetcher = Fetcher::new(&config).unwrap();
        let result = fetcher
            .fetch_cart_controls(
                "https://www.ticketmaster.cz/event/synthetic/12345678",
                "category",
                2,
            )
            .await
            .unwrap();
        let request: serde_json::Value = serde_json::from_str(&result.html).unwrap();
        assert_eq!(request["display"], "cart_controls");
        assert_eq!(request["adapter"], "ticketmaster");
        assert_eq!(request["cart_category_id"], "category");
        assert_eq!(request["cart_demo_quantity"], 2);
        assert!(request.get("cart_selection").is_none());
        assert!(request.get("map_section_id").is_none());
        assert!(request.get("map_scan_limit").is_none());
        assert_eq!(result.cart_controls.unwrap()["demo_quantity"], 2);
        assert!(result.detail.contains("cart controls=cart_controls_ready"));
        for (kind, expected) in [
            ("wrong-category", "target or quantity mismatch"),
            ("wrong-quantity", "target or quantity mismatch"),
            ("wrong-event", "do not match"),
            ("blocked", "lacks a ready document"),
            ("missing", "omitted the requested cart"),
            ("map", "Unsolicited map scan"),
        ] {
            let event = format!("https://www.ticketmaster.cz/event/{kind}/12345678");
            let error = fetcher
                .fetch_cart_controls(&event, "category", 2)
                .await
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{kind}: {error}");
        }
        let ordinary = "https://www.ticketmaster.cz/event/unsolicited/12345678";
        assert!(
            fetcher
                .fetch_url(ordinary, FetchMode::Browser)
                .await
                .unwrap_err()
                .to_string()
                .contains("Unsolicited cart controls")
        );
        assert!(
            fetcher
                .fetch_map(ordinary)
                .await
                .unwrap_err()
                .to_string()
                .contains("Unsolicited cart controls")
        );
        let blocked = fetcher
            .fetch_cart_controls(
                "https://www.ticketmaster.cz/event/plain-blocked/12345678",
                "category",
                2,
            )
            .await
            .unwrap();
        assert_eq!(blocked.status, 403);
        assert_eq!(blocked.retry_after_secs, Some(120));
        assert!(blocked.cart_controls.is_none());
        let unknown = fetcher
            .fetch_cart_controls(
                "https://www.ticketmaster.cz/event/private-reason/12345678",
                "category",
                2,
            )
            .await
            .unwrap();
        assert!(!unknown.detail.contains("secret-marker"));
        assert!(!unknown.detail.contains("cart controls="));
        let unsupported = fetcher
            .fetch_cart_controls(
                "https://www.ticketmaster.cz/event/unsupported/12345678",
                "category",
                2,
            )
            .await
            .unwrap();
        assert!(
            unsupported
                .detail
                .contains("cart controls=cart_controls_changed")
        );
        fetcher.shutdown().await;
    }

    #[tokio::test]
    async fn browser_manual_intervention_blocks_both_adapters_with_fixed_diagnostics() {
        let mut script = tempfile::NamedTempFile::new().unwrap();
        script
            .write_all(
                br#"
            const readline = require('node:readline');
            readline.createInterface({input:process.stdin}).on('line', line => {
                const r = JSON.parse(line);
                process.stdout.write(JSON.stringify({id:r.id,status:r.id >= 6 ? 401 : r.id >= 4 ? 403 : 200,
                    html:r.id >= 4 ? '' : '<button>Tickets</button>',retry_after_secs:r.id >= 6 ? 0 : 90,
                    final_url:r.url,adapter_state:'ready',manual_intervention:
                    r.id >= 6 ? 'identification_incomplete' : r.id >= 4 ? 'ticketmaster_protection' :
                    r.id === 3 ? 'secret-untrusted-worker-value' : 'captcha'})+'\n');
            });
        "#,
            )
            .unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().into();
        let fetcher = Fetcher::new(&config).unwrap();
        let mut watch = Watch {
            url: "https://www.ticketmaster.cz/event/synthetic/12345678".into(),
            mode: FetchMode::Browser,
            ..Watch::default()
        };
        for adapter in [PageAdapter::Selectors, PageAdapter::Ticketmaster] {
            watch.parser.adapter = adapter;
            let result = fetcher.fetch(&watch).await.unwrap();
            assert_eq!(result.adapter_state.as_deref(), Some("blocked"));
            assert!(
                result
                    .detail
                    .contains("manual intervention required: human verification")
            );
        }
        let unknown = fetcher.fetch(&watch).await.unwrap();
        assert_eq!(unknown.adapter_state.as_deref(), Some("blocked"));
        assert!(unknown.detail.contains("unrecognized browser restriction"));
        assert!(!unknown.detail.contains("secret-untrusted"));
        for adapter in [PageAdapter::Selectors, PageAdapter::Ticketmaster] {
            watch.parser.adapter = adapter;
            let blocked = fetcher.fetch(&watch).await.unwrap();
            assert_eq!(blocked.status, 403);
            assert!(blocked.html.is_empty());
            assert_eq!(blocked.retry_after_secs, Some(90));
            assert_eq!(blocked.adapter_state.as_deref(), Some("blocked"));
            assert!(blocked.detail.contains(
                "manual intervention required: Ticketmaster access-protection block; exact trigger undisclosed"
            ));
        }
        for adapter in [PageAdapter::Selectors, PageAdapter::Ticketmaster] {
            watch.parser.adapter = adapter;
            let blocked = fetcher.fetch(&watch).await.unwrap();
            assert_eq!(blocked.status, 401);
            assert!(blocked.html.is_empty());
            assert_eq!(blocked.retry_after_secs, Some(0));
            assert_eq!(blocked.final_url, watch.url);
            assert_eq!(blocked.adapter_state.as_deref(), Some("blocked"));
            assert!(blocked.detail.contains(
                "Ticketmaster identification incomplete; last confirmed error response retained"
            ));
        }
        fetcher.shutdown().await;
    }

    #[tokio::test]
    async fn watched_fetch_rejects_other_event_but_preserves_slug_redirects_and_http_blocks() {
        // A protocol stub exercises the actual Fetcher::fetch guard without a website request.
        let mut script = tempfile::NamedTempFile::new().unwrap();
        script
            .write_all(
                br#"
            const readline = require('node:readline');
            const lines = readline.createInterface({input:process.stdin});
            lines.on('line', line => {
                const r = JSON.parse(line);
                const wrong = r.url.replace('/496543017', '/999999999');
                const finalUrl = r.id === 2 ? r.url.replace('/korn-bilety/', '/new-slug/') : wrong;
                process.stdout.write(JSON.stringify({id:r.id,status:r.id === 3 ? 403 : 200,
                    html:r.id === 3 ? '' : '<div class="offer">Tickets</div>',final_url:finalUrl,elapsed_ms:1})+'\n');
            });
        "#,
            )
            .unwrap();
        let mut config = Config::default();
        config.browser.script = script.path().into();
        let fetcher = Fetcher::new(&config).unwrap();
        let watch = Watch {
            url: "https://www.ticketmaster.pl/event/korn-bilety/496543017".into(),
            mode: FetchMode::Browser,
            ..Watch::default()
        };
        let error = fetcher.fetch(&watch).await.unwrap_err().to_string();
        assert_eq!(
            error,
            "Fetched page does not match the watched event; inventory unknown"
        );
        assert!(!error.contains("999999999"));
        let renamed = fetcher.fetch(&watch).await.unwrap();
        assert!(renamed.final_url.contains("/new-slug/496543017"));
        let blocked = fetcher.fetch(&watch).await.unwrap();
        assert_eq!(blocked.status, 403);
        assert!(blocked.html.is_empty());
        fetcher.shutdown().await;
    }
}
