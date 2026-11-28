# Antre shell (Tauri 2)

A thin Tauri 2 window around the web UI served by the Moltis daemon.
Findings, measurements and recommendation: `plans/research/tauri-spike.md`.

Separate Cargo workspace, not built by the root `cargo build`.

```bash
cd apps/tauri
node bench/build.mjs                  # dist/: connecting page + bench pages (needs crates/web/ui/node_modules and a built style.css)
cargo run                             # default: sidecar daemon (reuses the configured one if already running)
cargo run -- --sidecar --isolated     # always a sidecar, own data dir
cargo run -- --remote https://host:13131   # ANTRE_API_KEY (operator.admin) opens the UI signed in
cargo run --features embedded -- --embedded   # needs `just build-web-assets` first
cargo test
```

The sidecar needs a `moltis` binary: next to the shell, `MOLTIS_BIN`, `PATH`,
or `--moltis PATH`.

## Behaviour

- **One instance**: launching the app again focuses the running window.
- **Tray**: closing the window hides it and the agent keeps running; "Quit
  Antre" in the tray (or Cmd+Q / SIGTERM) stops the window and the sidecar.
- **Notifications**: a native notification when the sidecar crashes and
  comes back, or keeps crashing.
- **Remote**: with an API key that has `operator.admin`, the shell trades it
  for a one-time session ticket (`POST /api/auth/session-ticket`) and opens
  the redeem URL, so the user is signed in; with a weaker key or none, the
  user logs in once in the window.
- Pages served by a daemon get no Tauri IPC (`capabilities/local.json`).

## Packaging with the sidecar

```bash
apps/tauri/scripts/stage-sidecar.sh             # release moltis → binaries/moltis-<triple>
cd apps/tauri && cargo tauri build --config tauri.bundle.conf.json
```

`tauri.bundle.conf.json` adds the `externalBin` entry; it is kept out of
`tauri.conf.json` so plain `cargo build` does not require a staged binary.

## Benchmarks

```bash
node bench/build.mjs
cargo build
caffeinate -d -u node bench/run.mjs --runs 3 --out bench/results/<date>/matrix
node bench/summarize.mjs bench/results/<date>/matrix   # Markdown tables (medians)
```

`cargo build` after `node bench/build.mjs`: the shell embeds `dist/` at
compile time. Keep the display awake and unlocked: WKWebView stops `requestAnimationFrame`
and throttles timers when the window is hidden or the display sleeps. The
bench window is focused and kept on top for the same reason.

WebKitGTK (Linux): `bench/webkitgtk/README.md` runs the same pages in a
container, or natively on a Linux machine.
