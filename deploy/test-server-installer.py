#!/usr/bin/env python3
"""Read-only installer checks. Never install packages or touch system paths."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().with_name('install-nl-server.sh')
NAME = 'ticket-watch-bot-linux-x86_64.tar.gz'


class ServerInstaller(unittest.TestCase):
    def run_script(self, *args, env=None):
        return subprocess.run(['/bin/bash', str(SCRIPT), *args], env=env, capture_output=True, text=True)

    def bundle(self, root, entries):
        with tarfile.open(root / NAME, 'w:gz') as archive:
            for name, mode, kind in entries:
                entry = tarfile.TarInfo(name)
                entry.mode = mode
                if kind == 'link':
                    entry.type = tarfile.SYMTYPE
                    entry.linkname = '/etc/shadow'
                    archive.addfile(entry)
                else:
                    data = b'public synthetic fixture'
                    entry.size = len(data)
                    archive.addfile(entry, io.BytesIO(data))
        digest = hashlib.sha256((root / NAME).read_bytes()).hexdigest()
        (root / (NAME + '.sha256')).write_text(f'{digest}  {NAME}\n')

    def test_plan_never_invokes_installers_or_network(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for command in ['apt-get', 'curl', 'mkdir', 'systemctl', 'npm', 'python3', 'uname']:
                stub = root / command
                stub.write_text('#!/bin/sh\necho unexpected-mutation >&2\nexit 88\n')
                stub.chmod(0o755)
            (root / 'cat').symlink_to('/bin/cat')
            env = {'PATH': str(root)}
            for args in [[], ['--plan']]:
                result = self.run_script(*args, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stderr, '')
                self.assertIn('No services are started', result.stdout)

    def test_valid_bundle_is_verified_without_extracting_files(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.bundle(root, [('ticket-watch-bot/README.md', 0o644, 'file')])
            before = set(root.iterdir())
            result = self.run_script('--verify-bundle', str(root))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(set(root.iterdir()), before)

    def test_hash_mismatch_and_malformed_manifest_fail(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.bundle(root, [('ticket-watch-bot/README.md', 0o644, 'file')])
            manifest = root / (NAME + '.sha256')
            original = manifest.read_text()
            for value in ['0' * 64 + f'  {NAME}\n', original + 'extra\n', original.replace(NAME, '../outside')]:
                manifest.write_text(value)
                self.assertNotEqual(self.run_script('--verify-bundle', str(root)).returncode, 0)

    def test_unsafe_archive_members_fail_before_extraction(self):
        for entries in [
            [('../escape', 0o644, 'file')],
            [('/absolute', 0o644, 'file')],
            [('other-root/file', 0o644, 'file')],
            [('ticket-watch-bot/link', 0o644, 'link')],
            [('ticket-watch-bot/file', 0o666, 'file')],
            [('ticket-watch-bot/file', 0o4755, 'file')],
            [('ticket-watch-bot/file', 0o644, 'file')] * 2,
        ]:
            with self.subTest(entries=entries), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                self.bundle(root, entries)
                self.assertNotEqual(self.run_script('--verify-bundle', str(root)).returncode, 0)
                self.assertEqual(len(list(root.iterdir())), 2)

    def test_bad_arguments_and_nonroot_install_stop_before_changes(self):
        for args in [['--unknown'], ['--install', 'do-not-echo-private-value'], ['--verify-bundle']]:
            result = self.run_script(*args)
            self.assertEqual(result.returncode, 2)
            self.assertNotIn('do-not-echo-private-value', result.stdout + result.stderr)
        if os.geteuid() != 0:
            result = self.run_script('--install')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('requires root', result.stderr)


if __name__ == '__main__':
    unittest.main(verbosity=2)
