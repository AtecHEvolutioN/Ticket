//! A bounded, foreground owner for one native browser session. Terminal commands
//! never turn an already dispatched attempt into another Find tickets action.
use std::{future::Future, pin::Pin, time::Duration};

use anyhow::{Result, ensure};
use serde::Serialize;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    time::{Instant, sleep_until},
};

use crate::{
    cart_controller::{
        Clock, ControllerResult, NativeCartController, NativeCartSession, SessionPrepareRequest,
        SessionReply, SessionState,
    },
    cart_journal::{AttemptRecord, AttemptState},
    cart_preferences::PreferencesDocument,
};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(15);
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_COMMAND_BYTES: usize = 64;

pub struct OperatorOptions {
    pub attempt_id: String,
    pub selection: SessionPrepareRequest,
    pub preferences: PreferencesDocument,
    pub timeout: Duration,
}

impl OperatorOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.timeout >= Duration::from_secs(1) && self.timeout <= Duration::from_secs(1800),
            "operator window must be between one and 1800 seconds"
        );
        ensure!(
            !self.attempt_id.is_empty()
                && self.attempt_id.len() <= 128
                && self
                    .attempt_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':')),
            "invalid operator attempt ID"
        );
        self.selection.validate()?;
        let effective =
            crate::cart_preferences::resolve(&self.preferences, &self.selection.event_url)?;
        ensure!(
            effective.preferences.quantity == self.selection.quantity
                && effective.preferences.currency == "CZK",
            "operator selection differs from ticket preferences"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorPhase {
    Preparing,
    AwaitingHuman,
    Dispatching,
    Observing,
    Reviewing,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorExitReason {
    CloseCommand,
    InputClosed,
    Cancelled,
    Deadline,
    SessionError,
    OutputError,
}

#[derive(Debug, Serialize)]
pub struct OperatorReport {
    pub exit_reason: OperatorExitReason,
    pub phase: OperatorPhase,
    /// A durable marker does not prove that the native button received a click.
    pub dispatch_recorded: bool,
    pub browser_closed: bool,
    /// Passive observation commands attempted, including interrupted reads.
    pub observations: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_preparation: Option<SessionReply>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result: Option<ControllerResult>,
    /// Retained even when cancellation interrupted the dispatch response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_attempt: Option<AttemptRecord>,
    /// A fixed diagnostic; never a raw browser or transport error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
}

impl OperatorReport {
    fn new() -> Self {
        Self {
            exit_reason: OperatorExitReason::SessionError,
            phase: OperatorPhase::Preparing,
            dispatch_recorded: false,
            browser_closed: false,
            observations: 0,
            last_preparation: None,
            last_result: None,
            current_attempt: None,
            error: None,
        }
    }

    fn absorb_result(&mut self, result: ControllerResult) {
        self.dispatch_recorded |= result.attempt.dispatch_at_ms.is_some();
        self.phase = if result.observation_error.is_some() || !can_poll(&result.attempt) {
            OperatorPhase::Reviewing
        } else {
            OperatorPhase::Observing
        };
        self.current_attempt = Some(result.attempt.clone());
        self.last_result = Some(result);
    }

    fn auto_observe(&self) -> bool {
        self.last_result
            .as_ref()
            .is_some_and(|result| result.observation_error.is_none() && can_poll(&result.attempt))
    }

    fn may_observe(&self) -> bool {
        self.last_result.as_ref().is_some_and(|result| {
            can_poll(&result.attempt)
                && result.observation_error != Some("native_transport_uncertain")
        })
    }
}

fn can_poll(attempt: &AttemptRecord) -> bool {
    attempt.dispatch_at_ms.is_some() && attempt.state == AttemptState::NeedsReconciliation
}

enum Command {
    Resume,
    Status,
    Close,
    Invalid,
    End,
}

/// State lives outside the future so cancellation by an idle observation tick
/// cannot discard half a command. Oversized and non-UTF8 input is never echoed.
struct Commands<R> {
    reader: R,
    bytes: Vec<u8>,
    oversized: bool,
}

impl<R: AsyncBufRead + Unpin> Commands<R> {
    async fn next(&mut self) -> std::io::Result<Command> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                self.bytes.clear();
                return Ok(Command::End);
            }
            let end = available.iter().position(|byte| *byte == b'\n');
            let count = end.map_or(available.len(), |i| i + 1);
            if !self.oversized {
                if self.bytes.len().saturating_add(count) > MAX_COMMAND_BYTES {
                    self.bytes.clear();
                    self.oversized = true;
                } else {
                    self.bytes.extend_from_slice(&available[..count]);
                }
            }
            self.reader.consume(count);
            if end.is_some() {
                let command = if self.oversized {
                    Command::Invalid
                } else {
                    match std::str::from_utf8(&self.bytes).map(str::trim) {
                        Ok("resume") => Command::Resume,
                        Ok("status") => Command::Status,
                        Ok("close") => Command::Close,
                        _ => Command::Invalid,
                    }
                };
                self.bytes.clear();
                self.oversized = false;
                return Ok(command);
            }
            tokio::task::yield_now().await;
        }
    }
}

enum Flow<T> {
    Value(T),
    Stop(OperatorExitReason, Option<&'static str>),
}

async fn emit<W: AsyncWrite + Unpin>(
    output: &mut W,
    value: serde_json::Value,
    deadline: Instant,
) -> bool {
    let Ok(mut bytes) = serde_json::to_vec(&value) else {
        return false;
    };
    bytes.push(b'\n');
    tokio::time::timeout_at(
        std::cmp::min(deadline, Instant::now() + OUTPUT_TIMEOUT),
        async {
            output.write_all(&bytes).await?;
            output.flush().await
        },
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

/// Keeps terminal cancellation live while a browser operation is pending.
/// Close/EOF buffered before dispatch wins over starting the native command.
async fn drive<T, F, R, W, C>(
    future: F,
    commands: &mut Commands<R>,
    output: &mut W,
    cancel: &mut Pin<Box<C>>,
    deadline: Instant,
    phase: OperatorPhase,
    error: &'static str,
) -> Flow<T>
where
    F: Future<Output = Result<T>>,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    C: Future<Output = ()>,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            _ = cancel.as_mut() => return Flow::Stop(OperatorExitReason::Cancelled, None),
            _ = sleep_until(deadline) => return Flow::Stop(OperatorExitReason::Deadline, None),
            command = commands.next() => {
                let notice = match command {
                    Ok(Command::Close) => return Flow::Stop(OperatorExitReason::CloseCommand, None),
                    Ok(Command::End) | Err(_) => return Flow::Stop(OperatorExitReason::InputClosed, None),
                    Ok(Command::Status) => "operation_in_progress",
                    Ok(Command::Resume) => "resume_already_in_progress",
                    Ok(Command::Invalid) => "unknown_command",
                };
                if !emit(output, serde_json::json!({"event":"operator_status","phase":phase,"notice":notice}), deadline).await {
                    return Flow::Stop(OperatorExitReason::OutputError, Some("operator_output_failed"));
                }
            }
            result = &mut future => return match result {
                Ok(value) => Flow::Value(value),
                Err(_) => Flow::Stop(OperatorExitReason::SessionError, Some(error)),
            }
        }
    }
}

enum DispatchStep {
    Paused(Box<SessionReply>),
    Dispatched(Box<ControllerResult>),
}

async fn dispatch<S: NativeCartSession, C: Clock>(
    controller: &mut NativeCartController<S, C>,
    options: &OperatorOptions,
) -> Result<DispatchStep> {
    match controller
        .dispatch_once(&options.attempt_id, &options.preferences, true)
        .await
    {
        Ok(result) => Ok(DispatchStep::Dispatched(Box::new(result))),
        Err(error) => {
            if controller.current_attempt().is_none()
                && let Some(reply) = controller.cached_preparation()
                && reply.state == SessionState::NeedsHuman
            {
                return Ok(DispatchStep::Paused(Box::new(reply.clone())));
            }
            Err(error)
        }
    }
}

/// Own one foreground session until explicit closure, EOF, cancellation or the
/// bounded deadline. The caller must enforce a headed browser and terminal IO;
/// this function is generic so its control flow can be tested without a provider.
/// `--execute` is checked by the CLI before entering this authorized workflow.
pub async fn run<S, C, R, W, F>(
    controller: &mut NativeCartController<S, C>,
    options: &OperatorOptions,
    input: R,
    mut output: W,
    cancel: F,
) -> Result<OperatorReport>
where
    S: NativeCartSession,
    C: Clock,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    F: Future<Output = ()>,
{
    let validated = options.validate();
    if let Err(error) = validated {
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, controller.close()).await;
        return Err(error);
    }
    let deadline = Instant::now() + options.timeout;
    let mut report = OperatorReport::new();
    let mut commands = Commands {
        reader: input,
        bytes: Vec::new(),
        oversized: false,
    };
    let mut cancel = Box::pin(cancel);
    let mut operation = Some(false);
    let mut next_observation = Instant::now() + POLL_INTERVAL;
    let mut announced = false;
    'session: loop {
        if !announced {
            if !emit(
                &mut output,
                serde_json::json!({"event":"operator_status",
                "phase":OperatorPhase::Preparing,"commands":["status","resume","close"]}),
                deadline,
            )
            .await
            {
                report.exit_reason = OperatorExitReason::OutputError;
                report.error = Some("operator_output_failed");
                break;
            }
            announced = true;
        }
        if let Some(resume) = operation.take() {
            report.phase = OperatorPhase::Preparing;
            let preparation = async {
                if resume {
                    controller.resume_preparation().await
                } else {
                    controller.prepare(&options.selection).await
                }
            };
            let reply = match drive(
                preparation,
                &mut commands,
                &mut output,
                &mut cancel,
                deadline,
                report.phase,
                "native_preparation_failed",
            )
            .await
            {
                Flow::Value(reply) => reply,
                Flow::Stop(reason, error) => {
                    report.exit_reason = reason;
                    report.error = error;
                    break;
                }
            };
            let prepared = reply.state == SessionState::Prepared;
            report.last_preparation = Some(reply);
            if prepared {
                report.phase = OperatorPhase::Dispatching;
                match drive(
                    dispatch(controller, options),
                    &mut commands,
                    &mut output,
                    &mut cancel,
                    deadline,
                    report.phase,
                    "native_dispatch_failed",
                )
                .await
                {
                    Flow::Value(DispatchStep::Dispatched(result)) => report.absorb_result(*result),
                    Flow::Value(DispatchStep::Paused(reply)) => {
                        report.phase = OperatorPhase::AwaitingHuman;
                        report.last_preparation = Some(*reply);
                    }
                    Flow::Stop(reason, error) => {
                        report.exit_reason = reason;
                        report.error = error;
                        break;
                    }
                }
            } else {
                report.phase = OperatorPhase::AwaitingHuman;
            }
            next_observation = Instant::now() + POLL_INTERVAL;
        }
        if !emit(
            &mut output,
            serde_json::json!({"event":"operator_status","phase":report.phase,
            "dispatch_recorded":report.dispatch_recorded,"preparation":report.last_preparation,
            "result":report.last_result,"commands":["status","resume","close"]}),
            deadline,
        )
        .await
        {
            report.exit_reason = OperatorExitReason::OutputError;
            report.error = Some("operator_output_failed");
            break;
        }
        loop {
            let command = tokio::select! {
                biased;
                _ = cancel.as_mut() => { report.exit_reason = OperatorExitReason::Cancelled; break 'session; }
                _ = sleep_until(deadline) => { report.exit_reason = OperatorExitReason::Deadline; break 'session; }
                command = commands.next() => match command {
                    Ok(command) => Some(command),
                    Err(_) => Some(Command::End),
                },
                _ = sleep_until(next_observation), if report.auto_observe() => None,
            };
            match command {
                Some(Command::Close) => {
                    report.exit_reason = OperatorExitReason::CloseCommand;
                    break 'session;
                }
                Some(Command::End) => {
                    report.exit_reason = OperatorExitReason::InputClosed;
                    break 'session;
                }
                Some(Command::Invalid) => {
                    if !emit(
                        &mut output,
                        serde_json::json!({"event":"operator_status","notice":"unknown_command",
                        "commands":["status","resume","close"]}),
                        deadline,
                    )
                    .await
                    {
                        report.exit_reason = OperatorExitReason::OutputError;
                        report.error = Some("operator_output_failed");
                        break 'session;
                    }
                    continue;
                }
                Some(Command::Resume) if !report.dispatch_recorded => {
                    operation = Some(true);
                    break;
                }
                Some(Command::Status) if !report.dispatch_recorded => break,
                Some(Command::Resume | Command::Status) | None => {}
            }
            if report.may_observe() {
                report.observations += 1;
                let reconciliation_id = format!("operator-observation-{}", report.observations);
                match drive(
                    controller.observe(&reconciliation_id),
                    &mut commands,
                    &mut output,
                    &mut cancel,
                    deadline,
                    report.phase,
                    "native_observation_failed",
                )
                .await
                {
                    Flow::Value(result) => report.absorb_result(result),
                    Flow::Stop(reason, error) => {
                        report.exit_reason = reason;
                        report.error = error;
                        break 'session;
                    }
                }
            }
            next_observation = Instant::now() + POLL_INTERVAL;
            break;
        }
    }
    report.current_attempt = controller.current_attempt().cloned();
    report.dispatch_recorded = report
        .current_attempt
        .as_ref()
        .is_some_and(|record| record.dispatch_at_ms.is_some());
    report.browser_closed = tokio::time::timeout(CLOSE_TIMEOUT, controller.close())
        .await
        .is_ok_and(|result| result.is_ok_and(|reply| reply.state == SessionState::Closed));
    Ok(report)
}
