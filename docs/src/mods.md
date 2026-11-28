# Mods

Mods extend Antre with tools and hooks that run as sandboxed WebAssembly
components. A mod asks for capabilities in its manifest; nothing runs until a
person approves exactly that set of capabilities, and an update that asks for
more waits for a new approval while the old version keeps running.

This page covers the server side (phase 1 of the mod SDK). UI contributions
(panes, status bar, renderers) come later; the design is in
`plans/2026-11-28-plan-mods-sdk.md`.

## What a mod can do today

| Contribution | Declared in | Runs as |
|---|---|---|
| Tool offered to the agent | `[[contributes.tools]]` | `antre:mods/tools.call` |
| Hook handler (observe or intercept) | `[[contributes.hooks]]` | `antre:mods/hooks.handle` |

Mod tools are available to trusted (owner) turns only, never to channel or
webhook turns. A mod tool cannot replace a built-in tool or another mod's tool
with the same name: the first registration wins.

## Package layout

```text
word-stats/
  antre-mod.toml           # manifest (required)
  server/word_stats.wasm    # component for the antre:mods/plugin world
```

The WIT contract is `wit/mod/antre-mod.wit` (package `antre:mods@0.1.0`).
Build a Rust mod with `wit-bindgen` for the `plugin` world and the
`wasm32-wasip2` target. The demo in `examples/mods/word-stats` is a complete
example; `just mod-demo` builds it into `target/mods/word-stats`.

## Manifest

```toml
schema      = 1
id          = "acme.word-stats"    # <publisher>.<name>, lowercase
version     = "0.1.0"
name        = "Word stats"
description = "Counts words."
license     = "MIT"
sdk         = "0.1"                # must match the host SDK version

[server]
component = "server/word_stats.wasm"

[capabilities]
net     = ["api.github.com"]       # HTTP hosts; "*.example.org" for subdomains
storage = "4KiB"                   # per-mod key/value storage quota

[[contributes.tools]]
name        = "word_stats"
description = "Count words, lines and characters in a text."
parameters  = { type = "object", properties = { text = { type = "string" } }, required = ["text"] }

[[contributes.hooks]]
id       = "count-sessions"
events   = ["SessionStart"]        # Antre or Claude Code event names
mode     = "observe"               # or "intercept" (block / modify)
priority = 0
```

Unknown keys are errors. The publishers `core`, `local`, `antre` and
`antre` are reserved. Packages cannot ask for `exec` (native processes).

## Capabilities and consent

The capability set is what the manifest declares plus what its contributions
imply:

| Capability | Risk | Implied by |
|---|---|---|
| `tool` | medium | any tool |
| `hook:<Event>:observe` | medium | an observe hook, or any hook on a read-only event |
| `hook:<Event>:intercept` | high | an intercept hook on an event that can be blocked |
| `net:<host>` | medium | `capabilities.net` |
| `storage:<bytes>` | low | `capabilities.storage` |

The **consent hash** is a SHA-256 over the sorted canonical capability list.
Rules:

1. Installing never grants anything. A new mod is installed **disabled**.
2. Granting requires the exact consent hash of the capability set being
   approved, so a client cannot approve something it did not show.
3. Non-interactive callers (the agent, channels, schedules) can never grant.
   RPC and REST callers with `operator.admin` act for the operator and count as
   interactive.
4. An update whose capabilities fit within the grant applies at once. An
   update that adds any capability is staged as `pendingUpdate`; the current
   version keeps running until its consent hash is approved.
5. Revoking disables and unloads the mod immediately.

## Sandbox

Each call into a mod gets a fresh instance with:

- a fuel budget (`[mods] fuel`), a memory limit (`memory_mb`) and a wall-clock
  deadline (`timeout_ms`, enforced with an engine-wide epoch tick);
- a bounded return size (`max_output_kb`);
- WASI with no preopened directories, no environment variables and no
  sockets.

At install, the host reads the component's imports and rejects it if it
imports anything outside `antre:mods/*` and WASI, imports `wasi:sockets` or
`wasi:http`, or imports `antre:mods/http` without declaring `net` hosts.
At run time every host call checks the grant again: storage calls fail with
`capability denied: storage` without a storage grant, HTTP calls fail with
`denied` for hosts not granted (exact names only; `*.example.org` for
subdomains), and private, loopback and link-local addresses stay blocked by
the SSRF filter. Redirects are not followed.

## Installing and managing mods

Installs come from a folder on the gateway host. The folder is copied into a
content-addressed store under `<data_dir>/mods/store/`; symlinks, special
files and oversized packages are rejected, and validation runs on the copy.

RPC (`mods.list` and `mods.get` need `operator.read`; the rest need
`operator.admin`):

| Method | Params |
|---|---|
| `mods.list` | — |
| `mods.get` | `id` |
| `mods.install` | `path` (absolute) |
| `mods.grant` | `id`, `consentHash`, `enable` (optional) |
| `mods.enable` / `mods.disable` | `id` |
| `mods.revoke` | `id` |
| `mods.uninstall` | `id` |

REST, same semantics: `GET /api/mods`, `GET /api/mods/{id}`,
`POST /api/mods/install` (`{"path": …}`), `POST /api/mods/{id}/grant`
(`{"consentHash": …, "enable": true}`), `POST /api/mods/{id}/enable`,
`/disable`, `/revoke`, `DELETE /api/mods/{id}`.

A typical first install:

```bash
just mod-demo
```

Then call `mods.install` with the printed path, read `consent.consent_hash`
and the listed capabilities from the result, and call `mods.grant` with that
hash and `enable: true`.

## Built-in hooks

The built-in hooks (command logger, session memory, auto-checkpoint) are
registered through the same contribution registry as mod hooks, under
`core/hook.*` ids. `mods.list` returns every contribution with its namespaced
id.

## Configuration

```toml
[mods]
enabled = true            # load mods and expose mods.* / /api/mods
fuel = 50000000           # per call
memory_mb = 32            # per call
timeout_ms = 10000        # per call
epoch_interval_ms = 50
max_output_kb = 1024
pin_shell_hooks = true    # see Hooks → Content pinning
```

## Storage

| Path | Content |
|---|---|
| `<data_dir>/mods/state.json` | installed mods, grants, pending updates, shell hook pins |
| `<data_dir>/mods/store/<sha256>/` | installed package copies |
| `<data_dir>/mods/kv/<mod-id>.json` | per-mod key/value storage |

The cargo feature is `mods`, on by default in the CLI build.
