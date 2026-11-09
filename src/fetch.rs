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
                self.browser(safe_url.as_str(), watch.parser.adapter)
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
                self.browser(safe_url.as_str(), PageAdapter::Selectors)
                    .await
            }
        }
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

    async fn browser(&self, url: &str, adapter: PageAdapter) -> Result<FetchResult> {
        // One context and child process are reused. The worker processes one navigation at a time.
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
struct BrowserRequest<'a> {
    id: u64,
    url: &'a str,
    timeout_ms: u64,
    max_body_bytes: usize,
    render_wait_ms: u64,
    adapter: PageAdapter,
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
    manual_intervention: Option<ManualIntervention>,
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
            return Ok(FetchResult {
                status,
                html,
                final_url,
                retry_after_secs: reply.retry_after_secs,
                elapsed_ms: reply.elapsed_ms.unwrap_or(0),
                detail,
                adapter_state,
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
