# Install on an existing Proxmox Debian VM

Use an existing **Debian 13 x86_64 VM** with systemd, at least 2 GiB available
memory and 5 GiB free disk space for the initial installation. Check available
memory inside Linux; the Proxmox memory graph can include reclaimable cache.
The commands below use example VM ID `101`; replace it with your chosen VM ID.

The Proxmox node's **Shell** can run commands through the guest agent even when
the VM's console login is unavailable. These operations require the operator's
existing Proxmox privileges. They do not reset Linux passwords or modify SSH keys.

Inspect the VM before installing:

```sh
qm guest exec 101 -- cat /etc/os-release
qm guest exec 101 -- sh -c 'free -h; df -h /; docker ps --format "{{.Names}}"'
```

Download the reviewed installer **inside the VM**:

```sh
qm guest exec 101 -- curl -fL --proto '=https' --proto-redir '=https' --max-time 60 https://raw.githubusercontent.com/AtecHEvolutioN/Ticket/v0.8.0-server-test/deploy/install-nl-server.sh -o /root/ticket-watch-install.sh
```

Start it in the guest and capture its output in a private guest log. Proxmox
returns the process ID immediately; that means installation started, not that it
finished. Do not rerun an active installer.

```sh
qm guest exec 101 --synchronous 0 -- sh -c 'umask 077; exec bash /root/ticket-watch-install.sh --install > /root/ticket-watch-install.log 2>&1'
```

Inspect progress and the final result:

```sh
qm guest exec 101 -- tail -n 30 /root/ticket-watch-install.log
```

Successful preparation ends with `INSTALLATION PREPARED`. To check its process
exit code, use `qm guest exec-status 101 PID` with the returned process ID.
An error or missing success message needs inspection before a retry.

The installer downloads the `v0.8.0-server-test` bundle and its SHA256 checksum,
validates its archive structure and per-file hashes, installs Debian dependencies,
and prepares an unprivileged `ticketwatch-nl` service. It refuses existing Ticket
Watch paths and accounts. Existing Docker applications are not managed by it.

Shared-VM limits apply only to the new worker: one concurrent check, one CPU's
processing time, a 1 GiB memory high-water setting and a 1536 MiB hard limit.
These limits do not establish actual browser performance or provider access.

**Preparation leaves the watcher stopped, disabled and without credentials.**
Next, configure the provider/bot credentials privately, validate website access
on this VM, select the pilot destination and activate the reviewed configuration.
Never paste credentials into chat or a guest-exec command argument. Proxmox's
documented `--pass-stdin` option can pass private input without putting it in
command arguments; credential setup needs a separate reviewed step.

The authenticated chart API and HTTPS access are separate deployment steps;
see [the server guide](netherlands-server.md) and [live charts](live-history-api.md).
This installation does not activate Dutch carting.
