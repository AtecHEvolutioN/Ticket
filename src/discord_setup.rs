//! Offline Discord installation guidance and an explicitly requested, one-message test.
//! Neither command opens monitoring state or fetches ticket-provider information.
use crate::{
    config::{DiscordConfig, discord_snowflake},
    discord::{self, Delivery, Discord},
};
use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

const PERMISSIONS: &str = "19456"; // View Channel | Send Messages | Embed Links
const CHANNEL_MANAGEMENT_PERMISSIONS: &str = "19472"; // Base permissions | Manage Channels
const SENDER_NOTE: &str = "run, map-run and discord-test use independent rate limiters with the same bot token. Use one sender initially: stop monitors before the standalone test and wait any reported cooldown before restarting them. This test does not read or update their durable cooldowns.";
const NOTIFICATION_NOTE: &str = "Discord acceptance does not verify a device notification. Channel permissions, notification preferences and role mentionability still apply.";
static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

/// Build an invite and a safe configuration report without contacting Discord.
pub fn setup_report(
    config: &DiscordConfig,
    application_id: &str,
    guild_id: Option<&str>,
    manage_channels: bool,
) -> Result<Value> {
    config.validate()?;
    if !discord_snowflake(application_id) {
        bail!("Discord application_id must be a positive numeric ID fitting in 64 bits");
    }
    if guild_id.is_some_and(|id| !discord_snowflake(id)) {
        bail!("Discord guild_id must be a positive numeric ID fitting in 64 bits");
    }
    let permissions = if manage_channels {
        CHANNEL_MANAGEMENT_PERMISSIONS
    } else {
        PERMISSIONS
    };
    let mut permission_names = vec!["View Channel", "Send Messages", "Embed Links"];
    if manage_channels {
        permission_names.push("Manage Channels");
    }
    let mut invite = url::Url::parse("https://discord.com/oauth2/authorize")?;
    {
        let mut query = invite.query_pairs_mut();
        query
            .append_pair("client_id", application_id)
            .append_pair("scope", "bot applications.commands")
            .append_pair("permissions", permissions);
        if let Some(guild) = guild_id {
            query
                .append_pair("guild_id", guild)
                .append_pair("disable_guild_select", "true");
        }
    }
    Ok(json!({
        "schema_version": 1,
        "mode": "offline_setup",
        "application_id": application_id,
        "guild_id": guild_id,
        "invite_url": invite.as_str(),
        "scopes": ["bot", "applications.commands"],
        "permissions": {"value": permissions, "names": permission_names},
        "configuration": {
            "channel_id": config.channel_id,
            "token_env": config.token_env,
            "token_environment_set": std::env::var_os(&config.token_env).is_some(),
            "token_value": "[redacted]",
            "mention_user_ids": config.mention_user_ids,
            "mention_role_ids": config.mention_role_ids,
        },
        "notes": [
            "Offline report only: token presence is not credential validation. Authorize the invite and configure a normal text channel; channel permission overrides must also permit the bot.",
            "The invite does not register slash commands or verify the configured token belongs to this application.",
            SENDER_NOTE,
            NOTIFICATION_NOTE,
        ],
    }))
}

#[derive(Debug, Serialize)]
pub struct ConnectivityReport {
    schema_version: u32,
    mode: &'static str,
    status: &'static str,
    channel_id: Option<String>,
    payload: Value,
    message_id: Option<String>,
    cooldown_secs: Option<u64>,
    delivery_may_be_ambiguous: bool,
    automatic_retries: bool,
    diagnostic: Option<String>,
    next_step: &'static str,
    notes: [&'static str; 2],
}

impl ConnectivityReport {
    pub fn confirmed(&self) -> bool {
        self.status == "sent"
    }
}

/// Preview needs no credentials. Explicit sending performs at most one HTTP request.
/// A new invocation gets a new nonce; an ambiguous result must be checked manually.
pub async fn connectivity_test(config: &DiscordConfig, send: bool) -> Result<ConnectivityReport> {
    config.validate()?;
    let test_id = format!(
        "{}:{}:{}",
        std::process::id(),
        chrono::Utc::now().timestamp_micros(),
        NEXT_TEST.fetch_add(1, Ordering::Relaxed),
    );
    let report = preview_report(config, &test_id);
    if !send {
        return Ok(report);
    }
    let mut sender = Discord::new(config)?;
    send_once(&mut sender, report, &test_id).await
}

fn preview_report(config: &DiscordConfig, test_id: &str) -> ConnectivityReport {
    ConnectivityReport {
        schema_version: 1,
        mode: "preview",
        status: "not_sent",
        channel_id: config.channel_id.clone(),
        payload: discord::connectivity_preview(config, test_id),
        message_id: None,
        cooldown_secs: None,
        delivery_may_be_ambiguous: false,
        automatic_retries: false,
        diagnostic: None,
        next_step: "Configure the channel and bot token environment variable, then use discord-test --send to send one synthetic test with the configured mentions.",
        notes: [SENDER_NOTE, NOTIFICATION_NOTE],
    }
}

async fn send_once(
    sender: &mut Discord,
    mut report: ConnectivityReport,
    test_id: &str,
) -> Result<ConnectivityReport> {
    report.mode = "send_once";
    let outcome = sender.send_connectivity_test(test_id).await?;
    report.cooldown_secs = sender.cooldown_secs();
    match outcome {
        Delivery::Sent { message_id } => {
            report.status = "sent";
            report.message_id = Some(message_id);
            report.next_step = "Check the synthetic message and configured pings in Discord. Honor the reported cooldown before starting a sender.";
        }
        Delivery::Retry { after_secs, reason } => {
            report.cooldown_secs = Some(report.cooldown_secs.unwrap_or(0).max(after_secs));
            if matches!(
                reason.as_str(),
                "Discord rate limited the request" | "Discord rate-limit cooldown"
            ) {
                report.status = "rate_limited";
                report.diagnostic = Some(reason);
                report.next_step = "No retry was made. Wait at least cooldown_secs before explicitly trying again or starting a sender.";
            } else {
                report.status = "delivery_unconfirmed";
                report.delivery_may_be_ambiguous = true;
                report.diagnostic = Some(
                    if reason.starts_with("Discord acceptance could not be confirmed") {
                        "Discord acceptance could not be confirmed; the test may have been delivered".into()
                    } else {
                        reason
                    },
                );
                report.next_step = "No retry was made. Check the channel first and honor cooldown_secs. Another invocation has a new nonce and could duplicate an already delivered test.";
            }
        }
        Delivery::Permanent { reason } => {
            report.status = "rejected";
            report.diagnostic = Some(reason);
            report.next_step = "No retry was made. Correct the reported credential, channel or permission problem before sending another test; honor any reported cooldown.";
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    struct Fixture {
        sender: Discord,
        payloads: Arc<Mutex<Vec<Value>>>,
        task: JoinHandle<()>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn configuration() -> DiscordConfig {
        DiscordConfig {
            channel_id: Some("123456789012345678".into()),
            mention_user_ids: vec!["111111111111111111".into()],
            mention_role_ids: vec!["222222222222222222".into()],
            ..DiscordConfig::default()
        }
    }

    async fn fixture(response: Option<String>) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/messages", listener.local_addr().unwrap());
        let payloads = Arc::new(Mutex::new(Vec::new()));
        let received = payloads.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (start, length) = loop {
                    let mut chunk = [0_u8; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert!(n > 0 && bytes.len() < 65_536);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse::<usize>()
                            .unwrap();
                        assert!(length < 65_536);
                        break (index + 4, length);
                    }
                };
                while bytes.len() < start + length {
                    let mut chunk = [0_u8; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                }
                received
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice(&bytes[start..start + length]).unwrap());
                if let Some(response) = &response {
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
                // No response simulates a connection lost after the server read the message.
            }
        });
        Fixture {
            sender: Discord::for_test(&configuration(), endpoint),
            payloads,
            task,
        }
    }

    fn response(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn standalone_success_sends_only_synthetic_payload_and_reports_exhausted_bucket() {
        let mut fixture = fixture(Some(response(
            "201 Created",
            "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 120\r\n",
            r#"{"id":"333333333333333333"}"#,
        )))
        .await;
        let report = send_once(
            &mut fixture.sender,
            preview_report(&configuration(), "local-success"),
            "local-success",
        )
        .await
        .unwrap();
        assert!(report.confirmed());
        assert_eq!(report.message_id.as_deref(), Some("333333333333333333"));
        assert!(report.cooldown_secs.unwrap() >= 119);
        assert!(!report.automatic_retries && !report.delivery_may_be_ambiguous);
        let payloads = fixture.payloads.lock().unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0], report.payload);
        assert_eq!(report.payload["allowed_mentions"]["parse"], json!([]));
        assert_eq!(
            report.payload["allowed_mentions"]["users"],
            json!(["111111111111111111"])
        );
        assert_eq!(
            report.payload["allowed_mentions"]["roles"],
            json!(["222222222222222222"])
        );
        assert!(
            report.payload["embeds"][0]["title"]
                .as_str()
                .unwrap()
                .contains("synthetic connectivity test")
        );
        assert!(report.payload["embeds"][0].get("url").is_none());
        assert!(report.payload["embeds"][0].get("fields").is_none());
        assert!(!report.payload.to_string().contains("ticketmaster"));
    }

    #[tokio::test]
    async fn standalone_errors_never_retry_or_echo_server_bodies_and_keep_ambiguity_explicit() {
        let cases = [
            (
                Some(response(
                    "429 Too Many Requests",
                    "Retry-After: 4\r\n",
                    r#"{"retry_after":3,"message":"synthetic-private-marker"}"#,
                )),
                "rate_limited",
                false,
            ),
            (
                Some(response(
                    "401 Unauthorized",
                    "",
                    r#"{"message":"synthetic-private-marker"}"#,
                )),
                "rejected",
                false,
            ),
            (
                Some(response(
                    "403 Forbidden",
                    "",
                    r#"{"message":"synthetic-private-marker"}"#,
                )),
                "rejected",
                false,
            ),
            (
                Some(response(
                    "503 Unavailable",
                    "",
                    r#"{"message":"synthetic-private-marker"}"#,
                )),
                "delivery_unconfirmed",
                true,
            ),
            (
                Some(response(
                    "201 Created",
                    "",
                    r#"{"message":"synthetic-private-marker"}"#,
                )),
                "delivery_unconfirmed",
                true,
            ),
            (None, "delivery_unconfirmed", true),
        ];
        for (response, status, ambiguous) in cases {
            let mut fixture = fixture(response).await;
            let report = send_once(
                &mut fixture.sender,
                preview_report(&configuration(), "local-failure"),
                "local-failure",
            )
            .await
            .unwrap();
            assert_eq!(report.status, status);
            assert_eq!(report.delivery_may_be_ambiguous, ambiguous);
            assert!(!report.automatic_retries && !report.confirmed());
            assert_eq!(fixture.payloads.lock().unwrap().len(), 1);
            assert!(
                !serde_json::to_string(&report)
                    .unwrap()
                    .contains("synthetic-private-marker")
            );
            if ambiguous {
                assert!(report.next_step.contains("could duplicate"));
                assert!(report.cooldown_secs.unwrap() >= 2);
                assert!(!report.diagnostic.unwrap().contains("same nonce"));
            } else if status == "rate_limited" {
                assert_eq!(report.cooldown_secs, Some(4));
            }
        }
    }
}
