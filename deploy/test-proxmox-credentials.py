#!/usr/bin/env python3
import importlib.util
import json
import os
from pathlib import Path
import stat
import sys
from types import SimpleNamespace
import tempfile
import unittest

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location('proxmox_credentials', Path(__file__).with_name('proxmox-credentials.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
API = 'synthetic_api_key_only_1234567890'
TOKEN = 'synthetic.bot_token_only.1234567890'


class ProxmoxCredentials(unittest.TestCase):
    def initial(self, root):
        path = root / 'environment'
        path.write_text('# placeholders only\n')
        path.chmod(0o600)
        return path

    def test_atomic_private_store_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = self.initial(root)
            module.save_environment(path, API, TOKEN, owner=os.getuid(), group=os.getgid())
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            self.assertEqual(path.read_text(), f'TICKETMASTER_API_KEY={API}\nDISCORD_BOT_TOKEN={TOKEN}\n')
            before = path.read_bytes()
            with self.assertRaisesRegex(ValueError, 'Existing credentials'):
                module.save_environment(path, API, TOKEN, owner=os.getuid(), group=os.getgid())
            self.assertEqual(path.read_bytes(), before)
            self.assertEqual({p.name for p in root.iterdir()}, {'environment', '.credentials.lock'})

    def test_invalid_input_and_unsafe_paths_preserve_existing_files(self):
        for mode in ['newline', 'token_prefix', 'file_permissions', 'parent_permissions', 'symlink', 'hardlink']:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                path = self.initial(root)
                api, token = API, TOKEN
                if mode == 'newline': api += '\nOTHER=value'
                if mode == 'token_prefix': token = 'Bot ' + token
                if mode == 'file_permissions': path.chmod(0o644)
                if mode == 'parent_permissions': root.chmod(0o777)
                if mode == 'symlink':
                    path.rename(root / 'original')
                    path.symlink_to(root / 'original')
                if mode == 'hardlink': os.link(path, root / 'original')
                before = path.read_bytes()
                with self.assertRaises((ValueError, OSError)):
                    module.save_environment(path, api, token, owner=os.getuid(), group=os.getgid())
                self.assertEqual(path.read_bytes(), before)

    def test_guest_transport_uses_stdin_and_never_credentials_in_arguments(self):
        calls = []
        def runner(command, **kwargs):
            calls.append((command, kwargs))
            self.assertNotIn(API, '\n'.join(command))
            self.assertNotIn(TOKEN, '\n'.join(command))
            self.assertEqual(command[:4], ['qm', 'guest', 'exec', '101'])
            self.assertEqual(json.loads(kwargs['input']), {'api_key': API, 'bot_token': TOKEN})
            return SimpleNamespace(returncode=0, stdout=json.dumps({'exited': 1, 'exitcode': 0, 'out-data': json.dumps({'credentials_saved': True})}))
        module.guest_request('101', 'store', {'api_key': API, 'bot_token': TOKEN}, runner=runner)
        self.assertEqual(len(calls), 1)
        compile(module.guest_program(), '<fixed-guest-program>', 'exec')

    def test_failed_or_ambiguous_transport_is_redacted_and_not_retried(self):
        for mode in ['exception', 'failure', 'still_running', 'bad_json', 'truncated']:
            with self.subTest(mode=mode):
                calls = []
                def runner(command, **kwargs):
                    calls.append(command)
                    if mode == 'exception': raise RuntimeError(TOKEN)
                    if mode == 'failure': return SimpleNamespace(returncode=1, stdout=TOKEN)
                    if mode == 'bad_json': return SimpleNamespace(returncode=0, stdout=TOKEN)
                    result = {'pid': 100} if mode == 'still_running' else {'exited': 1, 'exitcode': 0, 'out-truncated': True, 'out-data': TOKEN}
                    return SimpleNamespace(returncode=0, stdout=json.dumps(result))
                with self.assertRaises(RuntimeError) as error:
                    module.guest_request('101', 'store', {'api_key': API, 'bot_token': TOKEN}, runner=runner)
                self.assertNotIn(TOKEN, str(error.exception))
                self.assertNotIn(API, str(error.exception))
                self.assertEqual(len(calls), 1)


if __name__ == '__main__':
    unittest.main(verbosity=2)
