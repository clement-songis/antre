# Antre shell (Tauri 2 spike)

A thin Tauri 2 window around the web UI served by the Moltis daemon.
Findings, measurements and recommendation: `plans/research/tauri-spike.md`.

Separate Cargo workspace, not built by the root `cargo build`.

```bash
cd apps/tauri
node bench/build.mjs                  # dist/: connecting page + bench pages (needs crates/web/ui/node_modules and a built style.css)
cargo run                             # auto: local daemon, else a sidecar
cargo run -- --sidecar --isolated     # always a sidecar, own data dir
cargo run -- --remote https://host:13131   # API key in ANTRE_API_KEY
cargo run --features embedded -- --embedded   # needs `just build-web-assets` first
cargo test
```

The sidecar needs a `moltis` binary: next to the shell, `MOLTIS_BIN`, `PATH`,
or `--moltis PATH`.

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
