#!/usr/bin/env python3
"""One-time Proxmox-to-VM SSH setup, then run Codex in the Debian VM."""
import ipaddress
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import stat
import subprocess
import sys

REPOSITORY = 'https://github.com/AtecHEvolutioN/Ticket.git'
USAGE = 'Usage: python3 connect-vm-codex.py [--plan | --apply] VMID PRIVATE_IPV4'
GUEST = r'''
import fcntl, json, os, re, stat, subprocess, sys
from pathlib import Path
try:
    if os.geteuid() != 0 or 'VERSION_ID="13"' not in Path('/etc/os-release').read_text() or 'ID=debian' not in Path('/etc/os-release').read_text():
        raise ValueError('Debian13 root required')
    if subprocess.run(['/usr/bin/systemctl', 'is-active', '--quiet', 'ssh.service']).returncode != 0:
        raise ValueError('existing SSH service must be active')
    settings = subprocess.run(['/usr/sbin/sshd', '-T'], capture_output=True, text=True, check=True).stdout
    settings = dict(line.split(' ', 1) for line in settings.splitlines() if ' ' in line)
    if settings.get('permitrootlogin') not in {'yes', 'prohibit-password', 'without-password'} or settings.get('pubkeyauthentication') != 'yes':
        raise ValueError('existing SSH policy does not permit this key login')
    port = int(settings['port'])
    if not 1 <= port <= 65535:
        raise ValueError('invalid SSH port')
    host_key = ' '.join(Path('/etc/ssh/ssh_host_ed25519_key.pub').read_text().split()[:2])
    if not re.fullmatch(r'ssh-ed25519 [A-Za-z0-9+/]{40,100}={0,2}', host_key):
        raise ValueError('invalid public host key')
    if sys.argv[1] == 'authorize':
        raw = sys.stdin.read(8193)
        if len(raw) > 8192:
            raise ValueError('input bound')
        data = json.loads(raw)
        if set(data) != {'public_key', 'expected_host_key', 'vmid'} or data['expected_host_key'] != host_key:
            raise ValueError('host identity changed')
        if not re.fullmatch(r'ssh-ed25519 [A-Za-z0-9+/]{40,100}={0,2}', data['public_key']) or not re.fullmatch(r'[1-9][0-9]{2,8}', data['vmid']):
            raise ValueError('invalid public key or target')
        directory = Path('/root/.ssh')
        directory.mkdir(mode=0o700, exist_ok=True)
        info = directory.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o700:
            raise ValueError('inspect root SSH directory permissions')
        descriptor = os.open(directory / 'authorized_keys', os.O_RDWR | os.O_CREAT | os.O_APPEND | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, 'a+') as handle:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            info = os.fstat(handle.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022 or info.st_nlink != 1:
                raise ValueError('inspect authorized_keys permissions')
            handle.seek(0)
            existing = handle.read(65537)
            if len(existing) > 65536:
                raise ValueError('inspect large authorized_keys')
            key = data['public_key'].split()[1]
            if not any(key in line.split() for line in existing.splitlines()):
                if existing and not existing.endswith('\n'):
                    handle.write('\n')
                handle.write('restrict,pty ' + data['public_key'] + ' ticket-watch-codex-vm' + data['vmid'] + '\n')
                handle.flush()
                os.fsync(handle.fileno())
    elif sys.argv[1] != 'inspect':
        raise ValueError('invalid operation')
    print(json.dumps({'host_key': host_key, 'ssh_port': port}))
except Exception:
    print(json.dumps({'setup_error': 'inspect_guest_ssh_before_retry'}))
    sys.exit(1)
'''


def target(vmid, host):
    if not re.fullmatch(r'[1-9][0-9]{2,8}', vmid):
        raise ValueError('Supply a numeric Proxmox VM ID.')
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        raise ValueError('Supply the VM private IPv4 address.') from None
    if address.version != 4 or not address.is_private or address.is_loopback or address.is_link_local or address.is_unspecified or address.is_multicast:
        raise ValueError('Supply the VM private IPv4 address.')
    return vmid, str(address)


def guest(vmid, operation, data=None, runner=subprocess.run):
    command = ['qm', 'guest', 'exec', vmid, '--timeout', '20', '--pass-stdin', '1', '--', '/usr/bin/python3', '-c', GUEST, operation]
    try:
        result = runner(command, input='' if data is None else json.dumps(data), capture_output=True, text=True, timeout=30)
        if result.returncode != 0:
            raise ValueError('guest command failed')
        wrapper = json.loads(result.stdout)
        if not wrapper.get('exited') or wrapper.get('exitcode') != 0 or wrapper.get('out-truncated'):
            raise ValueError('guest not confirmed')
        reply = json.loads(wrapper['out-data'])
        if set(reply) != {'host_key', 'ssh_port'} or type(reply['ssh_port']) is not int or not 1 <= reply['ssh_port'] <= 65535 or not re.fullmatch(r'ssh-ed25519 [A-Za-z0-9+/]{40,100}={0,2}', reply['host_key']):
            raise ValueError('invalid host identity')
        return reply
    except Exception:
        raise RuntimeError('VM SSH setup was not confirmed. Inspect the guest SSH service/policy before retrying; no automatic retry was made.') from None


def pinned_host(path, host, port, public_key, owner=0):
    label = host if port == 22 else f'[{host}]:{port}'
    wanted = f'{label} {public_key}\n'
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    except FileExistsError:
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != owner or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600 or info.st_size > 4096 or path.read_text() != wanted:
            raise ValueError('Existing pinned host identity differs or has unsafe permissions; inspect before continuing.')
    else:
        with os.fdopen(descriptor, 'w') as handle:
            handle.write(wanted)
            handle.flush()
            os.fsync(handle.fileno())


def main(args=None):
    args = sys.argv[1:] if args is None else args
    if len(args) == 2:
        mode, vmid, host = '--plan', *args
    elif len(args) == 3 and args[0] in {'--plan', '--apply'}:
        mode, vmid, host = args
    else:
        raise ValueError(USAGE)
    vmid, host = target(vmid, host)
    if mode == '--plan':
        print(f'Prepare one dedicated SSH key on Proxmox for VM {vmid} at {host}; preserve existing keys/passwords and pin the VM host identity from its guest agent.')
        print('Install official Codex inside that VM, create/reuse /root/ticket-codex, then open its terminal for private ChatGPT sign-in. No changes were made.')
        return
    if os.geteuid() != 0 or not sys.stdin.isatty():
        raise ValueError('Run --apply interactively in the Proxmox node Shell as its administrator.')
    os.environ['PATH'] = '/usr/sbin:/usr/bin:/sbin:/bin'
    if any(not shutil.which(tool) for tool in ['qm', 'ssh', 'ssh-keygen']):
        raise ValueError('The Proxmox node needs its existing qm, ssh and ssh-keygen tools.')
    identity = guest(vmid, 'inspect')
    folder = Path('/root/.ssh')
    folder.mkdir(mode=0o700, exist_ok=True)
    info = folder.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o700:
        raise ValueError('Inspect Proxmox root SSH directory ownership/permissions.')
    private = folder / f'ticket-watch-codex-vm{vmid}'
    public = Path(str(private) + '.pub')
    if not private.exists() and not private.is_symlink() and not public.exists() and not public.is_symlink():
        subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', f'ticket-watch-codex-vm{vmid}', '-f', str(private)], check=True)
    for path, private_file in [(private, True), (public, False)]:
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_nlink != 1 or info.st_mode & 0o022 or (private_file and stat.S_IMODE(info.st_mode) != 0o600):
            raise ValueError('Inspect the dedicated SSH key files; existing files were not replaced.')
    key = ' '.join(public.read_text().split()[:2])
    if not re.fullmatch(r'ssh-ed25519 [A-Za-z0-9+/]{40,100}={0,2}', key):
        raise ValueError('Dedicated public key is invalid.')
    known = folder / f'ticket-watch-codex-vm{vmid}-known-hosts'
    pinned_host(known, host, identity['ssh_port'], identity['host_key'])
    if guest(vmid, 'authorize', {'public_key': key, 'expected_host_key': identity['host_key'], 'vmid': vmid}) != identity:
        raise RuntimeError('VM host identity changed; inspect before retrying.')
    base = ['ssh', '-i', str(private), '-o', 'IdentitiesOnly=yes', '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes',
            '-o', f'UserKnownHostsFile={known}', '-o', 'ConnectTimeout=10', '-p', str(identity['ssh_port'])]
    destination = f'root@{host}'
    check = subprocess.run(base + [destination, '/usr/bin/id -u'], capture_output=True, text=True, timeout=20)
    if check.returncode != 0 or check.stdout.strip() != '0':
        raise RuntimeError('Pinned SSH login was not confirmed. The SSH policy was not weakened; inspect the connection.')
    install = f'''set -eu
if ! command -v git >/dev/null; then
    DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends git
fi
if [ ! -e /root/ticket-codex ]; then
    git clone {shlex.quote(REPOSITORY)} /root/ticket-codex
elif [ "$(git -C /root/ticket-codex remote get-url origin)" != {shlex.quote(REPOSITORY)} ]; then
    echo 'Existing Codex checkout needs inspection.' >&2; exit 1
fi
if [ ! -x /root/.local/bin/codex ]; then
    curl -fL --proto '=https' --proto-redir '=https' --max-time 60 https://chatgpt.com/codex/install.sh -o /root/ticket-watch-official-codex-install.sh
    CODEX_NON_INTERACTIVE=1 sh /root/ticket-watch-official-codex-install.sh
fi
/root/.local/bin/codex --version
'''
    subprocess.run(base + [destination, install], check=True)
    print('SSH READY. Codex runs inside the Debian VM. Complete ChatGPT device sign-in privately in this terminal, then the local session starts.', flush=True)
    prompt = 'Read HANDOFF.md and docs/remote-codex.md. You are running directly inside the project Debian VM. Continue the bot deployment there, preserving existing applications and private credentials. The website identification wait fix is in the checkout but has not been applied to the installed worker. Verify current state before changing anything.'
    launch = 'cd /root/ticket-codex && (/root/.local/bin/codex login status >/dev/null 2>&1 || /root/.local/bin/codex login --device-auth) && exec /root/.local/bin/codex ' + shlex.quote(prompt)
    os.execvp('ssh', base + ['-t', destination, launch])


if __name__ == '__main__':
    try:
        main()
    except (ValueError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, subprocess.SubprocessError):
        print('Setup stopped. Inspect the preceding installation/SSH result before retrying. No private key was printed.', file=sys.stderr)
        sys.exit(1)
