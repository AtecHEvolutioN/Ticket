//! One-shot native UI orchestration. The journal commit precedes the only
//! stock-affecting command; all subsequent commands only observe that session.
//! Configuration prices remain provisional until a real receipt supplies seats,
//! complete fees, and (separately) sufficient cheapest-group evidence.
use std::{future::Future, path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use crate::{
    cart,
    cart_journal::{
        AttemptRecord, CartJournal, NativeSearchOutcome, NativeSearchReconciliation,
        NativeSearchRequest,
    },
    cart_preferences::{self, PreferencesDocument},
    event_identity,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionPrepareRequest {
    pub event_url: String,
    pub expected_event_start: String,
    pub category_id: String,
    pub price_id: String,
    pub section_ids: Vec<String>,
    pub quantity: u32,
    pub configured_unit_price_minor: u64,
}

impl SessionPrepareRequest {
    pub fn validate(&self) -> Result<()> {
        validate_selection(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionPreflight {
    pub session_id: String,
    pub document_id: String,
    pub config_id: String,
    pub preflight_id: String,
    pub config_observed_at_ms: i64,
    pub config_expires_at_ms: i64,
    pub event_url: String,
    pub source_event_start: String,
    pub category_id: String,
    pub category_title: String,
    pub quantity: u32,
    pub price_id: String,
    pub configured_unit_price_minor: u64,
    pub currency: String,
    pub section_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Prepared,
    NeedsHuman,
    Uncertain,
    Closed,
}

/// A diagnostic from the current native page, never proof of sold-out inventory
/// or confirmation that the attempted request created no hold.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeSignal {
    InsufficientTicketsReported,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionReply {
    pub state: SessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<SessionPreflight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<NativeSearchOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_intervention: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_signal: Option<NativeSignal>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SessionCommand {
    Prepare {
        request: SessionPrepareRequest,
    },
    Refresh,
    ResumePreparation,
    Commit {
        attempt_id: String,
        preflight_id: String,
        dispatch_token: String,
        /// Effective preference from the durable dispatch binding. The browser
        /// uses it only to diagnose conflicting native split-seat requests.
        adjacent_required: bool,
    },
    Observe {
        attempt_id: String,
    },
    Close,
}

/// Implementations must retain one browser context and document lineage. Commit
/// consumes its own in-memory dispatch latch before touching Find tickets.
pub trait NativeCartSession {
    fn exchange(&mut self, command: SessionCommand) -> impl Future<Output = Result<SessionReply>>;
}

pub trait Clock {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

#[derive(Debug, Serialize)]
pub struct ControllerResult {
    pub attempt: AttemptRecord,
    pub dispatched_this_call: bool,
    pub session_state: SessionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual_intervention: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_signal: Option<NativeSignal>,
    /// A fixed code, never a browser exception that could expose a private URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation_error: Option<&'static str>,
}

pub struct NativeCartController<S, C = SystemClock> {
    journal: CartJournal,
    session: S,
    clock: C,
    selection: Option<SessionPrepareRequest>,
    preflight: Option<SessionPreflight>,
    last_preparation: Option<SessionReply>,
    lineage: Option<(String, String, String)>,
    attempt: Option<AttemptRecord>,
}

impl<S: NativeCartSession> NativeCartController<S> {
    pub fn new(journal: CartJournal, session: S) -> Self {
        Self::with_clock(journal, session, SystemClock)
    }
}

impl<S: NativeCartSession, C: Clock> NativeCartController<S, C> {
    pub fn with_clock(journal: CartJournal, session: S, clock: C) -> Self {
        Self {
            journal,
            session,
            clock,
            selection: None,
            preflight: None,
            last_preparation: None,
            lineage: None,
            attempt: None,
        }
    }

    pub async fn prepare(&mut self, request: &SessionPrepareRequest) -> Result<SessionReply> {
        ensure!(
            self.selection.is_none() && self.attempt.is_none(),
            "controller already owns a native session; only passive refresh is permitted"
        );
        validate_selection(request)?;
        let mut request = request.clone();
        request.expected_event_start = cart::normalize_event_start(&request.expected_event_start)
            .context("native performance start is invalid")?;
        self.selection = Some(request.clone());
        self.last_preparation = None;
        let mut reply = self
            .session
            .exchange(SessionCommand::Prepare { request })
            .await?;
        reply.manual_intervention = safe_intervention(reply.manual_intervention);
        self.accept_preflight(&reply)?;
        self.last_preparation = Some(reply.clone());
        Ok(reply)
    }

    /// Rechecks the already prepared document without opening another event page.
    pub async fn refresh(&mut self) -> Result<SessionReply> {
        ensure!(
            self.selection.is_some() && self.attempt.is_none(),
            "refresh requires an undispatched native session"
        );
        self.last_preparation = None;
        let mut reply = self.session.exchange(SessionCommand::Refresh).await?;
        reply.manual_intervention = safe_intervention(reply.manual_intervention);
        self.accept_preflight(&reply)?;
        self.last_preparation = Some(reply.clone());
        Ok(reply)
    }

    /// Continue only an undispatched preparation in the same retained browser.
    /// The native adapter revalidates the current page; this never reloads it or
    /// submits Find tickets. An established source lineage remains immutable.
    pub async fn resume_preparation(&mut self) -> Result<SessionReply> {
        ensure!(
            self.selection.is_some() && self.attempt.is_none(),
            "resume requires an undispatched native session"
        );
        self.last_preparation = None;
        let mut reply = self
            .session
            .exchange(SessionCommand::ResumePreparation)
            .await?;
        reply.manual_intervention = safe_intervention(reply.manual_intervention);
        self.accept_preflight(&reply)?;
        self.last_preparation = Some(reply.clone());
        Ok(reply)
    }

    /// Read-only local state for an operator; no browser command is sent.
    pub fn cached_preparation(&self) -> Option<&SessionReply> {
        self.last_preparation.as_ref()
    }

    /// Includes a dispatch marker even if the asynchronous native command was
    /// interrupted before its response. This never implies a verified hold.
    pub fn current_attempt(&self) -> Option<&AttemptRecord> {
        self.attempt.as_ref()
    }

    fn accept_preflight(&mut self, reply: &SessionReply) -> Result<()> {
        ensure!(
            reply.outcome.is_none() && reply.native_signal.is_none(),
            "preparation unexpectedly supplied a post-action outcome"
        );
        if reply.state != SessionState::Prepared {
            self.preflight = None;
            ensure!(
                reply.state == SessionState::NeedsHuman && reply.preflight.is_none(),
                "invalid native preparation state"
            );
            return Ok(());
        }
        let preflight = reply
            .preflight
            .as_ref()
            .context("native preparation omitted its binding")?;
        validate_preflight(
            self.selection
                .as_ref()
                .context("native selection missing")?,
            preflight,
            self.clock.now_ms(),
        )?;
        let lineage = (
            preflight.session_id.clone(),
            preflight.document_id.clone(),
            preflight.config_id.clone(),
        );
        if let Some(previous) = &self.lineage {
            ensure!(
                previous == &lineage,
                "prepared session, document or configuration changed"
            );
        }
        self.lineage = Some(lineage);
        self.preflight = Some(preflight.clone());
        Ok(())
    }

    /// Cancellation or process failure after the durable marker leaves an active
    /// event lock. Calling this again cannot replay an ambiguous native action.
    pub async fn dispatch_once(
        &mut self,
        attempt_id: &str,
        preferences: &PreferencesDocument,
        live_reservations: bool,
    ) -> Result<ControllerResult> {
        ensure!(
            live_reservations,
            "native submission requires explicit live execution"
        );
        ensure!(identifier(attempt_id), "invalid native attempt ID");
        if let Some(attempt) = &self.attempt {
            ensure!(
                attempt.attempt_id == attempt_id,
                "controller is already bound to another attempt"
            );
            return Ok(result(
                attempt.clone(),
                false,
                SessionState::Uncertain,
                None,
                None,
            ));
        }
        let refreshed = self.refresh().await?;
        ensure!(
            refreshed.state == SessionState::Prepared,
            "native session needs human intervention before dispatch"
        );
        let preflight = self
            .preflight
            .as_ref()
            .context("native session needs human intervention before dispatch")?
            .clone();
        let selection = self
            .selection
            .as_ref()
            .context("native selection missing")?;
        let request = request_from_preflight(selection, &preflight, preferences)?;
        let prepared = self
            .journal
            .begin_search(attempt_id, &request, self.clock.now_ms())?;
        // Existing attempts, including crash recovery, are never dispatched here.
        // A caller must not try to reconstruct a live browser from an old binding.
        if prepared.dispatch_at_ms.is_some() {
            self.attempt = Some(prepared.clone());
            return Ok(result(prepared, false, SessionState::Uncertain, None, None));
        }
        let dispatch = self
            .journal
            .mark_search_dispatch_started(attempt_id, self.clock.now_ms())?;
        self.attempt = Some(dispatch.record.clone());
        if !dispatch.newly_started {
            return Ok(result(
                dispatch.record,
                false,
                SessionState::Uncertain,
                None,
                None,
            ));
        }
        // Both journal transactions have committed before any native commit byte.
        let adjacent_required = dispatch
            .record
            .search_binding()
            .context("native dispatch binding missing")?
            .request
            .adjacent_required;
        let response = self
            .session
            .exchange(SessionCommand::Commit {
                attempt_id: attempt_id.into(),
                preflight_id: preflight.preflight_id,
                dispatch_token: attempt_id.into(),
                adjacent_required,
            })
            .await;
        self.accept_observation(response, None, true)
    }

    /// Passive observation in the same retained browser. It never submits or
    /// navigates, including after transport errors and manual challenges.
    pub async fn observe(&mut self, reconciliation_id: &str) -> Result<ControllerResult> {
        let attempt = self
            .attempt
            .as_ref()
            .context("no dispatched native attempt to observe")?;
        ensure!(
            attempt.dispatch_at_ms.is_some(),
            "native attempt was not dispatched"
        );
        let reply = self
            .session
            .exchange(SessionCommand::Observe {
                attempt_id: attempt.attempt_id.clone(),
            })
            .await;
        self.accept_observation(reply, Some(reconciliation_id), false)
    }

    fn accept_observation(
        &mut self,
        response: Result<SessionReply>,
        reconciliation_id: Option<&str>,
        dispatched_this_call: bool,
    ) -> Result<ControllerResult> {
        let previous = self
            .attempt
            .as_ref()
            .context("native attempt missing")?
            .clone();
        let now = self.clock.now_ms();
        // Advancing a rolled-back clock to the persisted timestamp could make
        // a future terminal receipt look current and release the event lock.
        // Keep the existing record until a trustworthy clock and a fresh
        // observation are available; this path must not rewrite the journal.
        if now < 0 || now as u64 > cart::MAX_EXACT_INTEGER || now < previous.updated_at_ms {
            return Ok(result(
                previous,
                dispatched_this_call,
                SessionState::Uncertain,
                None,
                Some("native_clock_needs_review"),
            ));
        }
        let (state, intervention, signal, outcome, mut error) = match response {
            Ok(reply)
                if matches!(
                    reply.state,
                    SessionState::Uncertain | SessionState::NeedsHuman
                ) && reply.preflight.is_none() =>
            {
                (
                    reply.state,
                    safe_intervention(reply.manual_intervention),
                    if matches!(reply.outcome, Some(NativeSearchOutcome::Uncertain(_))) {
                        reply.native_signal
                    } else {
                        None
                    },
                    reply.outcome,
                    None,
                )
            }
            Ok(_) => (
                SessionState::Uncertain,
                None,
                None,
                None,
                Some("invalid_native_observation"),
            ),
            Err(_) => (
                SessionState::Uncertain,
                None,
                None,
                None,
                Some("native_transport_uncertain"),
            ),
        };
        let updated = if let Some(outcome) = outcome {
            let recorded = if let Some(hash) = &previous.native_outcome_hash {
                match reconciliation_id {
                    Some(id) => self.journal.reconcile_search(
                        &previous.attempt_id,
                        &NativeSearchReconciliation {
                            reconciliation_id: id.into(),
                            expected_outcome_hash: hash.clone(),
                            outcome,
                        },
                        now,
                    ),
                    None => anyhow::bail!("initial native outcome already exists"),
                }
            } else {
                self.journal
                    .record_search_outcome(&previous.attempt_id, &outcome, now)
            };
            match recorded {
                Ok(record) => record,
                Err(_) => {
                    error = Some("native_outcome_rejected");
                    previous.clone()
                }
            }
        } else {
            previous.clone()
        };
        // Do not overwrite a previously observed hold or confirmed terminal state
        // with a transport failure. Initial uncertainty remains durably locked.
        let updated = if updated.native_outcome_hash.is_none() {
            self.journal.mark_uncertain(&updated.attempt_id, now)?
        } else {
            updated
        };
        self.attempt = Some(updated.clone());
        let mut result = result(updated, dispatched_this_call, state, intervention, error);
        // A rejected observation must not attach a fresh-page diagnostic to the
        // older journal record retained for recovery.
        result.native_signal = if error.is_none() { signal } else { None };
        Ok(result)
    }

    /// Closing the browser does not release a hold or clear the durable event lock.
    pub async fn close(&mut self) -> Result<SessionReply> {
        self.session.exchange(SessionCommand::Close).await
    }
}

fn result(
    attempt: AttemptRecord,
    dispatched_this_call: bool,
    session_state: SessionState,
    manual_intervention: Option<String>,
    observation_error: Option<&'static str>,
) -> ControllerResult {
    ControllerResult {
        attempt,
        dispatched_this_call,
        session_state,
        manual_intervention,
        native_signal: None,
        observation_error,
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':'))
}

fn validate_selection(request: &SessionPrepareRequest) -> Result<()> {
    ensure!(
        event_identity::identity(&request.event_url)
            .is_some_and(|(host, _)| host == "ticketmaster.cz"),
        "native cart session requires a public Czech event"
    );
    ensure!(
        cart::normalize_event_start(&request.expected_event_start).is_some(),
        "native cart session requires an exact event start with timezone"
    );
    let native_id = |value: &str| {
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    };
    ensure!(
        native_id(&request.category_id)
            && native_id(&request.price_id)
            && request.section_ids.len() <= 128
            && request.section_ids.iter().all(|id| native_id(id))
            && request
                .section_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == request.section_ids.len(),
        "invalid native configured selection"
    );
    ensure!(
        (1..=cart::MAX_QUANTITY).contains(&request.quantity)
            && request.configured_unit_price_minor <= cart::MAX_EXACT_INTEGER,
        "invalid native quantity or price"
    );
    Ok(())
}

fn validate_preflight(
    selection: &SessionPrepareRequest,
    preflight: &SessionPreflight,
    now: i64,
) -> Result<()> {
    for id in [
        &preflight.session_id,
        &preflight.document_id,
        &preflight.config_id,
        &preflight.preflight_id,
    ] {
        ensure!(identifier(id), "invalid native application correlation ID");
    }
    ensure!(
        event_identity::same_event(&selection.event_url, &preflight.event_url)
            && cart::normalize_event_start(&selection.expected_event_start)
                == cart::normalize_event_start(&preflight.source_event_start),
        "native event or performance start mismatch"
    );
    ensure!(
        selection.category_id == preflight.category_id
            && selection.price_id == preflight.price_id
            && selection.quantity == preflight.quantity
            && selection.configured_unit_price_minor == preflight.configured_unit_price_minor
            && selection
                .section_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                == preflight
                    .section_ids
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
            && selection.section_ids.len() == preflight.section_ids.len()
            && preflight.currency == "CZK",
        "native selection differs from requested form"
    );
    ensure!(
        preflight.config_observed_at_ms >= 0
            && preflight.config_observed_at_ms <= now
            && now < preflight.config_expires_at_ms
            && now - preflight.config_observed_at_ms < cart::MAX_QUOTE_AGE_MS,
        "native preflight is not fresh"
    );
    Ok(())
}

fn request_from_preflight(
    selection: &SessionPrepareRequest,
    preflight: &SessionPreflight,
    preferences: &PreferencesDocument,
) -> Result<NativeSearchRequest> {
    let request = NativeSearchRequest {
        event_url: preflight.event_url.clone(),
        expected_event_start: selection.expected_event_start.clone(),
        source_event_start: preflight.source_event_start.clone(),
        quantity: preflight.quantity,
        session_id: preflight.session_id.clone(),
        document_id: preflight.document_id.clone(),
        config_id: preflight.config_id.clone(),
        config_observed_at_ms: preflight.config_observed_at_ms,
        config_expires_at_ms: preflight.config_expires_at_ms,
        category_id: preflight.category_id.clone(),
        category_title: Some(preflight.category_title.clone()),
        filters: Default::default(),
        price_id: preflight.price_id.clone(),
        configured_unit_price_minor: preflight.configured_unit_price_minor,
        currency: preflight.currency.clone(),
        max_total_minor: None,
        adjacent_required: true,
        cheapest_required: true,
    };
    let prepared = cart_preferences::apply_to_native_search(preferences, &request)?;
    ensure!(
        prepared.request.quantity == preflight.quantity,
        "native form quantity differs from selected preferences"
    );
    Ok(prepared.request)
}

fn safe_intervention(value: Option<String>) -> Option<String> {
    value.map(|v| match v.as_str() {
        "captcha"
        | "queue"
        | "session_expired"
        | "authentication_required"
        | "access_blocked"
        | "verification"
        | "ticketmaster_protection"
        | "identification_incomplete"
        | "non_adjacent"
        | "modal" => v,
        _ => "unknown".into(),
    })
}

pub struct NativeBrowserOptions {
    pub node: String,
    pub script: PathBuf,
    pub executable_path: Option<String>,
    pub headed: bool,
    pub evidence_dir: PathBuf,
    pub timeout: Duration,
}

/// A separate retained JSON-lines process, never the stateless capture worker.
/// A timed-out channel is poisoned: a late commit reply cannot be mistaken for
/// the reply to a subsequent observation. The browser remains passive until close.
pub struct NativeBrowserSession {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    timeout: Duration,
    poisoned: bool,
}

impl NativeBrowserSession {
    pub fn spawn(options: &NativeBrowserOptions) -> Result<Self> {
        ensure!(
            options.timeout >= Duration::from_secs(1)
                && options.timeout <= Duration::from_secs(180),
            "invalid native browser timeout"
        );
        let script = std::fs::canonicalize(&options.script)
            .context("native cart session worker is missing")?;
        let mut command = Command::new(&options.node);
        command
            .arg(script)
            .arg("--evidence-dir")
            .arg(&options.evidence_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear();
        for name in [
            "PATH",
            "HOME",
            "TMPDIR",
            "DISPLAY",
            "XAUTHORITY",
            "XDG_RUNTIME_DIR",
            "LANG",
            "LC_ALL",
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "NO_PROXY",
            "https_proxy",
            "http_proxy",
            "no_proxy",
            "NODE_EXTRA_CA_CERTS",
            // Managed runtimes may install Playwright in a shared module path.
            // NODE_OPTIONS remains excluded: it can execute arbitrary preload code.
            "NODE_PATH",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "PLAYWRIGHT_BROWSERS_PATH",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        if options.headed {
            command.arg("--headed");
        }
        if let Some(executable) = options
            .executable_path
            .clone()
            .or_else(|| std::env::var("CHROMIUM_PATH").ok())
        {
            command.arg("--executable-path").arg(executable);
        }
        // Dropping the controller must not issue a cancellation/navigation that
        // could obscure an ambiguous hold. Explicit close is a separate command.
        command.kill_on_drop(false);
        let mut child = command.spawn().context("start native browser session")?;
        let input = child
            .stdin
            .take()
            .context("native browser stdin unavailable")?;
        let output = BufReader::new(
            child
                .stdout
                .take()
                .context("native browser stdout unavailable")?,
        );
        Ok(Self {
            child,
            input,
            output,
            next_id: 1,
            timeout: options.timeout,
            poisoned: false,
        })
    }

    pub fn process_id(&self) -> Option<u32> {
        self.child.id()
    }

    async fn send(&mut self, command: SessionCommand) -> Result<SessionReply> {
        let closing = matches!(command, SessionCommand::Close);
        if self.poisoned && matches!(command, SessionCommand::Close) {
            // Closing is safe even with an unread commit response. Do not try to
            // correlate that response or send any new action through this pipe.
            let encoded = format!("{{\"id\":{},\"op\":\"close\"}}\n", self.next_id);
            let status = tokio::time::timeout(Duration::from_secs(10), async {
                self.input.write_all(encoded.as_bytes()).await?;
                self.input.flush().await?;
                self.child.wait().await
            })
            .await
            .context("native close timed out; passive worker cleanup pending")?
            .context("native close failed; passive worker cleanup pending")?;
            ensure!(
                status.success(),
                "native browser exited without successful cleanup"
            );
            return Ok(SessionReply {
                state: SessionState::Closed,
                preflight: None,
                outcome: None,
                manual_intervention: None,
                native_signal: None,
            });
        }
        ensure!(
            !self.poisoned,
            "native session channel is uncertain; no further commands may be sent"
        );
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("native command ID exhausted")?;
        let mut value = serde_json::to_value(command)?;
        value
            .as_object_mut()
            .context("invalid native command")?
            .insert("id".into(), id.into());
        let mut encoded = serde_json::to_vec(&value)?;
        encoded.push(b'\n');
        // Mark before the first await: cancellation also poisons this channel.
        self.poisoned = true;
        let response = tokio::time::timeout(self.timeout, async {
            self.input
                .write_all(&encoded)
                .await
                .context("native command write failed")?;
            self.input
                .flush()
                .await
                .context("native command flush failed")?;
            let mut line = Vec::new();
            loop {
                let available = self
                    .output
                    .fill_buf()
                    .await
                    .context("native reply read failed")?;
                ensure!(!available.is_empty(), "native session output closed");
                let consumed = available
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(available.len(), |i| i + 1);
                ensure!(
                    line.len().saturating_add(consumed) <= cart::MAX_INPUT_BYTES + 65_536,
                    "native session response exceeds limit"
                );
                let complete = available[consumed - 1] == b'\n';
                line.extend_from_slice(&available[..consumed]);
                self.output.consume(consumed);
                if complete {
                    break;
                }
            }
            let mut value: serde_json::Value = serde_json::from_slice(&line)
                .map_err(|_| anyhow::anyhow!("invalid native response JSON"))?;
            let object = value.as_object_mut().context("invalid native response")?;
            ensure!(
                object.remove("id").and_then(|v| v.as_u64()) == Some(id),
                "native response ID mismatch"
            );
            match object.remove("ok").and_then(|v| v.as_bool()) {
                Some(true) => {}
                Some(false) => {
                    let code = object.remove("error");
                    let known = [
                        "invalid_request",
                        "invalid_state",
                        "invalid_commit",
                        "checkout_policy_unavailable",
                        "preflight_expired",
                        "evidence_unavailable",
                        "navigation_failed",
                        "unsafe_redirect",
                        "native_form_changed",
                        "native_form_timeout",
                        "native_form_unrecognized",
                        "native_modal_blocked",
                        "event_date_changed",
                        "cart_controls_unrecognized",
                        "session_failed",
                    ];
                    let code = code
                        .as_ref()
                        .and_then(|value| value.as_str())
                        .filter(|value| known.contains(value))
                        .unwrap_or("session_failed");
                    anyhow::bail!("native worker rejected command: {code}");
                }
                None => anyhow::bail!("native response omitted command status"),
            }
            let mut reply: SessionReply = serde_json::from_value(value)
                .map_err(|_| anyhow::anyhow!("invalid native response fields"))?;
            reply.manual_intervention = safe_intervention(reply.manual_intervention);
            Ok::<_, anyhow::Error>(reply)
        })
        .await
        .context("native session command timed out")??;
        if closing {
            ensure!(
                response.state == SessionState::Closed,
                "native close was not confirmed"
            );
            let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
                .await
                .context("native browser cleanup timed out")?
                .context("native browser cleanup failed")?;
            ensure!(
                status.success(),
                "native browser exited without successful cleanup"
            );
        }
        self.poisoned = false;
        Ok(response)
    }
}

impl NativeCartSession for NativeBrowserSession {
    async fn exchange(&mut self, command: SessionCommand) -> Result<SessionReply> {
        self.send(command).await
    }
}
