#!/usr/bin/env python3
"""Offline deployment-contract checks; never touches /etc, users or providers."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parent.parent
DEPLOY = ROOT / "deploy"
SCRIPT = DEPLOY / "prepare-nl-server.sh"


class NetherlandsDeployment(unittest.TestCase):
    def test_plan_is_default_and_does_not_execute_host_mutators(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            for command in ["systemctl", "useradd", "groupadd", "mkdir", "install", "chmod", "chown", "ln", "curl", "wget", "node"]:
                stub = temp / command
                stub.write_text("#!/bin/sh\necho unexpected-execution >&2\nexit 88\n")
                stub.chmod(0o755)
            (temp / "cat").symlink_to("/bin/cat")
            environment = {"PATH": str(temp), "HOME": str(temp)}
            default = subprocess.run(["/bin/bash", str(SCRIPT)], cwd=temp, env=environment, capture_output=True, text=True)
            explicit = subprocess.run(["/bin/bash", str(SCRIPT), "--plan"], cwd=temp, env=environment, capture_output=True, text=True)
            self.assertEqual(default.returncode, 0, default.stderr)
            self.assertEqual(default.stdout, explicit.stdout)
            self.assertEqual(default.stderr, "")
            self.assertIn("subscription starts disabled", default.stdout)
            self.assertIn("no enable/start/restart", default.stdout)

    def test_arguments_reject_secrets_or_extra_values_before_host_checks(self):
        for args in [["--token=synthetic-must-not-echo"], ["--apply", "synthetic-must-not-echo"]]:
            result = subprocess.run(["/bin/bash", str(SCRIPT), *args], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertNotIn("synthetic-must-not-echo", result.stdout + result.stderr)

    def test_prepared_scope_is_netherlands_only_and_inactive(self):
        config = tomllib.loads((DEPLOY / "nl-service.example.toml").read_text())
        self.assertEqual(config["database"], "/var/lib/ticket-watch-nl/ticket.sqlite")
        self.assertEqual(config["catalog"]["max_events"], 1000)
        self.assertEqual(config["per_host_interval_secs"], 60)
        self.assertEqual(len(config["subscriptions"]), 1)
        subscription = config["subscriptions"][0]
        self.assertTrue(subscription["all_artists"])
        self.assertEqual(subscription["country"], "NL")
        self.assertFalse(subscription["enabled"])
        self.assertNotIn("attraction_id", subscription)
        self.assertFalse(config.get("map_watches"))
        self.assertFalse(config.get("watches"))
        self.assertNotIn("channel_id", config["discord"])
        self.assertEqual(config["browser"]["node"], "/usr/bin/node")
        self.assertEqual(config["browser"]["executable_path"], "/usr/bin/chromium")

    def test_unit_runs_unprivileged_preview_with_separate_private_storage(self):
        text = (DEPLOY / "ticket-watch-nl.service").read_text()
        lines = [line.strip() for line in text.splitlines() if not line.startswith("#")]
        self.assertIn("User=ticketwatch-nl", lines)
        self.assertIn("StateDirectoryMode=0700", lines)
        self.assertIn("CacheDirectoryMode=0700", lines)
        self.assertIn("ProtectSystem=strict", lines)
        self.assertIn("NoNewPrivileges=true", lines)
        self.assertIn("Environment=HOME=/var/lib/ticket-watch-nl", lines)
        self.assertIn("Environment=XDG_CACHE_HOME=/var/cache/ticket-watch-nl", lines)
        self.assertIn("Restart=on-failure", lines)
        self.assertIn("KillSignal=SIGINT", lines)
        self.assertIn("WantedBy=multi-user.target", lines)
        start = next(line for line in lines if line.startswith("ExecStart="))
        self.assertTrue(start.endswith(" run"))
        self.assertNotIn("--send-discord", start)
        self.assertNotIn("map-run", start)
        dropin = (DEPLOY / "ticket-watch-nl.send-discord.conf.example").read_text()
        self.assertIn("ExecStart=\n", dropin)
        self.assertIn(" run --send-discord\n", dropin)

    def test_shell_syntax(self):
        subprocess.run(["/bin/bash", "-n", str(SCRIPT)], check=True)
        text = SCRIPT.read_text()
        self.assertIn('offline /usr/bin/node -e \'require("node:fs").readFileSync("/opt/ticket-watch/browser/worker.cjs")', text)
        self.assertLess(text.index('readFileSync("/opt/ticket-watch/browser/worker.cjs")'), text.index('require.resolve("playwright"'))

    def test_systemd_unit_syntax_with_temporary_executable_substitution(self):
        verifier = shutil.which("systemd-analyze")
        if not verifier:
            self.skipTest("systemd-analyze unavailable")
        with tempfile.TemporaryDirectory() as temp:
            target = Path(temp) / "ticket-watch-nl.service"
            # Verify syntax without requiring an installed /opt checkout; this
            # does not prove browser startup or runtime service compatibility.
            text = (DEPLOY / "ticket-watch-nl.service").read_text().replace(
                "/opt/ticket-watch/target/release/ticket-watch", "/bin/true"
            )
            target.write_text(text)
            result = subprocess.run([verifier, "verify", str(target)], capture_output=True, text=True)
            if "Failed to create directory '/run/systemd/': Read-only file system" in result.stderr:
                self.skipTest("sandbox prevents systemd-analyze runtime initialization; syntax requires target-host verification")
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_current_binary_validates_template_without_creating_runtime_state(self):
        binary = ROOT / "target/release/ticket-watch"
        if not binary.is_file():
            self.skipTest("release binary unavailable")
        with tempfile.TemporaryDirectory() as temp:
            environment = {"PATH": "/usr/bin:/bin", "HOME": temp}
            version = subprocess.run([str(binary), "--version"], env=environment, capture_output=True, text=True)
            self.assertEqual(version.returncode, 0, version.stderr)
            self.assertTrue(version.stdout.startswith("ticket-watch "))
            result = subprocess.run([str(binary), "--config", str(DEPLOY / "nl-service.example.toml"), "validate"], cwd=temp, env=environment, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(list(Path(temp).iterdir()), [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
