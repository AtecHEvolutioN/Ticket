use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
};

use anyhow::Result;
use tempfile::tempdir;
use ticket_watch::{cart_controller::*, cart_journal::*, cart_preferences::PreferencesDocument};

const NOW: i64 = 1_800_000_000_000;
const EVENT: &str = "https://www.ticketmaster.cz/event/korn-vstupenky/1472650657";
const START: &str = "2026-11-16T18:00:00+01:00";

#[derive(Clone)]
struct TestClock(Arc<AtomicI64>);
impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct FakeSession {
    path: PathBuf,
    replies: VecDeque<Result<SessionReply>>,
    commands: Arc<Mutex<Vec<&'static str>>>,
}
impl NativeCartSession for FakeSession {
    async fn exchange(&mut self, command: SessionCommand) -> Result<SessionReply> {
        let name = match command {
            SessionCommand::Prepare { .. } => "prepare",
            SessionCommand::Refresh => "refresh",
            SessionCommand::ResumePreparation => "resume_preparation",
            SessionCommand::Commit {
                attempt_id,
                preflight_id,
                dispatch_token,
                adjacent_required,
            } => {
                assert_eq!(attempt_id, "attempt-1");
                assert_eq!(dispatch_token, attempt_id);
                assert_eq!(preflight_id, "preflight-1");
                // A second connection can see the durable dispatch before the
                // fake stock-affecting browser boundary is ever crossed.
                let rows = inspect_file(&self.path).unwrap();
                assert_eq!(rows.len(), 1);
                assert!(rows[0].dispatch_at_ms.is_some());
                assert_eq!(rows[0].attempt_id, attempt_id);
                assert_eq!(
                    adjacent_required,
                    rows[0].search_binding().unwrap().request.adjacent_required
                );
                "commit"
            }
            SessionCommand::Observe { attempt_id } => {
                assert_eq!(attempt_id, "attempt-1");
                "observe"
            }
            SessionCommand::Close => "close",
        };
        self.commands.lock().unwrap().push(name);
        self.replies
            .pop_front()
            .expect("unexpected browser command")
    }
}

fn selection() -> SessionPrepareRequest {
    SessionPrepareRequest {
        event_url: EVENT.into(),
        expected_event_start: START.into(),
        category_id: "23".into(),
        price_id: "11".into(),
        section_ids: vec![],
        quantity: 2,
        configured_unit_price_minor: 171200,
    }
}
fn preflight() -> SessionPreflight {
    SessionPreflight {
        session_id: "session-1".into(),
        document_id: "document-1".into(),
        config_id: "config-1".into(),
        preflight_id: "preflight-1".into(),
        config_observed_at_ms: NOW,
        config_expires_at_ms: NOW + 5000,
        event_url: EVENT.into(),
        source_event_start: START.into(),
        category_id: "23".into(),
        category_title: "Omezený výhled".into(),
        quantity: 2,
        price_id: "11".into(),
        configured_unit_price_minor: 171200,
        currency: "CZK".into(),
        section_ids: vec![],
    }
}
fn prepared() -> SessionReply {
    SessionReply {
        state: SessionState::Prepared,
        preflight: Some(preflight()),
        outcome: None,
        manual_intervention: None,
        native_signal: None,
    }
}
fn context() -> NativeSearchReceiptContext {
    NativeSearchReceiptContext {
        attempt_id: "attempt-1".into(),
        event_url: Some(EVENT.into()),
        event_start: Some(START.into()),
        session_id: "session-1".into(),
        source_document_id: "document-1".into(),
        outcome_document_id: "document-1".into(),
        config_id: "config-1".into(),
        observed_at_ms: NOW,
        alternate_event_or_date_reported: false,
    }
}
fn uncertain(reason: NativeSearchUncertainty) -> SessionReply {
    SessionReply {
        state: SessionState::Uncertain,
        preflight: None,
        outcome: Some(NativeSearchOutcome::Uncertain(
            NativeSearchUncertainReceipt {
                context: context(),
                reason,
            },
        )),
        manual_intervention: None,
        native_signal: None,
    }
}
fn no_hold(observed_at_ms: i64) -> SessionReply {
    let mut context = context();
    context.observed_at_ms = observed_at_ms;
    SessionReply {
        state: SessionState::Uncertain,
        preflight: None,
        outcome: Some(NativeSearchOutcome::NoHold(NativeSearchNoHoldReceipt {
            context,
            receipt_correlation_id: "absence-1".into(),
            absence_explicitly_confirmed: true,
            request_finished_without_hold: true,
        })),
        manual_intervention: None,
        native_signal: None,
    }
}
type CommandLog = Arc<Mutex<Vec<&'static str>>>;
type TestController = NativeCartController<FakeSession, TestClock>;

fn controller(
    path: PathBuf,
    replies: Vec<Result<SessionReply>>,
) -> (TestController, CommandLog, TestClock) {
    let commands = Arc::new(Mutex::new(vec![]));
    let clock = TestClock(Arc::new(AtomicI64::new(NOW)));
    let session = FakeSession {
        path: path.clone(),
        replies: replies.into(),
        commands: commands.clone(),
    };
    (
        NativeCartController::with_clock(CartJournal::open(&path).unwrap(), session, clock.clone()),
        commands,
        clock,
    )
}

#[tokio::test]
async fn commits_durably_before_single_action_and_repeat_does_not_refresh() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let (mut control, commands, _) = controller(
        path.clone(),
        vec![
            Ok(prepared()),
            Ok(prepared()),
            Ok(uncertain(NativeSearchUncertainty::Pending)),
        ],
    );
    control.prepare(&selection()).await.unwrap();
    let first = control
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    assert!(first.dispatched_this_call);
    assert_eq!(first.attempt.state, AttemptState::NeedsReconciliation);
    assert!(first.attempt.native_outcome_hash.is_some());
    let repeated = control
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    assert!(!repeated.dispatched_this_call);
    assert_eq!(repeated.attempt, first.attempt);
    assert_eq!(*commands.lock().unwrap(), ["prepare", "refresh", "commit"]);
    assert!(
        CartJournal::open(&path)
            .unwrap()
            .begin_search(
                "another-attempt",
                &first.attempt.search_binding().unwrap().request,
                NOW
            )
            .is_err()
    );
}

#[tokio::test]
async fn commit_carries_effective_adjacency_preference_from_persisted_dispatch() {
    for (default_adjacent, override_adjacent) in [
        (true, None),
        (false, None),
        (true, Some(false)),
        (false, Some(true)),
    ] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        let (mut control, commands, _) = controller(
            path.clone(),
            vec![
                Ok(prepared()),
                Ok(prepared()),
                Ok(uncertain(NativeSearchUncertainty::Pending)),
            ],
        );
        let mut preferences = PreferencesDocument::default();
        preferences.defaults.require_adjacent = default_adjacent;
        if let Some(required) = override_adjacent {
            let mut event_preferences = preferences.defaults.clone();
            event_preferences.require_adjacent = required;
            preferences
                .event_overrides
                .push(ticket_watch::cart_preferences::EventOverride {
                    event_url: EVENT.into(),
                    preferences: event_preferences,
                });
        }
        control.prepare(&selection()).await.unwrap();
        let result = control
            .dispatch_once("attempt-1", &preferences, true)
            .await
            .unwrap();
        // FakeSession independently reads the committed journal and checks the
        // browser command's boolean before accepting its single synthetic action.
        assert_eq!(
            result
                .attempt
                .search_binding()
                .unwrap()
                .request
                .adjacent_required,
            override_adjacent.unwrap_or(default_adjacent)
        );
        assert_eq!(result.attempt.state, AttemptState::NeedsReconciliation);
        preferences.defaults.require_adjacent = !default_adjacent;
        let repeated = control
            .dispatch_once("attempt-1", &preferences, true)
            .await
            .unwrap();
        assert!(!repeated.dispatched_this_call);
        assert_eq!(repeated.attempt, result.attempt);
        assert_eq!(*commands.lock().unwrap(), ["prepare", "refresh", "commit"]);
    }
}

#[tokio::test]
async fn disabled_live_action_does_not_refresh_or_acquire_journal_lock() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let (mut control, commands, _) = controller(path.clone(), vec![Ok(prepared())]);
    control.prepare(&selection()).await.unwrap();
    assert!(
        control
            .dispatch_once("attempt-1", &PreferencesDocument::default(), false)
            .await
            .is_err()
    );
    assert_eq!(*commands.lock().unwrap(), ["prepare"]);
    assert!(inspect_file(&path).unwrap().is_empty());
}

#[tokio::test]
async fn lost_commit_reply_retains_lock_and_can_only_observe() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let (mut control, commands, _) = controller(
        path.clone(),
        vec![
            Ok(prepared()),
            Ok(prepared()),
            Err(anyhow::anyhow!("private transport detail")),
            Ok(uncertain(NativeSearchUncertainty::Challenge)),
        ],
    );
    control.prepare(&selection()).await.unwrap();
    let initial = control
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    assert_eq!(
        initial.observation_error,
        Some("native_transport_uncertain")
    );
    assert_eq!(initial.attempt.state, AttemptState::NeedsReconciliation);
    assert!(initial.attempt.native_outcome_hash.is_none());
    let later = control.observe("observation-1").await.unwrap();
    assert!(later.attempt.native_outcome_hash.is_some());
    assert!(!later.dispatched_this_call);
    control
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    assert_eq!(
        *commands.lock().unwrap(),
        ["prepare", "refresh", "commit", "observe"]
    );
    assert_eq!(
        inspect_file(&path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
}

#[tokio::test]
async fn fresh_same_session_terminal_observation_reconciles_without_submission() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let (mut control, commands, clock) = controller(
        path.clone(),
        vec![
            Ok(prepared()),
            Ok(prepared()),
            Ok(uncertain(NativeSearchUncertainty::Pending)),
            Ok(no_hold(NOW + 100)),
        ],
    );
    control.prepare(&selection()).await.unwrap();
    control
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    clock.0.store(NOW + 100, Ordering::SeqCst);
    let observed = control.observe("passive-1").await.unwrap();
    assert_eq!(observed.attempt.state, AttemptState::NoHoldConfirmed);
    assert_eq!(inspect_search_history(&path, "attempt-1").unwrap().len(), 2);
    assert_eq!(
        *commands.lock().unwrap(),
        ["prepare", "refresh", "commit", "observe"]
    );
}

#[tokio::test]
async fn invalid_or_rolled_back_clock_cannot_accept_terminal_absence_after_a_lost_reply() {
    for invalid_now in [
        NOW - 1,
        -1,
        ticket_watch::cart::MAX_EXACT_INTEGER as i64 + 1,
    ] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        let mut terminal = no_hold(NOW);
        terminal.state = SessionState::NeedsHuman;
        terminal.manual_intervention = Some("captcha".into());
        terminal.native_signal = Some(NativeSignal::InsufficientTicketsReported);
        let (mut control, commands, clock) = controller(
            path.clone(),
            vec![
                Ok(prepared()),
                Ok(prepared()),
                Err(anyhow::anyhow!("lost commit reply")),
                Ok(terminal),
                Ok(no_hold(NOW + 100)),
            ],
        );
        control.prepare(&selection()).await.unwrap();
        let initial = control
            .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
            .await
            .unwrap();
        assert!(initial.attempt.native_outcome_hash.is_none());
        clock.0.store(invalid_now, Ordering::SeqCst);
        let rejected = control.observe("clock-conflict").await.unwrap();
        assert_eq!(
            rejected.observation_error,
            Some("native_clock_needs_review")
        );
        assert_eq!(rejected.attempt, initial.attempt);
        assert_eq!(inspect_file(&path).unwrap(), vec![initial.attempt.clone()]);
        assert!(
            inspect_search_history(&path, "attempt-1")
                .unwrap()
                .is_empty()
        );
        assert!(!rejected.dispatched_this_call);
        assert_eq!(rejected.session_state, SessionState::Uncertain);
        assert!(rejected.manual_intervention.is_none());
        assert!(rejected.native_signal.is_none());
        assert!(
            CartJournal::open(&path)
                .unwrap()
                .begin_search(
                    "another-attempt",
                    &initial.attempt.search_binding().unwrap().request,
                    NOW
                )
                .is_err()
        );
        // Restoring the clock permits a new passive observation of the same
        // attempt; it does not resubmit the native request or reuse the rejected
        // terminal receipt.
        clock.0.store(NOW + 100, Ordering::SeqCst);
        let recovered = control.observe("fresh-after-clock-review").await.unwrap();
        assert!(recovered.observation_error.is_none());
        assert_eq!(recovered.attempt.state, AttemptState::NoHoldConfirmed);
        assert_eq!(inspect_search_history(&path, "attempt-1").unwrap().len(), 1);
        assert_eq!(
            *commands.lock().unwrap(),
            ["prepare", "refresh", "commit", "observe", "observe"]
        );
    }
}

#[tokio::test]
async fn preference_quantity_mismatch_fails_before_dispatch() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let (mut control, commands, _) = controller(path.clone(), vec![Ok(prepared()), Ok(prepared())]);
    control.prepare(&selection()).await.unwrap();
    let mut prefs = PreferencesDocument::default();
    prefs.defaults.quantity = 3;
    assert!(
        control
            .dispatch_once("attempt-1", &prefs, true)
            .await
            .is_err()
    );
    assert!(inspect_file(&path).unwrap().is_empty());
    assert_eq!(*commands.lock().unwrap(), ["prepare", "refresh"]);
}

#[tokio::test]
async fn stale_preflight_cannot_acquire_dispatch_marker() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let (mut control, commands, clock) =
        controller(path.clone(), vec![Ok(prepared()), Ok(prepared())]);
    control.prepare(&selection()).await.unwrap();
    clock.0.store(NOW + 5000, Ordering::SeqCst);
    assert!(
        control
            .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
            .await
            .is_err()
    );
    assert!(inspect_file(&path).unwrap().is_empty());
    assert_eq!(*commands.lock().unwrap(), ["prepare", "refresh"]);
}

#[tokio::test]
async fn manual_pause_cannot_replace_document_lineage() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let human = SessionReply {
        state: SessionState::NeedsHuman,
        preflight: None,
        outcome: None,
        manual_intervention: Some("captcha".into()),
        native_signal: None,
    };
    let mut switched = prepared();
    switched.preflight.as_mut().unwrap().document_id = "other-document".into();
    let (mut control, commands, _) =
        controller(path.clone(), vec![Ok(prepared()), Ok(human), Ok(switched)]);
    control.prepare(&selection()).await.unwrap();
    assert_eq!(
        control.refresh().await.unwrap().state,
        SessionState::NeedsHuman
    );
    assert!(
        control
            .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
            .await
            .is_err()
    );
    assert!(inspect_file(&path).unwrap().is_empty());
    assert_eq!(*commands.lock().unwrap(), ["prepare", "refresh", "refresh"]);
}

#[tokio::test]
async fn malformed_outcome_remains_locked_and_does_not_reveal_error_details() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut invalid = uncertain(NativeSearchUncertainty::UnrecognizedResponse);
    invalid.native_signal = Some(NativeSignal::InsufficientTicketsReported);
    if let Some(NativeSearchOutcome::Uncertain(receipt)) = invalid.outcome.as_mut() {
        receipt.context.outcome_document_id = "private-token?not-a-correlation-id".into();
    }
    let (mut control, _, _) = controller(
        path.clone(),
        vec![Ok(prepared()), Ok(prepared()), Ok(invalid)],
    );
    control.prepare(&selection()).await.unwrap();
    let response = control
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    assert_eq!(response.observation_error, Some("native_outcome_rejected"));
    assert!(response.native_signal.is_none());
    assert_eq!(
        inspect_file(&path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("private-token")
    );
}

#[tokio::test]
async fn restart_with_existing_dispatch_cannot_submit_again() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    {
        let (mut original, _, _) = controller(
            path.clone(),
            vec![
                Ok(prepared()),
                Ok(prepared()),
                Err(anyhow::anyhow!("lost connection")),
            ],
        );
        original.prepare(&selection()).await.unwrap();
        original
            .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
            .await
            .unwrap();
    }
    let (mut restarted, commands, _) =
        controller(path.clone(), vec![Ok(prepared()), Ok(prepared())]);
    restarted.prepare(&selection()).await.unwrap();
    let result = restarted
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    assert!(!result.dispatched_this_call);
    assert_eq!(result.attempt.state, AttemptState::NeedsReconciliation);
    assert_eq!(*commands.lock().unwrap(), ["prepare", "refresh"]);
}

#[tokio::test]
async fn conflicting_event_attempt_blocks_before_native_commit() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let (mut first, _, _) = controller(
        path.clone(),
        vec![
            Ok(prepared()),
            Ok(prepared()),
            Ok(uncertain(NativeSearchUncertainty::Pending)),
        ],
    );
    first.prepare(&selection()).await.unwrap();
    first
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    let (mut second, commands, _) = controller(path, vec![Ok(prepared()), Ok(prepared())]);
    second.prepare(&selection()).await.unwrap();
    assert!(
        second
            .dispatch_once("attempt-2", &PreferencesDocument::default(), true)
            .await
            .is_err()
    );
    assert_eq!(*commands.lock().unwrap(), ["prepare", "refresh"]);
}

struct CancelledCommitSession {
    commands: Arc<Mutex<Vec<&'static str>>>,
}
impl NativeCartSession for CancelledCommitSession {
    async fn exchange(&mut self, command: SessionCommand) -> Result<SessionReply> {
        match command {
            SessionCommand::Prepare { .. } => {
                self.commands.lock().unwrap().push("prepare");
                Ok(prepared())
            }
            SessionCommand::Refresh => {
                self.commands.lock().unwrap().push("refresh");
                Ok(prepared())
            }
            SessionCommand::Commit { .. } => {
                self.commands.lock().unwrap().push("commit");
                std::future::pending().await
            }
            _ => panic!("unexpected native command"),
        }
    }
}

#[tokio::test]
async fn cancelled_commit_future_still_prevents_repeat_submission() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let commands = Arc::new(Mutex::new(vec![]));
    let session = CancelledCommitSession {
        commands: commands.clone(),
    };
    let mut control = NativeCartController::with_clock(
        CartJournal::open(&path).unwrap(),
        session,
        TestClock(Arc::new(AtomicI64::new(NOW))),
    );
    control.prepare(&selection()).await.unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            control.dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        )
        .await
        .is_err()
    );
    let repeated = control
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    assert!(!repeated.dispatched_this_call);
    assert!(inspect_file(&path).unwrap()[0].dispatch_at_ms.is_some());
    assert_eq!(*commands.lock().unwrap(), ["prepare", "refresh", "commit"]);
}

fn browser_options(script: PathBuf, evidence_dir: PathBuf) -> NativeBrowserOptions {
    NativeBrowserOptions {
        node: "node".into(),
        script,
        executable_path: None,
        headed: false,
        evidence_dir,
        timeout: std::time::Duration::from_secs(2),
    }
}

#[tokio::test]
async fn transport_hides_invalid_schema_values_and_only_allows_close_after_poison() {
    let dir = tempdir().unwrap();
    let script = dir.path().join("invalid.cjs");
    std::fs::write(&script, r#"
        require('node:readline').createInterface({input:process.stdin}).on('line', line => {
            const command = JSON.parse(line);
            if (command.op === 'close') { process.exit(0); return; }
            process.stdout.write(JSON.stringify({id:command.id,ok:true,state:'private_checkout_token_123'})+'\n');
        });
    "#).unwrap();
    let mut session =
        NativeBrowserSession::spawn(&browser_options(script, dir.path().to_path_buf())).unwrap();
    let error = session.exchange(SessionCommand::Refresh).await.unwrap_err();
    assert!(!format!("{error:#}").contains("private_checkout_token"));
    assert!(error.to_string().contains("invalid native response fields"));
    let blocked = session
        .exchange(SessionCommand::Observe {
            attempt_id: "attempt-1".into(),
        })
        .await
        .unwrap_err();
    assert!(blocked.to_string().contains("channel is uncertain"));
    assert_eq!(
        session.exchange(SessionCommand::Close).await.unwrap().state,
        SessionState::Closed
    );
}

#[tokio::test]
async fn transport_sanitizes_manual_intervention_before_returning_it() {
    let dir = tempdir().unwrap();
    let script = dir.path().join("intervention.cjs");
    std::fs::write(&script, r#"
        require('node:readline').createInterface({input:process.stdin}).on('line', line => {
            const command = JSON.parse(line);
            if (command.op === 'close') {
                process.stdout.write(JSON.stringify({id:command.id,ok:true,state:'closed'})+'\n');
                process.exit(0); return;
            }
            process.stdout.write(JSON.stringify({id:command.id,ok:true,state:'needs_human',manual_intervention:'private_session_url'})+'\n');
        });
    "#).unwrap();
    let mut session =
        NativeBrowserSession::spawn(&browser_options(script, dir.path().to_path_buf())).unwrap();
    let reply = session.exchange(SessionCommand::Refresh).await.unwrap();
    assert_eq!(reply.manual_intervention.as_deref(), Some("unknown"));
    session.exchange(SessionCommand::Close).await.unwrap();
}

#[tokio::test]
async fn worker_rejections_preserve_only_allowlisted_error_codes() {
    for (worker_error, expected) in [
        ("native_form_changed", "native_form_changed"),
        ("private_checkout_token_123", "session_failed"),
    ] {
        let dir = tempdir().unwrap();
        let script = dir.path().join("rejection.cjs");
        std::fs::write(
            &script,
            format!(
                r#"
            require('node:readline').createInterface({{input:process.stdin}}).on('line', line => {{
                const command = JSON.parse(line);
                if (command.op === 'close') {{ process.exit(0); return; }}
                process.stdout.write(JSON.stringify({{id:command.id,ok:false,error:{}}})+'\n');
            }});
        "#,
                serde_json::to_string(worker_error).unwrap()
            ),
        )
        .unwrap();
        let mut session =
            NativeBrowserSession::spawn(&browser_options(script, dir.path().to_path_buf()))
                .unwrap();
        let error = session.exchange(SessionCommand::Refresh).await.unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            format!("native worker rejected command: {expected}")
        );
        session.exchange(SessionCommand::Close).await.unwrap();
    }
}

#[tokio::test]
async fn native_worker_resolves_modules_from_forwarded_node_path() {
    const CHILD_MARKER: &str = "TICKET_TEST_NATIVE_NODE_PATH_CHILD";
    if std::env::var_os(CHILD_MARKER).is_none() {
        // Isolate the environment in another test process instead of mutating
        // this process's environment while other asynchronous tests are running.
        let modules = tempdir().unwrap();
        let package = modules.path().join("ticket-native-resolution-probe");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("index.js"), "module.exports = 'captcha';\n").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_worker_resolves_modules_from_forwarded_node_path",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env("NODE_PATH", modules.path())
            .env("NODE_OPTIONS", "--require=forbidden-native-preload")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated module-resolution test failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        return;
    }
    let dir = tempdir().unwrap();
    let script = dir.path().join("module-resolution.cjs");
    std::fs::write(
        &script,
        r#"
        const intervention = require('ticket-native-resolution-probe');
        if (process.env.NODE_OPTIONS) process.exit(2);
        require('node:readline').createInterface({input:process.stdin}).on('line', line => {
            const command = JSON.parse(line);
            const reply = command.op === 'close'
                ? {id:command.id,ok:true,state:'closed'}
                : {id:command.id,ok:true,state:'needs_human',manual_intervention:intervention};
            process.stdout.write(JSON.stringify(reply)+'\n');
            if (command.op === 'close') process.exit(0);
        });
    "#,
    )
    .unwrap();
    let mut session =
        NativeBrowserSession::spawn(&browser_options(script, dir.path().to_path_buf())).unwrap();
    let reply = session.exchange(SessionCommand::Refresh).await.unwrap();
    assert_eq!(reply.manual_intervention.as_deref(), Some("captcha"));
    session.exchange(SessionCommand::Close).await.unwrap();
}

#[tokio::test]
async fn passive_uncertainty_without_event_date_keeps_observing_and_preserves_lock() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let observation = |at| {
        let mut reply = uncertain(NativeSearchUncertainty::Pending);
        let Some(NativeSearchOutcome::Uncertain(value)) = reply.outcome.as_mut() else {
            unreachable!()
        };
        value.context.event_url = None;
        value.context.event_start = None;
        value.context.observed_at_ms = at;
        reply.native_signal = Some(NativeSignal::InsufficientTicketsReported);
        reply
    };
    let (mut control, commands, clock) = controller(
        path.clone(),
        vec![
            Ok(prepared()),
            Ok(prepared()),
            Ok(observation(NOW)),
            Ok(observation(NOW + 100)),
            Ok(observation(NOW + 200)),
        ],
    );
    control.prepare(&selection()).await.unwrap();
    let initial = control
        .dispatch_once("attempt-1", &PreferencesDocument::default(), true)
        .await
        .unwrap();
    assert_eq!(
        initial.native_signal,
        Some(NativeSignal::InsufficientTicketsReported)
    );
    for index in 1..=2 {
        clock.0.store(NOW + index * 100, Ordering::SeqCst);
        let observed = control
            .observe(&format!("observation-{index}"))
            .await
            .unwrap();
        assert!(observed.observation_error.is_none());
        assert_eq!(
            observed.native_signal,
            Some(NativeSignal::InsufficientTicketsReported)
        );
        assert_eq!(observed.attempt.state, AttemptState::NeedsReconciliation);
        assert!(!observed.attempt.event_start_claims_match);
        assert!(!observed.dispatched_this_call);
    }
    assert_eq!(inspect_search_history(&path, "attempt-1").unwrap().len(), 3);
    assert!(
        CartJournal::open(&path)
            .unwrap()
            .begin_search(
                "another-attempt",
                &initial.attempt.search_binding().unwrap().request,
                NOW + 200
            )
            .is_err()
    );
    assert_eq!(
        *commands.lock().unwrap(),
        ["prepare", "refresh", "commit", "observe", "observe"]
    );
}
