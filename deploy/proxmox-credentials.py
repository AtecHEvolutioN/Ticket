#!/usr/bin/env python3
"""Prompt privately on the Proxmox node and pass credentials to its guest stdin."""
import getpass
import inspect
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import warnings


def validate_credentials(api_key, bot_token):
    import re
    if not isinstance(api_key, str) or not re.fullmatch(r'[A-Za-z0-9_-]{16,128}', api_key):
        raise ValueError('Invalid Ticketmaster key format; enter only the key value.')
    if not isinstance(bot_token, str) or not re.fullmatch(r'[A-Za-z0-9_.-]{30,512}', bot_token):
        raise ValueError('Invalid Discord token format; enter only the token value.')


def save_environment(path, api_key, bot_token, owner=0, group=0):
    import fcntl
    import os
    from pathlib import Path
    import stat
    import tempfile
    validate_credentials(api_key, bot_token)
    path = Path(path)
    parent = path.parent.lstat()
    if not stat.S_ISDIR(parent.st_mode) or parent.st_uid != owner or parent.st_mode & 0o022:
        raise ValueError('Private configuration directory requires inspection.')
    lock = os.open(path.parent / '.credentials.lock', os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(lock)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != owner or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600:
            raise ValueError('Private credential lock requires inspection.')
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        source = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(source, 'r') as handle:
            info = os.fstat(handle.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != owner or info.st_gid != group or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600:
                raise ValueError('Private environment file requires inspection.')
            existing = handle.read(16385)
        if len(existing) > 16384 or any(line.strip() and not line.lstrip().startswith('#') for line in existing.splitlines()):
            raise ValueError('Existing credentials/configuration retained; inspect before replacing.')
        descriptor, temporary = tempfile.mkstemp(prefix='.environment.', dir=path.parent)
        try:
            with os.fdopen(descriptor, 'w') as handle:
                os.fchmod(handle.fileno(), 0o600)
                os.fchown(handle.fileno(), owner, group)
                handle.write(f'TICKETMASTER_API_KEY={api_key}\nDISCORD_BOT_TOKEN={bot_token}\n')
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, path)
            directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    finally:
        os.close(lock)


def guest_program():
    return inspect.getsource(validate_credentials) + '\n' + inspect.getsource(save_environment) + '''
import json, os, stat, subprocess, sys
from pathlib import Path
try:
    if os.geteuid() != 0:
        raise ValueError('root required')
    target = Path('/etc/ticket-watch-nl/environment')
    parent = target.parent.lstat()
    info = target.lstat()
    if (not stat.S_ISDIR(parent.st_mode) or parent.st_uid != 0 or parent.st_mode & 0o022
        or not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_gid != 0
        or stat.S_IMODE(info.st_mode) != 0o600 or info.st_nlink != 1):
        raise ValueError('private paths require inspection')
    service = subprocess.run(['/usr/bin/systemctl', 'show', 'ticket-watch-nl.service',
                              '--property=LoadState', '--property=ActiveState'],
                             capture_output=True, text=True, timeout=5)
    states = dict(line.split('=', 1) for line in service.stdout.splitlines() if '=' in line)
    if service.returncode != 0 or states.get('LoadState') != 'loaded' or states.get('ActiveState') not in {'inactive', 'failed'}:
        raise ValueError('watcher must be installed and stopped')
    with target.open() as handle:
        existing = handle.read(16385)
    if len(existing) > 16384 or any(line.strip() and not line.lstrip().startswith('#') for line in existing.splitlines()):
        raise ValueError('existing credentials retained')
    if sys.argv[1] == 'preflight':
        print(json.dumps({'ready': True}))
    elif sys.argv[1] == 'store':
        raw = sys.stdin.read(4097)
        if len(raw) > 4096:
            raise ValueError('input bound')
        data = json.loads(raw)
        if set(data) != {'api_key', 'bot_token'}:
            raise ValueError('unexpected fields')
        save_environment(target, data['api_key'], data['bot_token'])
        print(json.dumps({'credentials_saved': True}))
    else:
        raise ValueError('invalid operation')
except Exception:
    print(json.dumps({'setup_error': 'inspect_before_retry'}))
    sys.exit(1)
'''


def guest_request(vmid, mode, credentials=None, runner=subprocess.run):
    command = ['qm', 'guest', 'exec', vmid, '--timeout', '20', '--pass-stdin', '1',
               '--', '/usr/bin/python3', '-c', guest_program(), mode]
    payload = '' if credentials is None else json.dumps(credentials)
    try:
        response = runner(command, input=payload, text=True, capture_output=True, timeout=30)
        if response.returncode != 0:
            raise ValueError('guest command failed')
        result = json.loads(response.stdout)
        if not result.get('exited') or result.get('exitcode') != 0 or result.get('out-truncated'):
            raise ValueError('guest result not confirmed')
        reply = json.loads(result['out-data'])
        expected = {'ready': True} if mode == 'preflight' else {'credentials_saved': True}
        if reply != expected:
            raise ValueError('unexpected guest response')
    except Exception:
        message = 'VM preflight was not confirmed; credentials were not requested.' if mode == 'preflight' else 'Credential storage was not confirmed. Inspect private file presence/permissions before retrying.'
        raise RuntimeError(message) from None


def main(args=None):
    args = sys.argv[1:] if args is None else args
    if args == ['--help']:
        print('Usage: python3 proxmox-credentials.py VMID. Prompts privately; never paste keys into chat.')
        return
    if len(args) != 1 or not re.fullmatch(r'[1-9][0-9]{2,8}', args[0]):
        raise ValueError('Supply one Proxmox VM ID, for example 101.')
    if os.geteuid() != 0 or not shutil.which('qm'):
        raise ValueError('Run on the Proxmox node as its administrator.')
    if not sys.stdin.isatty():
        raise ValueError('Run the downloaded file in an interactive terminal; private prompts require a terminal.')
    guest_request(args[0], 'preflight')
    print('Enter the two values here. Typing and pasted values stay hidden.')
    with warnings.catch_warnings():
        warnings.simplefilter('error', getpass.GetPassWarning)
        api_key = getpass.getpass('Ticketmaster Discovery API key: ')
        bot_token = getpass.getpass('Discord bot token: ')
    validate_credentials(api_key, bot_token)
    try:
        guest_request(args[0], 'store', {'api_key': api_key, 'bot_token': bot_token})
    finally:
        api_key = bot_token = None
    print('CREDENTIALS SAVED in the VM with root-only permissions. The watcher remains stopped.')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (EOFError, KeyboardInterrupt):
        print('\nCancelled. Inspect private credential state before retrying.', file=sys.stderr)
        sys.exit(1)
    except getpass.GetPassWarning:
        print('Private terminal input is unavailable. No fallback to visible input was used.', file=sys.stderr)
        sys.exit(1)
