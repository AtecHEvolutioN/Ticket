use std::{
    future::pending,
    io,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use anyhow::Result;
use ticket_watch::{
    cart_controller::*, cart_journal::*, cart_operator::*, cart_preferences::PreferencesDocument,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt, BufReader, DuplexStream},
    sync::{mpsc, oneshot},
    time::Instant,
};

const NOW: i64 = 1_800_000_000_000;
const EVENT: &str = "https://www.ticketmaster.cz/event/synthetic/1234567890";
const START: &str = "2027-01-02T18:00:00Z";

#[derive(Clone)]
struct TestClock {
    start: Instant,
    offset: Arc<AtomicI64>,
}
impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        NOW + self.start.elapsed().as_millis() as i64 + self.offset.load(Ordering::SeqCst)
    }
}
impl TestClock {
    fn observation(&self) -> i64 {
        self.offset.fetch_add(1, Ordering::SeqCst);
        self.now_ms()
    }
}

#[derive(Clone, Copy)]
enum Behavior {
    Ready,
    InitialHuman,
    RefreshHuman,
    ChangedResume,
    HangPrepare,
    HangCommit,
    LostCommit,
    Terminal,
    HangClose,
}

struct Session {
    path: PathBuf,
    clock: TestClock,
    behavior: Behavior,
    commands: Arc<Mutex<Vec<&'static str>>>,
    events: mpsc::UnboundedSender<&'static str>,
    refreshes: u32,
}

impl Session {
    fn prepared(&self, document: &str) -> SessionReply {
        let now = self.clock.observation();
        SessionReply {
            state: SessionState::Prepared,
            preflight: Some(SessionPreflight {
                session_id: "session-1".into(),
                document_id: document.into(),
                config_id: "config-1".into(),
                preflight_id: "preflight-1".into(),
                config_observed_at_ms: now,
                config_expires_at_ms: now + 5000,
                event_url: EVENT.into(),
                source_event_start: START.into(),
                category_id: "2".into(),
                category_title: "Sezení".into(),
                quantity: 2,
                price_id: "6".into(),
                configured_unit_price_minor: 188900,
                currency: "CZK".into(),
                section_ids: vec![],
            }),
            outcome: None,
            manual_intervention: None,
            native_signal: None,
        }
    }
    fn human(&self) -> SessionReply {
        SessionReply {
            state: SessionState::NeedsHuman,
            preflight: None,
            outcome: None,
            manual_intervention: Some("captcha".into()),
            native_signal: None,
        }
    }
    fn outcome(&self, terminal: bool) -> SessionReply {
        let context = NativeSearchReceiptContext {
            attempt_id: "operator-1".into(),
            event_url: terminal.then(|| EVENT.into()),
            event_start: terminal.then(|| START.into()),
            session_id: "session-1".into(),
            source_document_id: "document-1".into(),
            outcome_document_id: "document-1".into(),
            config_id: "config-1".into(),
            observed_at_ms: self.clock.observation(),
            alternate_event_or_date_reported: false,
        };
        let outcome = if terminal {
            NativeSearchOutcome::NoHold(NativeSearchNoHoldReceipt {
                context,
                receipt_correlation_id: "terminal-1".into(),
                absence_explicitly_confirmed: true,
                request_finished_without_hold: true,
            })
        } else {
            NativeSearchOutcome::Uncertain(NativeSearchUncertainReceipt {
                context,
                reason: NativeSearchUncertainty::Pending,
            })
        };
        SessionReply {
            state: SessionState::Uncertain,
            preflight: None,
            outcome: Some(outcome),
            manual_intervention: None,
            native_signal: None,
        }
    }
}

impl NativeCartSession for Session {
    async fn exchange(&mut self, command: SessionCommand) -> Result<SessionReply> {
        let name = match &command {
            SessionCommand::Prepare { .. } => "prepare",
            SessionCommand::Refresh => "refresh",
            SessionCommand::ResumePreparation => "resume",
            SessionCommand::Commit { .. } => "commit",
            SessionCommand::Observe { .. } => "observe",
            SessionCommand::Close => "close",
        };
        self.commands.lock().unwrap().push(name);
        self.events.send(name).unwrap();
        match command {
            SessionCommand::Prepare { .. } => match self.behavior {
                Behavior::HangPrepare => pending().await,
                Behavior::InitialHuman => Ok(self.human()),
                _ => Ok(self.prepared("document-1")),
            },
            SessionCommand::ResumePreparation => Ok(self.prepared(
                if matches!(self.behavior, Behavior::ChangedResume) {
                    "document-2"
                } else {
                    "document-1"
                },
            )),
            SessionCommand::Refresh => {
                self.refreshes += 1;
                if self.refreshes == 1
                    && matches!(
                        self.behavior,
                        Behavior::RefreshHuman | Behavior::ChangedResume
                    )
                {
                    Ok(self.human())
                } else {
                    Ok(self.prepared("document-1"))
                }
            }
            SessionCommand::Commit { attempt_id, .. } => {
                let rows = inspect_file(&self.path).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].attempt_id, attempt_id);
                assert!(
                    rows[0].dispatch_at_ms.is_some(),
                    "native commit must follow durable marker"
                );
                match self.behavior {
                    Behavior::HangCommit => pending().await,
                    Behavior::LostCommit => Err(anyhow::anyhow!("private-session-secret")),
                    Behavior::Terminal => Ok(self.outcome(true)),
                    _ => Ok(self.outcome(false)),
                }
            }
            SessionCommand::Observe { .. } => Ok(self.outcome(false)),
            SessionCommand::Close if matches!(self.behavior, Behavior::HangClose) => {
                pending().await
            }
            SessionCommand::Close => Ok(SessionReply {
                state: SessionState::Closed,
                preflight: None,
                outcome: None,
                manual_intervention: None,
                native_signal: None,
            }),
        }
    }
}

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl AsyncWrite for Output {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

type Controller = NativeCartController<Session, TestClock>;
struct Harness {
    _directory: tempfile::TempDir,
    path: PathBuf,
    controller: Controller,
    events: mpsc::UnboundedReceiver<&'static str>,
    commands: Arc<Mutex<Vec<&'static str>>>,
}
fn harness(behavior: Behavior) -> Harness {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cart.sqlite");
    let clock = TestClock {
        start: Instant::now(),
        offset: Arc::new(AtomicI64::new(0)),
    };
    let commands = Arc::new(Mutex::new(vec![]));
    let (sender, events) = mpsc::unbounded_channel();
    let session = Session {
        path: path.clone(),
        clock: clock.clone(),
        behavior,
        commands: commands.clone(),
        events: sender,
        refreshes: 0,
    };
    let controller =
        NativeCartController::with_clock(CartJournal::open(&path).unwrap(), session, clock);
    Harness {
        _directory: directory,
        path,
        controller,
        events,
        commands,
    }
}
fn options(timeout_secs: u64) -> OperatorOptions {
    OperatorOptions {
        attempt_id: "operator-1".into(),
        selection: SessionPrepareRequest {
            event_url: EVENT.into(),
            expected_event_start: START.into(),
            category_id: "2".into(),
            price_id: "6".into(),
            section_ids: vec![],
            quantity: 2,
            configured_unit_price_minor: 188900,
        },
        preferences: PreferencesDocument::default(),
        timeout: Duration::from_secs(timeout_secs),
    }
}
async fn event(events: &mut mpsc::UnboundedReceiver<&'static str>, expected: &str) {
    loop {
        if events.recv().await.unwrap() == expected {
            break;
        }
    }
}
async fn send(input: &mut DuplexStream, command: &str) {
    input.write_all(command.as_bytes()).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn ready_session_dispatches_once_and_post_dispatch_resume_only_observes() {
    let mut h = harness(Behavior::Ready);
    let (mut input, reader) = tokio::io::duplex(4096);
    let output = Output::default();
    let settings = options(30);
    let task = run(
        &mut h.controller,
        &settings,
        BufReader::new(reader),
        output.clone(),
        pending(),
    );
    let user = async {
        event(&mut h.events, "commit").await;
        send(&mut input, "resume\n").await;
        event(&mut h.events, "observe").await;
        send(&mut input, "status\n").await;
        event(&mut h.events, "observe").await;
        send(&mut input, "close\n").await;
    };
    let (report, ()) = tokio::join!(task, user);
    let report = report.unwrap();
    assert_eq!(report.exit_reason, OperatorExitReason::CloseCommand);
    assert!(report.dispatch_recorded && report.browser_closed);
    assert_eq!(report.observations, 2);
    assert_eq!(
        *h.commands.lock().unwrap(),
        [
            "prepare", "refresh", "commit", "observe", "observe", "close"
        ]
    );
    assert_eq!(
        inspect_search_history(&h.path, "operator-1").unwrap().len(),
        3
    );
    assert_eq!(
        inspect_file(&h.path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
}

#[tokio::test(start_paused = true)]
async fn initial_human_pause_retains_browser_and_status_does_not_prepare_or_submit() {
    let mut h = harness(Behavior::InitialHuman);
    let (mut input, reader) = tokio::io::duplex(4096);
    let output = Output::default();
    let settings = options(30);
    let task = run(
        &mut h.controller,
        &settings,
        BufReader::new(reader),
        output.clone(),
        pending(),
    );
    let user = async {
        event(&mut h.events, "prepare").await;
        send(&mut input, "status\n").await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(*h.commands.lock().unwrap(), ["prepare"]);
        assert!(inspect_file(&h.path).unwrap().is_empty());
        send(&mut input, "resume\n").await;
        event(&mut h.events, "commit").await;
        send(&mut input, "close\n").await;
    };
    let (report, ()) = tokio::join!(task, user);
    let report = report.unwrap();
    assert_eq!(report.exit_reason, OperatorExitReason::CloseCommand);
    assert_eq!(
        *h.commands.lock().unwrap(),
        ["prepare", "resume", "refresh", "commit", "close"]
    );
    assert!(output.text().contains("awaiting_human"));
}

#[tokio::test(start_paused = true)]
async fn challenge_during_final_refresh_can_resume_but_changed_lineage_cannot() {
    for behavior in [Behavior::RefreshHuman, Behavior::ChangedResume] {
        let mut h = harness(behavior);
        let (mut input, reader) = tokio::io::duplex(4096);
        let settings = options(30);
        let task = run(
            &mut h.controller,
            &settings,
            BufReader::new(reader),
            Output::default(),
            pending(),
        );
        let user = async {
            event(&mut h.events, "refresh").await;
            send(&mut input, "resume\n").await;
            event(
                &mut h.events,
                if matches!(behavior, Behavior::RefreshHuman) {
                    "commit"
                } else {
                    "close"
                },
            )
            .await;
            if matches!(behavior, Behavior::RefreshHuman) {
                send(&mut input, "close\n").await;
            }
        };
        let (report, ()) = tokio::join!(task, user);
        let report = report.unwrap();
        assert!(report.browser_closed);
        if matches!(behavior, Behavior::ChangedResume) {
            assert_eq!(report.exit_reason, OperatorExitReason::SessionError);
            assert_eq!(report.error, Some("native_preparation_failed"));
            assert!(!report.dispatch_recorded);
            assert!(inspect_file(&h.path).unwrap().is_empty());
        } else {
            assert!(report.dispatch_recorded);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_native_commit_retains_dispatch_and_closes_without_retry() {
    let mut h = harness(Behavior::HangCommit);
    let (_input, reader) = tokio::io::duplex(4096);
    let (cancel, cancelled) = oneshot::channel();
    let settings = options(30);
    let task = run(
        &mut h.controller,
        &settings,
        BufReader::new(reader),
        Output::default(),
        async {
            let _ = cancelled.await;
        },
    );
    let user = async {
        event(&mut h.events, "commit").await;
        cancel.send(()).unwrap();
    };
    let (report, ()) = tokio::join!(task, user);
    let report = report.unwrap();
    assert_eq!(report.exit_reason, OperatorExitReason::Cancelled);
    assert!(report.dispatch_recorded && report.browser_closed);
    assert!(report.last_result.is_none());
    assert!(report.current_attempt.unwrap().dispatch_at_ms.is_some());
    assert_eq!(
        *h.commands.lock().unwrap(),
        ["prepare", "refresh", "commit", "close"]
    );
    assert_eq!(
        inspect_file(&h.path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
}

#[tokio::test(start_paused = true)]
async fn deadline_and_eof_during_preparation_never_dispatch() {
    for eof in [false, true] {
        let mut h = harness(Behavior::HangPrepare);
        let (input, reader) = tokio::io::duplex(4096);
        let settings = options(5);
        let task = run(
            &mut h.controller,
            &settings,
            BufReader::new(reader),
            Output::default(),
            pending(),
        );
        let user = async {
            event(&mut h.events, "prepare").await;
            if eof {
                drop(input);
            } else {
                tokio::time::sleep(Duration::from_secs(6)).await;
                drop(input);
            }
        };
        let (report, ()) = tokio::join!(task, user);
        let report = report.unwrap();
        assert_eq!(
            report.exit_reason,
            if eof {
                OperatorExitReason::InputClosed
            } else {
                OperatorExitReason::Deadline
            }
        );
        assert!(!report.dispatch_recorded);
        assert!(report.browser_closed);
        assert!(inspect_file(&h.path).unwrap().is_empty());
        assert_eq!(*h.commands.lock().unwrap(), ["prepare", "close"]);
    }
}

#[tokio::test(start_paused = true)]
async fn partial_terminal_command_survives_automatic_observation_cancellation() {
    let mut h = harness(Behavior::Ready);
    let (mut input, reader) = tokio::io::duplex(4096);
    let output = Output::default();
    let settings = options(10);
    let task = run(
        &mut h.controller,
        &settings,
        BufReader::new(reader),
        output.clone(),
        pending(),
    );
    let user = async {
        event(&mut h.events, "commit").await;
        send(&mut input, "sta").await;
        event(&mut h.events, "observe").await;
        send(&mut input, "tus\n").await;
        event(&mut h.events, "observe").await;
        send(&mut input, "close\n").await;
    };
    let (report, ()) = tokio::join!(task, user);
    assert_eq!(report.unwrap().observations, 2);
    assert!(!output.text().contains("unknown_command"));
}

#[tokio::test(start_paused = true)]
async fn transport_loss_retains_browser_for_review_and_never_sends_another_command_until_close() {
    let mut h = harness(Behavior::LostCommit);
    let (mut input, reader) = tokio::io::duplex(4096);
    let output = Output::default();
    let settings = options(20);
    let task = run(
        &mut h.controller,
        &settings,
        BufReader::new(reader),
        output.clone(),
        pending(),
    );
    let user = async {
        event(&mut h.events, "commit").await;
        send(&mut input, "resume\nstatus\n").await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(
            *h.commands.lock().unwrap(),
            ["prepare", "refresh", "commit"]
        );
        send(&mut input, "close\n").await;
    };
    let (report, ()) = tokio::join!(task, user);
    let report = report.unwrap();
    assert_eq!(report.exit_reason, OperatorExitReason::CloseCommand);
    assert!(report.dispatch_recorded && report.browser_closed);
    assert!(!output.text().contains("private-session-secret"));
    assert_eq!(
        report.last_result.unwrap().observation_error,
        Some("native_transport_uncertain")
    );
}

#[tokio::test(start_paused = true)]
async fn terminal_result_remains_open_until_operator_closes_without_further_search_or_polling() {
    let mut h = harness(Behavior::Terminal);
    let (mut input, reader) = tokio::io::duplex(4096);
    let settings = options(20);
    let task = run(
        &mut h.controller,
        &settings,
        BufReader::new(reader),
        Output::default(),
        pending(),
    );
    let user = async {
        event(&mut h.events, "commit").await;
        send(&mut input, "resume\nstatus\n").await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(
            *h.commands.lock().unwrap(),
            ["prepare", "refresh", "commit"]
        );
        send(&mut input, "close\n").await;
    };
    let (report, ()) = tokio::join!(task, user);
    let report = report.unwrap();
    assert_eq!(
        report.current_attempt.unwrap().state,
        AttemptState::NoHoldConfirmed
    );
    assert_eq!(report.observations, 0);
    assert!(report.browser_closed);
}

#[tokio::test(start_paused = true)]
async fn oversized_input_is_not_echoed_and_buffered_close_prevents_preparation() {
    let mut h = harness(Behavior::Ready);
    let secret = "synthetic-private-marker".repeat(1000);
    let input = std::io::Cursor::new(format!("{secret}\nclose\n").into_bytes());
    let output = Output::default();
    let report = run(
        &mut h.controller,
        &options(5),
        input,
        output.clone(),
        pending(),
    )
    .await
    .unwrap();
    assert_eq!(report.exit_reason, OperatorExitReason::CloseCommand);
    assert_eq!(*h.commands.lock().unwrap(), ["close"]);
    assert!(output.text().contains("unknown_command"));
    assert!(!output.text().contains("synthetic-private-marker"));
    assert!(inspect_file(&h.path).unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_stalled_close_has_a_bounded_timeout_and_never_claims_success() {
    let mut h = harness(Behavior::HangClose);
    let (mut input, reader) = tokio::io::duplex(4096);
    let settings = options(30);
    let started = Instant::now();
    let task = run(
        &mut h.controller,
        &settings,
        BufReader::new(reader),
        Output::default(),
        pending(),
    );
    let user = async {
        event(&mut h.events, "commit").await;
        send(&mut input, "close\n").await;
    };
    let (report, ()) = tokio::join!(task, user);
    let report = report.unwrap();
    assert_eq!(report.exit_reason, OperatorExitReason::CloseCommand);
    assert!(!report.browser_closed);
    assert!(report.dispatch_recorded);
    assert_eq!(started.elapsed(), Duration::from_secs(15));
    assert_eq!(
        inspect_file(&h.path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
}

struct BrokenOutput;
impl AsyncWrite for BrokenOutput {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test(start_paused = true)]
async fn output_failure_prevents_starting_a_native_preparation() {
    let mut h = harness(Behavior::Ready);
    let (_input, reader) = tokio::io::duplex(4096);
    let report = run(
        &mut h.controller,
        &options(30),
        BufReader::new(reader),
        BrokenOutput,
        pending(),
    )
    .await
    .unwrap();
    assert_eq!(report.exit_reason, OperatorExitReason::OutputError);
    assert_eq!(report.error, Some("operator_output_failed"));
    assert_eq!(*h.commands.lock().unwrap(), ["close"]);
    assert!(report.browser_closed);
    assert!(inspect_file(&h.path).unwrap().is_empty());
}
