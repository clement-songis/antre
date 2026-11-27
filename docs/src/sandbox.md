# Sandbox Backends

Moltis runs LLM-generated commands inside a sandbox to protect your host
system. The sandbox backend controls which isolation technology is used:
containers or VMs when a runtime is installed, otherwise the operating
system's own sandbox (Seatbelt on macOS, bubblewrap on Linux).

## Backend Selection

Configure in `moltis.toml`:

```toml
[tools.exec.sandbox]
backend = "auto"          # default — picks the best available
# backend = "podman"      # force Podman (daemonless, rootless)
# backend = "docker"      # force Docker
# backend = "apple-container"  # force Apple Container (macOS only)
# backend = "os"          # native OS sandbox (Seatbelt on macOS, bubblewrap on Linux)
# backend = "seatbelt"    # force Seatbelt (macOS only)
# backend = "bubblewrap"  # force bubblewrap (Linux only)
# backend = "wasm"        # force WASM sandbox (Wasmtime + WASI)
# backend = "restricted-host"  # env clearing + rlimits only
```

With `"auto"` (the default), Moltis picks the strongest available backend:

| Priority | Backend           | Platform | Isolation          |
|----------|-------------------|----------|--------------------|
| 1        | Apple Container   | macOS    | VM (Virtualization.framework) |
| 2        | Podman            | any      | Linux namespaces / cgroups (daemonless) |
| 3        | Docker            | any      | Linux namespaces / cgroups    |
| 4        | Vercel / Daytona  | remote   | only when credentials are configured |
| 5        | OS sandbox        | macOS, Linux | Seatbelt profile / bubblewrap namespaces (see below) |
| 6        | Restricted Host   | any      | env clearing, rlimits (no filesystem isolation) |
| 7        | none (host)       | any      | no isolation                  |

The active backend and what it protects are logged at startup, for example:

```text
INFO sandbox backend: OS sandbox backend="seatbelt" protection="macOS Seatbelt: system read-only, writes limited to the sandbox home, temp and writable workspace, user home and Moltis secrets hidden, network only through the egress proxy on 127.0.0.1:18791"
```

When Moltis falls back to the restricted host it logs a warning, because
commands then run unconfined and need approval.

The WASM backend (`backend = "wasm"`) is not in the auto-detect chain because
it cannot execute arbitrary shell commands — use it explicitly when you want
WASI-isolated execution.

## Apple Container (recommended on macOS)

[Apple Container](https://github.com/apple/container) runs each sandbox in a
lightweight virtual machine using Apple's Virtualization.framework. Every
container gets its own kernel, so a kernel exploit inside the sandbox cannot
reach the host — unlike Docker, which shares the host kernel.

### Install

Download the signed installer from GitHub:

```bash
# Download the installer package
gh release download --repo apple/container --pattern "container-installer-signed.pkg" --dir /tmp

# Install (requires admin)
sudo installer -pkg /tmp/container-installer-signed.pkg -target /

# First-time setup — downloads a default Linux kernel
container system start
```

Alternatively, build from source with `brew install container` (requires
Xcode 26+).

### Verify

```bash
container --version
# Run a quick test
container run --rm ubuntu echo "hello from VM"
```

Once installed, restart `moltis gateway` — the startup banner will show
`sandbox: apple-container backend`.

## Podman

[Podman](https://podman.io/) is a daemonless, rootless container engine that
is CLI-compatible with Docker. It is preferred over Docker in auto-detection
because it doesn't require a background daemon process and runs rootless by
default for better security.

### Install

```bash
# macOS
brew install podman
podman machine init && podman machine start

# Debian/Ubuntu
sudo apt-get install -y podman

# Fedora/RHEL
sudo dnf install -y podman
```

### Verify

```bash
podman --version
podman run --rm docker.io/library/ubuntu echo "hello from podman"
```

Once installed, restart `moltis gateway` — the startup banner will show
`sandbox: podman backend`. All Docker hardening flags (see below) apply
identically to Podman containers.

If Moltis runs under systemd, rootless Podman must be allowed to create user
namespaces and access the Moltis user's container storage. For the bundled
system unit, install `deploy/moltis-podman.conf` as
`/etc/systemd/system/moltis.service.d/podman.conf`. It disables
`NoNewPrivileges` and places Podman's home and runtime state outside the paths
hidden by `ProtectHome`. Without those accommodations, Podman can fail with
`cannot clone: Operation not permitted` and `Error: cannot re-exec process` even
when the same `podman run ...` command works from an interactive shell.

Moltis supports using Podman as the sandbox backend. Running `podman run ...`
from inside an already sandboxed Moltis container is different and is blocked by
the default hardened sandbox, which drops capabilities and sets
`no-new-privileges` inside the container.

Two mutually exclusive escape hatches are available when you intentionally want
container commands inside a Podman sandbox. Both require `backend = "podman"`;
host socket passthrough is supported only on Linux:

```toml
[tools.exec.sandbox]
backend = "podman"

# Use the host Podman service from inside the sandbox.
# This mounts the host Podman socket and sets CONTAINER_HOST/DOCKER_HOST.
allow_host_podman = true
# The sandbox image must include a Podman client. This replaces default packages.
packages = ["ca-certificates", "curl", "podman"]

# Or run Podman nested inside the sandbox.
# This starts the sandbox with privileged Podman launch flags and disables
# the normal no-new-privileges/cap-drop/read-only hardening for that sandbox.
# allow_nested_podman = true
# packages = ["ca-certificates", "curl", "podman", "uidmap", "slirp4netns", "fuse-overlayfs"]
```

Before using `allow_host_podman`, start the Podman API socket. For a CLI-installed
user service, run `systemctl --user enable --now podman.socket`. For the bundled
system service, install `deploy/moltis-podman-api.service` as documented in
[VPS Deployment](deploy-vps.md). Moltis verifies that the configured path is a
live Unix socket and fails sandbox startup if it is unavailable. The mount also
disables SELinux label separation so the container can access the socket.

`allow_host_podman` removes the sandbox boundary: sandboxed commands can use the
API to create containers with host bind mounts and execute code with the Podman
service user's file access. `allow_nested_podman` keeps container state inside
the sandbox, but it is runtime-dependent and weakens the sandbox substantially
by using privileged container settings.

## Docker

Docker is supported on macOS, Linux, and Windows. On macOS it runs inside a
Linux VM managed by Docker Desktop, so it is reasonably isolated but adds more
overhead than Apple Container.

Install from https://docs.docker.com/get-docker/

### Docker/Podman Hardening

Docker and Podman containers launched by Moltis include the following security
hardening flags by default:

| Flag | Effect |
|------|--------|
| `--cap-drop ALL` | Drops all Linux capabilities |
| `--security-opt no-new-privileges` | Prevents privilege escalation via setuid/setgid binaries |
| `--tmpfs /tmp:rw,nosuid,size=256m` | Writable tmpfs for temp files (noexec on real root) |
| `--tmpfs /run:rw,nosuid,size=64m` | Writable tmpfs for runtime files |
| `--read-only` | Read-only root filesystem (prebuilt images only) |
| `--hostname sandbox` | Prevents host hostname leakage |
| `--tmpfs /sys/firmware:ro,nosuid` | Masks BIOS/UEFI firmware data (Docker only) |
| `--tmpfs /sys/class/dmi:ro,nosuid` | Masks system serial numbers and identifiers (Docker only) |
| `--tmpfs /sys/devices/virtual/dmi:ro,nosuid` | Masks DMI attributes (Docker only) |
| `--tmpfs /sys/class/block:ro,nosuid` | Masks block device info (Docker only) |

The `--read-only` flag is applied only to prebuilt sandbox images (where
packages are already baked in). Non-prebuilt images need a writable root
filesystem for `apt-get` provisioning on first start.

When `allow_nested_podman = true` and the backend is Podman, Moltis starts the
sandbox with `--privileged` and does not apply `--cap-drop ALL`,
`--security-opt no-new-privileges`, or `--read-only`. This is required for
nested container runtimes but materially reduces the security boundary.

The `/sys` tmpfs overlays prevent host hardware metadata (serial numbers, disk
models, LUKS UUIDs) from being visible inside the container. Note that
`tools.fs.deny_paths` only restricts Moltis file-access tools — these kernel
filesystem masks prevent leakage via shell commands as well.

> **Podman note:** The sysfs tmpfs overlays are applied on Docker only. Podman's
> OCI runtime performs "tmpcopyup" when mounting tmpfs over sysfs paths, which
> fails under `--cap-drop ALL` because some sysfs files are permission-denied
> even for root. Podman masks `/sys/firmware` via its built-in OCI
> `MaskedPaths`; `/sys/class/dmi`, `/sys/devices/virtual/dmi`, and
> `/sys/class/block` remain readable inside the container on Podman.

## WASM Sandbox (Wasmtime + WASI)

The WASM sandbox provides real sandboxed execution using
[Wasmtime](https://wasmtime.dev/) with WASI. Commands execute in an isolated
filesystem tree with fuel metering and epoch-based timeout enforcement.

### How It Works

The WASM sandbox has two execution tiers:

**Tier 1 — Built-in commands** (~20 common coreutils implemented in Rust):
`echo`, `cat`, `ls`, `mkdir`, `rm`, `cp`, `mv`, `pwd`, `env`, `head`, `tail`,
`wc`, `sort`, `touch`, `which`, `true`, `false`, `test`/`[`, `basename`,
`dirname`.

These operate on a sandboxed directory tree, translating guest paths (e.g.
`/home/sandbox/file.txt`) to host paths under `~/.moltis/sandbox/wasm/<id>/`.
Paths outside the sandbox root are rejected.

Basic shell features are supported: `&&`, `||`, `;` sequences, `$VAR`
expansion, quoting via `shell-words`, and `>` / `>>` output redirects.

**Tier 2 — Real WASM module execution**: When the command references a `.wasm`
file, it is loaded and run via Wasmtime + WASI preview1 with full isolation:
preopened directories, fuel metering, epoch interruption, and captured I/O.

**Unknown commands** return exit code 127: "command not found in WASM sandbox".

### Filesystem Isolation

```
~/.moltis/sandbox/wasm/<session-key>/
  home/        preopened as /home/sandbox (rw)
  tmp/         preopened as /tmp (rw)
```

Home persistence is respected:
- `shared`: uses `data_dir()/sandbox/home/shared/wasm/`
- `session`: uses `data_dir()/sandbox/wasm/<session-id>/`
- `off`: per-session, cleaned up on `cleanup()`

### Resource Limits

- **Fuel metering**: `store.set_fuel(fuel_limit)` — limits WASM instruction
  count (Tier 2 only)
- **Epoch interruption**: background thread ticks epochs, store traps on
  deadline (Tier 2 only)
- **Memory**: `wasm_config.memory_reservation(bytes)` — Wasmtime memory limits
  (Tier 2 only)

### Configuration

```toml
[tools.exec.sandbox]
backend = "wasm"

# WASM-specific settings
wasm_fuel_limit = 1000000000       # instruction fuel (default: 1 billion)
wasm_epoch_interval_ms = 100       # epoch interruption interval (default: 100ms)

[tools.exec.sandbox.resource_limits]
memory_limit = "512M"    # Wasmtime memory reservation
```

### Limitations

- Built-in commands cover common coreutils but not a full shell
- No pipe support yet (planned via busybox.wasm in future)
- No network access from WASM modules
- `.wasm` modules must target WASI preview1

### When to Use

The WASM sandbox is a good fit when:

- You want filesystem-isolated execution without container overhead
- You need a sandboxed environment on platforms without Docker or Apple
  Container
- You are running `.wasm` modules and want fuel-metered, time-bounded execution

### Compile-Time Feature

The WASM sandbox is gated behind the `wasm` cargo feature, which is enabled by
default. To build without Wasmtime (saves ~30 MB binary size):

```bash
cargo build --release --no-default-features --features lightweight
```

When the feature is disabled and the config requests `backend = "wasm"`, Moltis
falls back to `restricted-host` with a warning.

## OS Sandbox (Seatbelt / bubblewrap)

When no container runtime is available, desktops still get kernel-enforced
isolation without installing anything: Moltis runs each command on the host
under a per-session policy enforced by the operating system.

| Platform | Mechanism | Requirement |
|----------|-----------|-------------|
| macOS | Seatbelt (`/usr/bin/sandbox-exec` with a generated profile) | built in |
| Linux | [bubblewrap](https://github.com/containers/bubblewrap) (user, mount, PID, IPC, UTS and network namespaces) | `bwrap` installed and unprivileged user namespaces enabled |
| Windows | not available yet, see [Windows sandboxing](sandbox-windows.md) | — |

Moltis probes the mechanism at startup by running a trivial command under it.
If the probe fails (missing `bwrap`, user namespaces disabled by the
distribution, or Moltis itself already running inside a sandbox), the next
backend in the chain is used.

### Policy

Both platforms apply the same deny-by-default policy:

| Area | Allowed |
|------|---------|
| System | read and execute only: `/usr`, `/bin`, `/sbin`, `/etc`, system frameworks, Homebrew (`/opt/homebrew`), Nix store, Xcode command-line tools |
| Workspace (`data_dir()`) | read-only with `workspace_mount = "ro"` (default), read-write with `"rw"`, hidden with `"none"` |
| Managed Files | follows `managed_files_mount`; stays read-only inside a writable workspace when set to `"ro"` |
| Sandbox home | read-write. Follows `home_persistence`: `data_dir()/sandbox/home/shared` (default), one directory per session, or a temporary one removed at cleanup |
| Temp | read-write `data_dir()/sandbox/os/<session>/tmp`, exported as `TMPDIR` and removed at cleanup. On Linux `/tmp` is a private tmpfs |
| Everything else | denied: the user's home directory, other users' files, the system temp directories |

Some paths stay denied even inside an allowed root: Moltis secrets in the
data directory (`credentials.json`, `provider_keys.json`, OAuth token files,
`mcp-servers.json`, `moltis.toml`, `.env`, `logs/`, `browser/`, and every
`*.db`/`*.sqlite` file with its WAL and journal), the config directory, and
credential stores under `$HOME` (`.ssh`, `.gnupg`, `.aws`, `.azure`, `.kube`,
`.docker`, `.netrc`, `.npmrc`, `.pypirc`, `.git-credentials`,
`.config/gcloud`, `.config/gh`, `Library/Keychains`, `Library/Cookies`).
These denies also apply when you grant extra paths.

Entries that Moltis loads or runs on the host stay read-only even with
`workspace_mount = "rw"`, so a sandboxed command cannot plant code that runs
outside the sandbox: `hooks/`, `disabled_hooks.json`, `plugins/`,
`plugins-manifest.json`, `installed-skills/`, `bundled-skills/`,
`skills-manifest.json`, the service supervisor script and pid files.

The environment is never inherited: commands get a fixed `PATH`,
`HOME` pointing to the sandbox home, `TMPDIR`, `LANG=C.UTF-8`,
`MOLTIS_SANDBOX=os`, proxy variables in `trusted` mode, and the environment
variables you configured in Moltis. `LD_*` and `DYLD_*` variables are
dropped so nothing runs in the launcher before the policy applies.

Because the policy is kernel-enforced, commands in the OS sandbox do not need
per-command approval, like container backends. Host paths stay valid: the
working directory, the workspace and managed Files keep their host location.

### Network

| `network` | macOS (Seatbelt) | Linux (bubblewrap) |
|-----------|------------------|--------------------|
| `"blocked"` | no network | no network (own network namespace) |
| `"trusted"` (default) | only TCP to the egress proxy on `127.0.0.1:18791`; `HTTP(S)_PROXY` is set | **no network**: the sandbox's network namespace cannot reach the proxy on the host loopback, so it fails closed |
| `"bypass"` | unrestricted IP traffic, including services listening on the host's loopback; Unix sockets of the host (ssh-agent, Docker) stay unreachable | host network, including the host's loopback |

Unix sockets are allowed only inside the sandbox home and temp, so tools such
as tmux work while the host's ssh-agent and Docker sockets stay out of reach.

### Granting extra paths

```toml
[tools.exec.sandbox]
backend = "os"
os_read_paths = ["/Users/me/src/reference-docs"]   # read-only
os_write_paths = ["/Users/me/src/my-project"]       # read-write
```

Relative paths are resolved against the data directory. Credential stores
listed above stay denied even when a parent directory is granted.

### Limitations

- Seatbelt (`sandbox-exec`) is deprecated by Apple but still shipped and used
  by Chrome, Codex and other agent tools; Moltis only trusts
  `/usr/bin/sandbox-exec`.
- Seatbelt cannot nest: if Moltis runs inside another Seatbelt sandbox (for
  example a sandboxed terminal), the probe fails and Moltis falls back.
- bubblewrap needs unprivileged user namespaces. Ubuntu 24.04 and later
  restrict them through AppArmor; install the distribution's `bwrap` profile
  or allow them with `sysctl kernel.apparmor_restrict_unprivileged_userns=0`.
- With `workspace_mount = "rw"` on Linux, a denied or read-only entry that
  does not exist yet (for example no `.env` or no `hooks/` in the data
  directory) is not masked, so a command could create it. Seatbelt denies
  creation as well. Keep the default `workspace_mount = "ro"` on Linux unless
  you need it.
- Resource limits use `ulimit` like the restricted host, except that the
  process count limit is only applied when `pids_max` is set: the kernel
  counts every process of your user, not just the sandboxed ones.
- No image building — `moltis sandbox build` returns immediately.

## Restricted Host Sandbox

The restricted-host sandbox provides lightweight isolation by running commands
on the host via `sh -c` with environment clearing and `ulimit` resource
wrappers. This is the fallback when no container runtime is available.

### How It Works

When the restricted-host sandbox runs a command, it:

1. **Clears the environment** — all inherited environment variables are removed
2. **Sets a restricted PATH** — only `/usr/local/bin:/usr/bin:/bin`
3. **Sets HOME to `/tmp`** — prevents access to the user's home directory
4. **Applies resource limits** via shell `ulimit`:
   - `ulimit -u` (max processes) from `pids_max` config (default: 256)
   - `ulimit -n 1024` (max open files)
   - `ulimit -t` (CPU seconds) from `cpu_quota` config (default: 300s)
   - `ulimit -v` (virtual memory) from `memory_limit` config (default: 512M)
5. **Enforces a timeout** via `tokio::time::timeout`

User-specified environment variables from `opts.env` are re-applied after the
environment is cleared, so the LLM tool can still pass required variables.

### Limitations

- No filesystem isolation — commands run on the host filesystem
- No network isolation — commands can make network requests
- `ulimit` enforcement is best-effort
- No image building — `moltis sandbox build` returns immediately

For production use with untrusted workloads, prefer Apple Container or Docker.

## No sandbox

If no runtime is found (and the `wasm` feature is disabled), commands execute
directly on the host. The startup banner will show a warning. This is **not
recommended** for untrusted workloads.

## Failover Chain

Moltis wraps the primary sandbox backend with automatic failover:

- **Apple Container → Podman/Docker → OS sandbox → Restricted Host**: if Apple
  Container enters a corrupted state (stale metadata, missing config, VM boot
  failure), Moltis fails over to Podman or Docker. If neither is available, it
  uses Seatbelt, and restricted-host only when Seatbelt is unusable.
- **Docker → Restricted Host**: if Docker loses its daemon connection during a
  session, Moltis fails over to the restricted-host sandbox.

Failover is sticky for the lifetime of the gateway process — once triggered,
all subsequent commands use the fallback backend. Restart the gateway to retry
the primary backend.

Failover triggers:

| Primary | Triggers |
|---------|----------|
| Apple Container | `config.json missing`, `VM never booted`, `NSPOSIXErrorDomain Code=22`, service errors |
| Docker | `cannot connect to the docker daemon`, `connection refused`, `is the docker daemon running` |

## Per-session overrides

The web UI allows toggling sandboxing per session and selecting a custom
container image. These overrides persist across gateway restarts.

## Home persistence

By default, `/home/sandbox` is persisted in a shared host folder so that CLI
auth/config files survive container recreation. You can change this with
`home_persistence`:

```toml
[tools.exec.sandbox]
home_persistence = "session"   # "off", "session", or "shared" (default)
# shared_home_dir = "/path/to/shared-home"  # optional, used when mode is "shared"
```

- `off`: no home mount, container home is ephemeral
- `session`: mount a per-session host folder to `/home/sandbox`; folder names
  are opaque, collision-resistant identifiers rather than raw session keys
- `shared`: mount one shared host folder to `/home/sandbox` for all sessions
  (defaults to `data_dir()/sandbox/home/shared`, or `shared_home_dir` if set)

Moltis stores persisted homes under `data_dir()/sandbox/home/`. Per-session
folder names are opaque, collision-resistant identifiers; do not derive or
depend on them directly. The `shared` mode is intentionally visible to every
sandbox session and is not a session-isolation boundary. Use `session` or `off`
for credentials or other private state when sessions have different trust
levels.

After upgrading from a version that used readable session folder names, gateway
startup discards legacy instance containers. Legacy home folders remain on disk
but are not automatically reused because multiple session keys may have mapped
to the same folder. Administrators can inspect and migrate trusted content
manually.

## Managed Files mount

[Managed Files](managed-files.md) use a separate mount from the Moltis
workspace and sandbox home:

```toml
[tools.exec.sandbox]
managed_files_mount = "ro" # "none", "ro" (default), or "rw"
```

Docker, Podman, and Apple Container mount the host's `<data_dir>/files` at
`/home/sandbox/files`. `none` makes that tree unavailable, `ro` permits reads,
and `rw` permits mutations. WASM and remote sandbox backends do not yet support
this mount.

Although `/home/sandbox/files` is below the home path, it is an independent
mount. `home_persistence = "shared"` does not enable Managed Files, and changing
`managed_files_mount` does not change how sandbox caches, credentials, or other
home files persist. Moltis advertises the effective path and mount mode to the
agent and sets `MOLTIS_FILES_DIR` for `exec`.

The default `ro` mode prevents a sandboxed command from changing Managed Files,
but every agent session whose tool and channel policy permits reads sees the
same tree. See the [Managed Files privacy warning](managed-files.md#privacy)
before storing sensitive data.

## Docker-in-Docker workspace mounts

When Moltis runs inside a container and launches Docker-backed sandboxes via a
mounted container socket, the sandbox bind mount source must be a host-visible
path. Moltis auto-detects this by inspecting the parent container's mounts. If
that lookup fails or you want to pin the value explicitly, set
`host_data_dir`:

```toml
[tools.exec.sandbox]
host_data_dir = "/srv/moltis/data"
```

This remaps sandbox workspace mounts and default sandbox persistence paths from
the guest `data_dir()` to the host path you provide. It is mainly an override
for Docker-in-Docker deployments where mount auto-detection is unavailable or
ambiguous.

## Network policy

By default, sandbox containers have no network access (`no_network = true`).
For tasks that need filtered internet access, use
[trusted network mode](trusted-network.md) — a proxy-based allowlist that
lets containers reach approved domains while blocking everything else.

```toml
[tools.exec.sandbox]
network = "trusted"
trusted_domains = ["registry.npmjs.org", "github.com"]
```

See [Trusted Network](trusted-network.md) for full configuration and the
network audit log.

> **Note**: Home persistence applies to Docker, Apple Container, and WASM
> backends. The restricted-host backend uses `HOME=/tmp` and does not mount
> persistent storage.

## Resource limits

```toml
[tools.exec.sandbox.resource_limits]
memory_limit = "512M"
cpu_quota = 1.0
pids_max = 256
```

How resource limits are applied depends on the backend:

| Limit | Docker | Apple Container | WASM | OS sandbox | Restricted Host | cgroup (Linux) |
|-------|--------|-----------------|------|------------|-----------------|----------------|
| `memory_limit` | `--memory` | `--memory` | Wasmtime reservation | `ulimit -v` | `ulimit -v` | `MemoryMax=` |
| `cpu_quota` | `--cpus` | `--cpus` | epoch timeout | `ulimit -t` (seconds) | `ulimit -t` (seconds) | `CPUQuota=` |
| `pids_max` | `--pids-limit` | `--ulimit nproc=` | n/a | `ulimit -u` (only when set) | `ulimit -u` | `TasksMax=` |

Apple Container requires version 0.9 or newer to apply `pids_max`.
Its `--cpus` option only accepts positive whole CPU counts, so fractional
`cpu_quota` values are not supported by that backend.

## Comparison

| Feature | Apple Container | Docker | WASM | OS sandbox | Restricted Host | none |
|---------|----------------|--------|------|------------|-----------------|------|
| Filesystem isolation | ✅ VM boundary | ✅ namespaces | ✅ sandboxed tree | ✅ kernel policy on host FS | ❌ host FS | ❌ |
| Network isolation | ✅ | ✅ | ✅ (no network) | ✅ (proxy-only on macOS, blocked on Linux) | ❌ | ❌ |
| Kernel isolation | ✅ separate kernel | ❌ shared kernel | ✅ WASM VM | ❌ shared kernel | ❌ | ❌ |
| Environment isolation | ✅ | ✅ | ✅ | ✅ cleared + restricted | ✅ cleared + restricted | ❌ |
| Resource limits | ✅ | ✅ | ✅ fuel + epoch | ✅ ulimit | ✅ ulimit | ❌ |
| Image building | ✅ (via Docker) | ✅ | ❌ | ❌ | ❌ | ❌ |
| Shell commands | ✅ full shell | ✅ full shell | ~20 built-ins | ✅ host tools | ✅ full shell | ✅ full shell |
| Managed Files mount | yes: `none`/`ro`/`rw` | yes: `none`/`ro`/`rw` | not yet | host path, `none`/`ro`/`rw` enforced | host path, no mount | host path, no mount |
| Platform | macOS 26+ | any | any | macOS, Linux | any | any |
| Overhead | low | medium | minimal | minimal | minimal | none |
