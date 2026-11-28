# Service Management

Antre can be installed as an OS service so it starts automatically on boot
and restarts after crashes.

## Install

```bash
antre service install
```

This creates a service definition and starts it immediately:

| Platform | Service file | Init system |
|----------|-------------|-------------|
| macOS | `~/Library/LaunchAgents/ai.antre.gateway.plist` | launchd (user agent) |
| Linux | `~/.config/systemd/user/antre.service` | systemd (user unit) |
| Linux containers without systemd | `~/.antre/antre-service-supervisor.sh` | portable user supervisor |

The launchd and systemd configurations:

- **Start on boot** (`RunAtLoad` / `WantedBy=default.target`)
- **Restart on failure** with a 10-second cooldown
- **Log to** `~/.antre/antre.log`

On Linux, the generated user service is compatible with rootless Podman. If you
hand-edit the unit, avoid `NoNewPrivileges=true` and `ProtectHome=true` when
using Podman as the sandbox backend because they can prevent Podman from
creating user namespaces and reading its per-user container storage.

On Linux, Antre first uses `systemd --user` when it is available. Some
development containers, including Coder/devbox environments, do not run systemd
or provide the SysV `service` command. In those environments, `antre service
install` falls back to a small user-owned supervisor script in `~/.antre/`.
The fallback starts Antre in the background, records pid files, supports
`status`, `stop`, `restart`, and restarts Antre after crashes.

The portable supervisor is tied to the current container session. If the
workspace container is recreated, configure your devbox/Coder startup hook to
run `antre service restart` after installing Antre.

### Options

You can pass `--bind`, `--port`, and `--log-level` to bake them into the
service definition:

```bash
antre service install --bind 0.0.0.0 --port 8080 --log-level debug
```

These flags are written into the service file. The service reads the rest of
its configuration from `~/.antre/antre.toml` as usual.

## Manage

```bash
antre service status     # Show running/stopped/not-installed and PID
antre service stop       # Stop the service
antre service restart    # Restart the service
antre service logs       # Print the log file path
```

To tail the logs:

```bash
tail -f $(antre service logs)
```

## Uninstall

```bash
antre service uninstall
```

This stops the service, removes the service file, and cleans up.

## CLI Reference

| Command | Description |
|---------|-------------|
| `antre service install` | Install and start the service |
| `antre service uninstall` | Stop and remove the service |
| `antre service status` | Show service status and PID |
| `antre service stop` | Stop the service |
| `antre service restart` | Restart the service |
| `antre service logs` | Print log file path |

## How It Differs from `antre node add`

`antre service install` manages the **gateway** — the main Antre server
that hosts the web UI, chat sessions, and API.

`antre node add` registers a **headless node** — a client process on a
remote machine that connects back to a gateway for command execution. See
[Multi-Node](nodes.md) for details.

| | `antre service` | `antre node` |
|---|---|---|
| What it runs | The gateway server | A node client |
| Needs `--host`/`--token` | No | Yes |
| Config source | `~/.antre/antre.toml` | `~/.antre/node.json` |
| launchd label | `ai.antre.gateway` | `ai.antre.node` |
| systemd unit | `antre.service` | `antre-node.service` |
