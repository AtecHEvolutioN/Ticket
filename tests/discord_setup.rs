use serde_json::Value;
use std::process::{Command, Output};

fn run(directory: &std::path::Path, args: &[&str], token: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ticket-watch"));
    command
        .current_dir(directory)
        .env_remove("TICKETMASTER_API_KEY")
        .env_remove("DISCORD_BOT_TOKEN")
        .env_remove("TICKET_TEST_DISCORD_TOKEN")
        .args(args);
    if let Some(token) = token {
        command.env("TICKET_TEST_DISCORD_TOKEN", token);
    }
    command.output().unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn setup_and_preview_work_without_configuration_credentials_or_created_state() {
    let dir = tempfile::tempdir().unwrap();
    let report = success(run(
        dir.path(),
        &["discord-setup", "--application-id", "123456789012345678"],
        None,
    ));
    let invite = url::Url::parse(report["invite_url"].as_str().unwrap()).unwrap();
    let query: std::collections::HashMap<_, _> = invite.query_pairs().into_owned().collect();
    assert_eq!(
        invite.as_str().split('?').next().unwrap(),
        "https://discord.com/oauth2/authorize"
    );
    assert_eq!(query["scope"], "bot applications.commands");
    assert_eq!(query["permissions"], "19456");
    assert!(!query.contains_key("guild_id"));
    assert_eq!(report["configuration"]["token_environment_set"], false);
    let preview = success(run(dir.path(), &["discord-test"], None));
    assert_eq!(preview["status"], "not_sent");
    assert_eq!(preview["mode"], "preview");
    assert_eq!(preview["automatic_retries"], false);
    assert!(preview["channel_id"].is_null());
    assert!(preview["message_id"].is_null());
    assert!(
        preview["payload"]["embeds"][0]["title"]
            .as_str()
            .unwrap()
            .contains("synthetic connectivity test")
    );
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}

#[test]
fn configured_setup_redacts_credentials_and_locks_the_requested_guild() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let text = r#"
database = "never-open/inventory.sqlite"
map_database = "never-open/maps.sqlite"
[discord]
channel_id = "123456789012345678"
token_env = "TICKET_TEST_DISCORD_TOKEN"
mention_user_ids = ["111111111111111111"]
mention_role_ids = ["222222222222222222"]
"#;
    std::fs::write(&config, text).unwrap();
    let output = run(
        dir.path(),
        &[
            "discord-setup",
            "--application-id",
            "123456789012345678",
            "--guild-id",
            "333333333333333333",
        ],
        Some("synthetic-secret-token-marker"),
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-secret-token-marker"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-secret-token-marker"));
    let setup = success(output);
    assert_eq!(setup["configuration"]["token_environment_set"], true);
    assert_eq!(setup["configuration"]["token_value"], "[redacted]");
    let invite = url::Url::parse(setup["invite_url"].as_str().unwrap()).unwrap();
    let query: std::collections::HashMap<_, _> = invite.query_pairs().into_owned().collect();
    assert_eq!(query["guild_id"], "333333333333333333");
    assert_eq!(query["disable_guild_select"], "true");
    let preview = success(run(dir.path(), &["discord-test"], None));
    assert_eq!(
        preview["payload"]["content"],
        "<@111111111111111111> <@&222222222222222222>"
    );
    assert_eq!(
        preview["payload"]["allowed_mentions"]["parse"],
        serde_json::json!([])
    );
    assert_eq!(std::fs::read_to_string(config).unwrap(), text);
    assert!(!dir.path().join("never-open").exists());
}

#[test]
fn setup_ids_and_explicit_send_validate_locally_without_creating_state_or_leaking_values() {
    let dir = tempfile::tempdir().unwrap();
    for invalid in [
        "0",
        "18446744073709551616",
        "synthetic-secret-marker",
        "123&scope=administrator",
    ] {
        for args in [
            vec!["discord-setup", "--application-id", invalid],
            vec![
                "discord-setup",
                "--application-id",
                "123",
                "--guild-id",
                invalid,
            ],
        ] {
            let output = run(dir.path(), &args, None);
            assert!(!output.status.success() && output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-secret-marker"));
        }
    }
    let missing_config = run(
        dir.path(),
        &["--config", "missing.toml", "discord-test"],
        None,
    );
    assert!(!missing_config.status.success());
    let no_channel = run(dir.path(), &["discord-test", "--send"], None);
    assert!(!no_channel.status.success() && no_channel.stdout.is_empty());
    assert!(String::from_utf8_lossy(&no_channel.stderr).contains("channel_id is required"));
    std::fs::write(
        dir.path().join("config.toml"),
        "[discord]\nchannel_id='123456789012345678'\ntoken_env='TICKET_TEST_DISCORD_TOKEN'\n",
    )
    .unwrap();
    for (token, expected) in [
        (None, "not set"),
        (Some(""), "empty"),
        (
            Some("synthetic-secret-marker\n"),
            "invalid header characters",
        ),
    ] {
        // Put the newline inside the value: surrounding whitespace is validly trimmed.
        let token = token.map(|value| {
            if value.ends_with('\n') {
                "synthetic-secret-marker\ninside"
            } else {
                value
            }
        });
        let output = run(dir.path(), &["discord-test", "--send"], token);
        assert!(!output.status.success() && output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("synthetic-secret-marker"));
    }
    assert!(!dir.path().join("data").exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn discord_configuration_rejects_zero_and_overflow_for_channels_and_mentions() {
    use ticket_watch::config::{Config, DiscordConfig};
    for invalid in ["0", "000", "18446744073709551616", "-1", "x", ""] {
        for kind in 0..3 {
            let mut config = DiscordConfig::default();
            match kind {
                0 => config.channel_id = Some(invalid.into()),
                1 => config.mention_user_ids.push(invalid.into()),
                _ => config.mention_role_ids.push(invalid.into()),
            }
            assert!(config.validate().is_err());
            assert!(
                ticket_watch::config::validate(&Config {
                    discord: config,
                    ..Config::default()
                })
                .is_err()
            );
        }
    }
    for valid in ["1", "123456789012345678", "18446744073709551615"] {
        let config = DiscordConfig {
            channel_id: Some(valid.into()),
            mention_user_ids: vec![valid.into()],
            mention_role_ids: vec![valid.into()],
            ..DiscordConfig::default()
        };
        assert!(config.validate().is_ok());
    }
}
