#!/usr/bin/env python3
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('connect_vm_codex', Path(__file__).with_name('connect-vm-codex.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
KEY = 'ssh-ed25519 ' + 'A' * 68


class ConnectVmCodex(unittest.TestCase):
    def test_plan_does_not_contact_guest_or_start_programs(self):
        with patch.object(module, 'guest', side_effect=AssertionError('unexpected guest call')), patch.object(module.subprocess, 'run', side_effect=AssertionError('unexpected process')):
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                module.main(['--plan', '101', '192.168.77.55'])
            self.assertIn('No changes were made', output.getvalue())

    def test_target_rejects_unsafe_or_ambiguous_addresses(self):
        self.assertEqual(module.target('101', '192.168.77.55'), ('101', '192.168.77.55'))
        for vmid, host in [('01', '192.168.77.55'), ('101;true', '192.168.77.55'), ('101', '127.0.0.1'), ('101', '169.254.1.2'), ('101', '8.8.8.8'), ('101', 'server.invalid'), ('101', '192.168.77.55;true')]:
            with self.subTest(vmid=vmid, host=host), self.assertRaises(ValueError):
                module.target(vmid, host)

    def test_guest_protocol_binds_target_and_checks_completed_identity(self):
        calls = []
        def runner(command, **options):
            calls.append(command)
            self.assertEqual(command[:4], ['qm', 'guest', 'exec', '101'])
            return SimpleNamespace(returncode=0, stdout=json.dumps({'exited': 1, 'exitcode': 0, 'out-data': json.dumps({'host_key': KEY, 'ssh_port': 22})}))
        self.assertEqual(module.guest('101', 'inspect', runner=runner), {'host_key': KEY, 'ssh_port': 22})
        self.assertEqual(len(calls), 1)
        compile(module.GUEST, '<fixed-guest-setup>', 'exec')
        for response in [{'pid': 10}, {'exited': 1, 'exitcode': 1}, {'exited': 1, 'exitcode': 0, 'out-data': json.dumps({'host_key': KEY, 'ssh_port': True})}]:
            with self.assertRaises(RuntimeError):
                module.guest('101', 'inspect', runner=lambda *a, **k: SimpleNamespace(returncode=0, stdout=json.dumps(response)))

    def test_host_key_pin_is_private_and_cannot_be_silently_replaced(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'known-hosts'
            module.pinned_host(path, '192.168.77.55', 22, KEY, owner=os.getuid())
            before = path.read_bytes()
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            module.pinned_host(path, '192.168.77.55', 22, KEY, owner=os.getuid())
            with self.assertRaises(ValueError):
                module.pinned_host(path, '192.168.77.55', 22, 'ssh-ed25519 ' + 'B' * 68, owner=os.getuid())
            self.assertEqual(path.read_bytes(), before)
            link = Path(temp) / 'linked'
            link.symlink_to(path)
            with self.assertRaises(ValueError):
                module.pinned_host(link, '192.168.77.55', 22, KEY, owner=os.getuid())


if __name__ == '__main__':
    unittest.main(verbosity=2)
