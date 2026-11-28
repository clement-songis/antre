# Tauri 2 shell spike (Antre)

Date: 2026-11-27. Branch: `claude/tauri-spike-957729`. Code: `apps/tauri/`.

Question: can the existing web UI, hosted by a thin Tauri 2 shell around the
Rust daemon, be the single product surface on Windows, Linux, macOS, Android
and iOS? Context: `compare-oss-landscape.md` §6 recommended web UI + Tauri,
conditional on this spike.

## TL;DR

- **Go**, with conditions. The web UI in a Tauri 2 shell on macOS streams a
  4,000-token Markdown reply at 400 tok/s with no dropped frames, opens a
  2,000-message transcript in ~0.3 s and scrolls it without a dropped frame,
  using less memory than Chromium for the same pages. The shell itself is
  ~770 lines of Rust (plus ~300 lines of tests) and builds in 45 s.
- **Two WebKit limits** showed up, both outside our code:
  1. WKWebView runs `requestAnimationFrame` at **60 Hz** on this 120 Hz Mac
     (Chromium: 120 Hz). Smooth, but not ProMotion-smooth.
  2. WKWebView **throttles sandboxed (opaque-origin) iframes to ~20 fps** as
     long as the user has not interacted with them, whatever their content: a
     near-empty page gets 20 fps too, and the same heavy design served
     same-origin runs at 60 fps. Canvas/artifact/MCP-App animations will
     look choppy until clicked. Static designs are unaffected.
- **WebKitGTK, measured in a container** (§2.7, software rendering, no
  GPU): the chat passes, a heavy animated canvas does not.
  - Chat: 400 tok/s streams at 54 fps; the 2,000-message transcript scrolls
    at 58 fps.
  - Heavy canvas: 4–14 fps, where Chromium in the same container keeps
    60 fps.
  - A real click lifts the opaque-origin iframe throttle (25 → 49 fps),
    which confirms limit 2 above.
  - Still unmeasured: WebKitGTK on a real Linux machine with a GPU, and
    Android WebView.
- **End to end works**: the shell spawns the daemon, the real web UI loads
  and connects its WebSocket, and quitting stops the daemon.
- **Gateway findings that matter for any desktop shell:** a cold daemon took
  200–300 s to answer here, blocked on an untimed Docker probe while Docker
  Desktop was hung (3 s without it); the
  trusted-network proxy binds a fixed port (`0.0.0.0:18791`), so a sidecar and
  a user service cannot run side by side; an isolated daemon silently imported
  `~/.codex/auth.json` tokens into its own config dir.


## 1. What was built

`apps/tauri/` is a separate Cargo workspace (like `apps/desktop`), not a
member of the main one: Tauri pulls ~400 crates (wry, tao, objc2, and on Linux
the `webkit2gtk` system libraries that CI images do not have). The default
build (sidecar and remote modes) compiles in ~45 s from scratch on this Mac
and adds nothing to the main workspace's builds.

```
apps/tauri/
  src/cli.rs        modes and flags (unit-tested)
  src/connect.rs    attach / spawn / embed / remote
  src/sidecar.rs    find `moltis`, free port, spawn, health, restart with backoff
  src/probe.rs      /health and API-key checks (unit-tested against a fake server)
  src/embedded.rs   gateway in-process (`--features embedded`)
  src/bench.rs      bench pages report through one IPC command
  bench/            bench pages (JS), esbuild bundler, Playwright/ps harness
  capabilities/     IPC only for the shell's own pages
```

Modes:

| Mode | Command | What happens |
|---|---|---|
| auto (default) | `antre-shell` | Uses the daemon from `~/.moltis/moltis.toml` when it answers over plain HTTP; otherwise starts a sidecar. |
| sidecar | `antre-shell --sidecar [--moltis PATH] [--isolated]` | Spawns `moltis --bind 127.0.0.1 --port <free> --no-tls --no-open-browser gateway`, waits for `/health`, loads it, restarts it with backoff (5 tries) if it crashes, kills it on exit. `--isolated` uses the shell's app-data dir instead of `~/.moltis`. |
| embedded | `antre-shell --embedded` (built with `--features embedded`) | Calls `moltis_httpd::server::start_gateway` on its own thread and runtime inside the shell, on the shell's app-data directories. |
| remote | `ANTRE_API_KEY=… antre-shell --remote https://host:port` | Checks `/health`, checks the key with `GET /api/auth/status` (Bearer), then loads the URL. Plain `http` is refused except to loopback. The key is read from the environment, never from argv (visible in `ps`). |
| bench | `antre-shell --bench stream\|transcript\|canvas [--out FILE]` | Loads a bundled bench page, writes its JSON report, exits. |

Security choices in the shell:

- Daemon pages (local or remote) get **no Tauri IPC**: the only capability
  (`capabilities/local.json`) covers the shell's bundled pages. A compromised
  or remote daemon cannot reach native APIs through the shell.
- The sidecar binds loopback only and runs without TLS: the system WebView
  would reject the daemon's self-signed certificate, and loopback traffic
  stays on the machine. An attached local daemon that uses TLS is skipped
  (a sidecar is started instead) for the same reason.
- The web UI keeps its own auth: same-origin cookie and WebSocket Origin
  checks are unchanged because the page is served by the daemon itself.

### Gaps found

- **Remote auth.** The web UI authenticates with a session cookie; a WebView
  cannot add `Authorization: Bearer` to every request and to the WebSocket. So
  the API key only lets the shell validate the connection; the user still logs
  in once in the WebView (cookie persists in the WebView's store). A real fix is
  a small gateway endpoint that trades a scoped API key (or a pairing code) for
  a session cookie. Question filed in `plans/questions.md`.
- **Embedded mode** works (verified, §2.6) but costs: the shell workspace must copy the root `[patch.crates-io]` entries
  (sqlx fork, vendored ACP) to resolve at all, and compiling the gateway into
  the shell is a full gateway build in a second target directory
  (see §2.5). `start_gateway` also assumes it owns the process: process-wide
  config/data dir overrides, and its future is not `Send`, so it needs a
  dedicated thread and runtime. The sidecar is simpler and
  keeps the daemon's crash domain separate from the UI.
- **Daemon cold start.** A fresh isolated daemon took 200–300+ s to answer
  `/health` on this Mac, all of it between "vault ready" and "sandbox
  backend: docker". Docker Desktop was hung (`docker info` did not answer in
  30 s) and the sandbox backend probe waits on it with no timeout. With
  `[tools.exec.sandbox] backend = "restricted-host"` the same daemon answers
  in **3 s**. The shell waits up to 5 minutes; the real fix is in the gateway
  (time-box the probe, listen first, §4).
- **Fixed ports.** The trusted-network proxy listens on `0.0.0.0:18791`
  whatever the gateway port, so a sidecar cannot run next to a running user
  service, and it listens on all interfaces.
- **Credential import.** An `--isolated` daemon still auto-imported the
  Codex CLI tokens from `~/.codex/auth.json` into its own config directory
  (`~/Library/Application Support/org.antre.shell.spike/daemon/config/oauth_tokens.json`
  on this Mac). Isolation for a desktop shell should include not reading
  other tools' credentials without asking.
- **Graceful stop.** The sidecar is stopped with SIGKILL (`tokio` child kill);
  SQLite in WAL mode survives that, but a SIGTERM path (via a safe signal crate)
  should come before shipping.

## 2. Measurements (macOS, WKWebView vs Chromium)

### 2.1 Method

- Machine: this Mac (Apple Silicon, macOS 27 / Darwin 27.2, 120 Hz display,
  device pixel ratio 2), plugged in, display kept awake with
  `caffeinate -d -u`, nothing else heavy running.
- Engines: WKWebView inside the Tauri shell (`antre-shell --bench …`,
  Tauri 2.12.7, debug build of the shell; the shell's build profile does not
  affect page JS) vs Playwright's Chromium 145 (headed, 1280×860 viewport,
  DPR 2). Playwright was used instead of the `moltis-browser` crate: same
  engine, and it ships the harness plumbing.
- Pages (`apps/tauri/bench/src/`): the chat's own Markdown path, copied from
  `helpers.ts`/`ws/shared.ts` (same `marked` options and renderer), the app's
  real `style.css`, the app's real `code-highlight.ts` (Shiki). Content is
  generated with a seeded PRNG, so both engines render identical bytes.
  - **stream**: a 16.7 KB reply (≈4,165 tokens at 4 chars/token: headings,
    lists, tables, 10 code blocks), delivered in 3-token deltas. Every delta
    re-renders the whole accumulated text and scrolls to the bottom, exactly
    like `handleChatDelta`. Three runs inside one page: flat out (render +
    forced layout per delta, back to back), paced 100 tok/s, paced 400 tok/s
    (each delta a separate task, like WebSocket messages), then Shiki on the
    final message.
  - **transcript**: 2,000 messages (674 KB of Markdown, 198 code blocks, 28k
    DOM nodes) rendered at once, then Shiki on all code blocks, then an 8 s
    programmatic scroll from top to bottom, then 40 jumps to positions.
  - **canvas**: a 788 KB generated design (5,110 elements: gradients, shadows,
    `backdrop-filter`, CSS animations, 240 SVG sparklines, an animated
    `<canvas>`) in `<iframe sandbox="allow-scripts" srcdoc=…>`, the way canvas,
    artifacts and MCP Apps are embedded. Measures load, idle animation frames
    and scrolling inside the iframe (the iframe counts its own frames and posts
    them back), 5 remounts with new content, and two variants (a near-empty
    page in the same sandbox; the heavy design with `allow-same-origin`).
- Frames: `requestAnimationFrame` timestamps; refresh period measured on an
  idle page first; a frame is "dropped" when a gap spans more than one
  refresh. WebKit coarsens `performance.now()` to 1 ms; Chromium to 0.1 ms.
- Memory: peak RSS sampled from `ps` every 200 ms, from outside: WKWebView's
  `com.apple.WebKit.WebContent` (and GPU) processes started by the run;
  Chromium's renderer (and GPU) processes. RSS is a rough proxy (shared pages
  are counted), good for an order of magnitude only.
- Runs: 3 per engine and page; tables show medians. Raw JSON:
  `apps/tauri/bench/results/2026-11-27/` (`matrix/` for stream and
  transcript; `canvas/` for the canvas page, re-run after fixing a
  double-counted host frame counter and adding the variants; the canvas files
  in `matrix/` predate that fix). Reproduce: `apps/tauri/README.md`.

### 2.2 Streaming a long reply

| Metric | WKWebView (Tauri) | Chromium 145 |
|---|---:|---:|
| Refresh rate seen by rAF (Hz) | 59 | 120 |
| Flat out: max tokens/s (render + layout per delta) | 1,628 | 2,218 |
| Flat out: per-delta ms, first 10% of reply | 0.23 | 0.25 |
| Flat out: per-delta ms, last 10% of reply | 3.2 | 2.36 |
| Flat out: per-delta ms p95 | 4 | 2.5 |
| 100 tok/s: rendered tok/s | 100 | 100 |
| 100 tok/s: fps / dropped frames | 60 / 0% | 120 / 0% |
| 100 tok/s: worst frame (ms) | 32 | 9.4 |
| 400 tok/s: rendered tok/s | 399.9 | 399.9 |
| 400 tok/s: fps / dropped frames | 60 / 0% | 120 / 0% |
| 400 tok/s: worst frame (ms) | 22 | 9.4 |
| 400 tok/s: delta handler p95 (ms) | 3 | 3.2 |
| Shiki after the stream (ms, incl. init) | 168 | 456 |
| Peak page process RSS (MB) | 276 | 369 |
| Peak GPU process RSS (MB) | 32 | 120 |

Reading: both engines keep up with 400 tok/s (faster than any hosted model
streams today) with no dropped frames. The cost per delta grows linearly
with the reply because the UI re-parses the whole message on every delta
(0.2 ms at the start, 3 ms at the end of a 4k-token reply): at ~16k tokens
it would reach ~12 ms per delta and start eating a 60 Hz frame budget. That
is a UI algorithm issue, identical in every engine; incremental rendering
(re-parse only the last block) would remove it.

### 2.3 A 2,000-message transcript

| Metric | WKWebView (Tauri) | Chromium 145 |
|---|---:|---:|
| Markdown for 2,000 messages (ms) | 48 | 29 |
| DOM insert + layout (ms) | 119 | 140 |
| Time to first paint (ms) | 304 | 217 |
| Shiki on 198 code blocks (ms) | 377 | 1,285 |
| Scroll 8 s top→bottom: fps / dropped | 60 / 0% | 120 / 0% |
| Scroll: worst frame (ms) | 19 | 9.4 |
| Jump to a position, p95 (ms to 2nd frame) | 34 | 18 |
| Peak page process RSS (MB) | 460 | 584 |
| Peak GPU process RSS (MB) | 65 | 104 |

Reading: no virtualisation is needed at 2,000 messages on this hardware in
either engine. Jump latency is two frames in both (34 ms = 2 × 16.7 ms).

### 2.4 Heavy design in a sandboxed `srcdoc` iframe

| Metric | WKWebView (Tauri) | Chromium 145 |
|---|---:|---:|
| First mount: design script runs (ms) | 28 | 39 |
| Remount with new content, mean of 5 (ms) | 22 | 37 |
| Idle animations 3 s: iframe fps (dropped) | 21.6 (65%) | 120 (0%) |
| Scroll inside the design 4 s: iframe fps (dropped) | 23.1 (61%) | 120 (0%) |
| Scroll: iframe worst frame (ms) | 64 | 9 |
| Host page fps meanwhile | 58.3 | 120 |
| **Variant: near-empty page, same sandbox: iframe fps** | **20.5** | 120 |
| **Variant: heavy design, `allow-same-origin`: iframe fps** | **60** | 120 |
| Peak page process RSS (MB) | 119 | 2,193 |
| Peak GPU process RSS (MB) | 76 | 194 |

Reading: loading is fast everywhere (under 40 ms for 788 KB). The ~20 fps in
WKWebView is not rendering cost: an almost empty page in the same sandbox
gets the same 20 fps, and the heavy design runs at the full 60 fps once it is
same-origin. This matches WebKit's throttling of `requestAnimationFrame` in
cross-origin frames the user has not interacted with (an anti-ad/energy
measure); the sandbox without `allow-same-origin` gives the frame an opaque
origin, which counts as cross-origin. WebKit lifts it after a user gesture in
the frame (not verified on macOS, where the harness cannot produce a trusted click; verified in WebKitGTK, §2.7).
`allow-same-origin` is not an acceptable fix for untrusted designs: combined
with `allow-scripts` it lets the frame remove its own sandbox. Options:
accept it (designs become smooth once touched), or serve canvas content from
a dedicated origin and test whether a first-party separate origin is treated
the same way, or patch the WKWebView preference through a small Tauri plugin
(private WebKit preference, App Store risk). The Chromium RSS figure looks
too high to take at face value (it may include tile memory for the animated
1600×640 canvas at DPR 2 over six mounts); it is reported as measured.

### 2.5 Build cost

- Default shell (sidecar/remote): 45 s from scratch, 9–20 s incremental, on
  this Mac; binary is a debug build (release not measured).
- `--features embedded`: requires copying the root `[patch.crates-io]` (sqlx
  fork, vendored ACP) into the shell workspace just to resolve, and the
  generated web assets (`just build-web-assets`) because `moltis-web` refuses
  to build without them. First build: **~7 min wall clock, ~19 CPU-minutes**
  (the whole gateway compiled a second time in the shell's target directory);
  incremental shell-only rebuilds ~20 s. Debug binary: 760 MB (vs a few MB
  of shell code without it).
- `start_gateway`'s future is **not `Send`** (it holds tracing values across
  awaits), so it cannot run on the shell's runtime with `tokio::spawn`; the
  embedded mode gives it its own thread and runtime.

### 2.6 End-to-end with the real web UI

`antre-shell --sidecar --isolated --moltis target/debug/moltis` (the
debug daemon from the main checkout):

- The shell found a free port, spawned the daemon, waited for `/health`
  (4 s with the Docker probe disabled; 275 s, and once over 300 s, with the
  hung Docker Desktop, see "Daemon cold start"), then navigated the WKWebView
  to it. The real web UI loaded, opened its
  WebSocket (`/health` reported `connections: 1`, the daemon persisted the
  browser timezone from the page) and showed onboarding.
- Quitting with SIGTERM first left the daemon orphaned (signals bypass
  Tauri's `RunEvent::Exit`); the shell now turns SIGTERM/SIGINT into a normal
  exit, which shuts the sidecar down (verified: no daemon left). A shell **crash** still orphans it:
  the daemon should exit when its parent dies (stdin pipe closed, or a
  `--parent-pid` watchdog) before shipping.
- Embedded (`--features embedded -- --embedded`, on the shell's own
  directories): gateway answered in 8 s, the web UI connected
  (`connections: 1`), shell RSS 302 MB with the gateway inside, and the
  gateway stopped with the shell.
- Not tried: the remote mode against a real remote TLS daemon (the probes and
  URL rules are unit-tested against a local fake server).

### 2.7 WebKitGTK (Linux), in a container

Run 2026-11-28 on branch `claude/webkitgtk-bench`. No Linux machine was
available, so the same pages ran in WebKitGTK inside a Linux container on
this Mac, with Chromium in the same container as the baseline.

**Method and caveats:**
- **Image:** Debian trixie arm64, `libwebkit2gtk-4.1-0` **2.54.3** (the
  library Tauri 2 links on Linux) driven through its `MiniBrowser`, and
  Debian's Chromium 156. Both run under Xvfb at 1280×860, DPR 1.
  - The container runs in Docker Desktop's linuxkit VM (kernel 7.0, 12
    vCPU, 8 GB), so there is no GPU: **both engines render in software**.
  - Xvfb has no vsync. WebKitGTK paces `requestAnimationFrame` on a timer
    (50–56 Hz measured); Chromium paces it at 60 Hz.
- **Harness:** `apps/tauri/bench/webkitgtk/` (Dockerfile, `harness/run.py`,
  README).
  - It serves the same `dist/` bundle as §2 and gets results back through
    an injected script instead of Tauri IPC.
  - It samples RSS from `ps`.
  - `--click-on` sends a real X11 click (xdotool), which the macOS harness
    could not do.
- **Not the Tauri shell:** MiniBrowser uses the same engine but not wry's
  WebView settings. Raw JSON is in
  `apps/tauri/bench/results/2026-11-28-webkitgtk/`, 3 runs per column, and
  the tables show medians.
- **Read the columns against each other,** not against §2.2–2.4: different
  CPU budget, no GPU, DPR 1, 60 Hz.

| Metric | WebKitGTK 2.54 | WebKitGTK, `WEBKIT_DISABLE_COMPOSITING_MODE=1` | Chromium 156 |
|---|---:|---:|---:|
| **Stream** | | | |
| Refresh rate seen by rAF (Hz) | 53 | 50 | 60 |
| Flat out: max tokens/s | 1,454 | 1,836 | 2,704 |
| Flat out: per-delta ms, last 10% of reply | 3.9 | 3.0 | 2.0 |
| 400 tok/s: fps / dropped frames | 54.4 / 4.4% | 60.1 / 0% | 60 / 0% |
| 400 tok/s: worst frame (ms) | 46 | 21 | 16.8 |
| 400 tok/s: delta handler p95 (ms) | 9 | 3 | 2.3 |
| 100 tok/s: fps / worst frame (ms) | 54.8 / 65 | 86 / 29 | 60 / 16.8 |
| Shiki after the stream (ms) | 234 | 170 | 157 |
| **Transcript (2,000 messages)** | | | |
| Markdown (ms) | 46 | – | 27.5 |
| DOM insert + layout (ms) | 186 | – | 136 |
| Time to first paint (ms) | 345 | – | 230 |
| Shiki on 198 code blocks (ms) | 382 | – | 318 |
| Scroll 8 s: fps / dropped / worst frame (ms) | 57.6 / 0% / 24 | – | 60 / 0% / 16.8 |
| Jump to a position, p95 (ms) | 42 | – | 38 |
| **Memory (stream page)** | | | |
| Page process RSS (MB) | 588 | 485 | 407 |
| All engine processes (MB) | 807 | – | 1,048 |

Canvas page (heavy 788 KB design in `<iframe sandbox="allow-scripts" srcdoc>`):

| Metric | WebKitGTK 2.54 | WebKitGTK + real click | WebKitGTK, no compositing | Chromium 156 |
|---|---:|---:|---:|---:|
| First mount: design script runs (ms) | 44 | 47 | 148 | 60 |
| Remount, mean of 5 (ms) | 33 | 25 | 74 | 42 |
| Idle animations 3 s: iframe fps (dropped) | 14.2 (75%) | 27.5 (58%) | 22.7 (44%) | 60 (0%) |
| Scroll inside the design 4 s: iframe fps | 4.3 | 6 | 15.1 | 60 |
| Host page fps during that scroll | 4.3 | 6 | 15.6 | 60 |
| **Near-empty page, same sandbox: iframe fps** | **25.7** | **49.2** ¹ | 25.3 | 60 |
| Heavy design, `allow-same-origin`: iframe fps | 15.4 | 26.1 | 22.8 | 60 |
| All engine processes (MB) | 795 | – | – | 1,179 |

¹ A separate run with the click sent into the near-empty iframe instead
(`--click-on variants`). The "+ real click" column otherwise clicks the
heavy design once, right after its first mount.

Reading:

- **Chat surfaces pass in software rendering.**
  - **Streaming:** 400 tok/s streams at 54 fps with 4% dropped frames and
    no frame over 50 ms.
  - **Transcript:** the 2,000-message transcript scrolls at 58 fps with no
    dropped frame and paints in 0.35 s.
  - WebKitGTK is 1.3–2× slower than Chromium per operation (Markdown, layout,
    per-delta render), roughly the WKWebView/Chromium gap on macOS. Both chat
    figures stay above the ~45 fps no-go line of §4 with no GPU at all.
- **Heavy animated canvases do not pass in software rendering.**
  - The 5,000-element design (gradients, shadows, `backdrop-filter`, an
    animated `<canvas>`) idles at 14 fps and scrolls at 4 fps in WebKitGTK.
    It also drags the host page down with it: the iframe shares the page's
    web process.
  - Chromium draws the same design at 60 fps in the same software
    environment, so its CPU rasteriser is far faster on this content.
  - Whether a GPU closes the gap is the main thing a real Linux machine must
    tell. The likely culprits are WebKitGTK's GPU-less paths for
    `backdrop-filter` and large blurred shadows.
- **The opaque-origin iframe throttle is WebKit-wide, and a user gesture
  lifts it.**
  - A near-empty sandboxed page gets ~25 fps in WebKitGTK (macOS: ~20) and
    49 fps after one real click inside it.
  - This confirms the §2.4 hypothesis that macOS could not check. Designs
    become smooth once touched, on Linux and very likely on macOS too.
- **`WEBKIT_DISABLE_COMPOSITING_MODE=1`** (a common Tauri-on-Linux
  workaround) helps without a GPU: streaming has no dropped frames, and the
  heavy scroll rises from 4 to 15 fps.
  - Costs: rAF is no longer tied to a display refresh (86 "fps" at
    100 tok/s), and iframe mounts are 3× slower.
  - It is a per-machine workaround, not a default. Re-measure it on real
    hardware.
- **Memory:** the WebKitGTK web process is larger than Chromium's renderer
  (588 vs 407 MB). WebKitGTK still uses ~25% less in total (807 vs
  1,048 MB) because it has no separate GPU process and a lighter UI
  process. As on macOS, RSS is an order of magnitude only.

Still to do on a real Linux machine (procedure in
`apps/tauri/bench/webkitgtk/README.md`, "On a Linux machine"):

- A GPU and a real display, on X11 and Wayland.
- The distro's own WebKitGTK.
- `antre-shell --bench` itself rather than MiniBrowser.

## 3. Platforms

### 3.1 Windows (WebView2)

- Engine: Edge WebView2 (Chromium), the same engine family as the Chrome
  column in §2, so rendering and performance should track Chrome closely,
  including 120 Hz+ `requestAnimationFrame` (Chromium does not cap it).
- Runtime: preinstalled on Windows 11 and current Windows 10. Tauri's bundler
  can download the bootstrapper at install time (the default), embed the
  offline installer, or ship a fixed runtime with the app (well over 100 MB,
  full control over the version). Sizes per the Tauri docs, not measured.
- Packages: NSIS `.exe` and WiX `.msi` from `tauri build` on a Windows runner
  (cross-compiling from macOS/Linux works for NSIS only, experimental).
- Sidecar: `moltis.exe` declared as `bundle.externalBin` (target-triple
  suffixed), placed next to the shell exe; `find_moltis` already looks there.
  The daemon has no Windows service manager integration today
  (`moltis service` targets launchd/systemd), so on Windows the sidecar mode
  is the natural default.
- Signing: Authenticode through `bundle.windows.signCommand` (works with Azure
  Trusted Signing, a cloud HSM, or `signtool`). An EV certificate is no longer
  needed with Azure Trusted Signing; SmartScreen reputation still builds over
  time (not verified here).

### 3.2 Linux (WebKitGTK)

- Engine: WebKitGTK 4.1 (`webkit2gtk-4.1`), the weakest link. It is the same
  WebKit as WKWebView in theory, but distribution packages lag, GPU
  compositing is often disabled by default on NVIDIA/Wayland setups
  (`WEBKIT_DISABLE_COMPOSITING_MODE`, `WEBKIT_DISABLE_DMABUF_RENDERER` are
  common workarounds), and media codecs depend on the distro's GStreamer
  plugins.
  - §2.7 measured it in a container (software rendering): chat streaming and
    scrolling pass, with WebKitGTK 1.3–2× slower than Chromium per operation.
  - A heavy animated design falls to 4–14 fps, where Chromium keeps 60 fps
    on the same CPU.
  - A real Linux machine with a GPU is still the open item (§4).
- Packages: `.deb`, `.rpm`, AppImage from `tauri build`; Flatpak via a
  manifest on the GNOME runtime (which ships a current WebKitGTK, a reason to
  prefer Flatpak); the repo already has `flatpak/` and `snap/` packaging for
  the daemon that could take the shell.
- CI: needs `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`,
  `librsvg2-dev` on the runner, which is why the shell is outside the main
  workspace.
- Escape hatch: Tauri 3 (alpha) makes the runtime pluggable with a CEF
  (Chromium) runtime. Shipping Chromium on Linux only would remove the
  WebKitGTK risk at the cost of ~100 MB. Not stable yet.

### 3.3 macOS (WKWebView)

Measured in §2. Packaging: `.app` and `.dmg` from `tauri build`; notarization
built into the bundler (`APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` or an App
Store Connect API key). The daemon binary goes in `Contents/MacOS/` as an
external bin; it needs its own entitlements for the sandbox features
(`apps/desktop/daemon.entitlements` already exists and can be reused).

### 3.4 Android and iOS (Tauri mobile)

What works today in Tauri 2:

- The same Rust core and web UI build for Android (system WebView, Chromium
  based, updated through the Play Store) and iOS (WKWebView). `tauri android
  init/dev/build`, `tauri ios init/dev/build` generate Gradle and Xcode
  projects.
- Official plugins with mobile support: notification (local notifications),
  deep-link, barcode-scanner, biometric, haptics, geolocation, NFC, clipboard,
  dialog, fs, http, opener, store, os.
- Writing a missing plugin is Kotlin/Swift code behind a Rust facade; the
  plugin system is designed for it.

What is missing or third-party:

| Need | Status |
|---|---|
| Remote push (APNs/FCM) | No official plugin. Community: `tauri-plugin-notifications` (push behind a feature flag), `tauri-plugin-remote-push`, `tauri-plugin-mobile-push`. The gateway already has a web-push stack (`push.rs`); mobile needs APNs/FCM tokens registered with it. |
| Background execution | Nothing official. iOS allows only short background tasks and push-triggered work anyway; Android needs a foreground service (custom Kotlin plugin). An agent run must therefore live on a daemon, never on the phone. |
| Share sheet (send to Antre / share from Antre) | Sharing out: community plugins (`tauri-plugin-share`-style). Receiving shares (share extension / intent filter): needs native target code (an iOS Share Extension is a separate binary target) — custom work. |
| Widgets, Live Activities, watch apps | Native only. |
| Updater | Desktop only; mobile updates go through the stores (web assets can still be reloaded from the daemon, see below). |

Architecture consequence: on mobile the shell is a **remote client only**
(no embedded or sidecar daemon: no background, no Docker sandboxes, battery).
It loads the web UI from the user's daemon (home server, VPS, or desktop
running Antre) over HTTPS/Tailscale. That is exactly the "remote" mode of the
spike, so the remote-auth gap above becomes a priority for mobile.

The WKWebView 60 fps cap measured on macOS (§2) is also documented for iOS
WKWebView on ProMotion iPhones (WebKit bug 294338, open): a Tauri iOS app will
scroll and animate at 60 Hz where a native SwiftUI list runs at 120 Hz.

### 3.5 Updater and signing

- Desktop: `tauri-plugin-updater` with a static JSON manifest (GitHub
  Releases works) and minisign-style signatures (`TAURI_SIGNING_PRIVATE_KEY`).
  Updates the shell and the bundled sidecar together, which keeps shell and
  daemon versions in lockstep. Combine with the existing release pipeline
  (Sigstore + GPG for artifacts) by adding the updater signature as one more
  artifact.
- The web UI itself can update without a shell update when it is served by
  the daemon (sidecar and remote modes): the shell is only a frame. This is a
  strong argument for keeping the UI served by the daemon rather than bundled
  into the shell.
- macOS: Developer ID + notarization (bundler built-in). Windows: Authenticode
  (`signCommand`). Linux: unsigned AppImage/deb, or Flatpak (Flathub signs).
  Mobile: store signing (Play App Signing, App Store).

### 3.6 What happens to the existing apps

- **`apps/desktop` (gpui).** Overlaps the Tauri shell completely on desktop
  and cannot render artifacts, canvas iframes, MCP Apps or HTML mods without
  embedding a WebView anyway. Recommendation: freeze it (keep building, no new
  features), and retire it once the Tauri shell reaches parity on desktop. Its
  useful parts move: daemon discovery and service install
  (`local_daemon.rs`), SSH/mDNS discovery (`moltis-client` features) can be
  called from the Tauri shell's Rust side.
- **`apps/macos` (SwiftUI, embeds the daemon via `swift-bridge`).** Replaced
  on macOS by the Tauri shell with a sidecar. Freeze, then retire.
- **`apps/ios` (SwiftUI companion over GraphQL).** Keep until Tauri iOS covers
  push and share, because those are exactly what Tauri mobile lacks. Two
  options later: (a) retire it for a Tauri iOS remote client plus small native
  plugins (push, share extension), (b) keep a thin native iOS app that hosts
  the web UI in its own WKWebView and does push/share natively. (b) is close to
  what a Tauri iOS app is, with less tooling risk; decide after a Tauri iOS
  prototype.

## Follow-up: what `claude/desktop-fixes` changed

| Gap found by the spike | Status |
|---|---|
| Docker probe blocks startup for minutes | Fixed: every runtime probe is killed after 5 s and detection falls through to the next backend. |
| Trusted-network proxy on fixed `0.0.0.0:18791` | Fixed: ephemeral port by default (`[tools.exec.sandbox] proxy_port` pins it); backends get the bound port. Still bound on all interfaces (containers reach it through the host gateway). |
| Silent import of `~/.codex/auth.json` | Fixed: read only with `[providers] external_credentials = ["codex-cli"]`, or copied by the explicit `providers.import_external` RPC. |
| `ulimit -u 256` in restricted-host | Fixed: process cap only when `pids_max` is set. |
| Remote auth (API key vs cookie) | Fixed: `POST /api/auth/session-ticket` + redeem URL; the shell opens the UI signed in. |
| Shell product basics | Single instance, tray (close hides), native notifications for daemon incidents, sidecar bundling script. |
| Canvas iframe throttling, chat notifications | Open questions in `plans/questions.md`. |

## 4. Recommendation

**Go for "web UI + thin Tauri 2 shell" on desktop**, as the single product
surface, with the daemon supervised as a sidecar (not embedded). **Defer the
mobile decision** until WebKitGTK and Android WebView are measured and a
Tauri iOS remote-client prototype exists.

Why go:

- On macOS the system WebView is good enough for the chat: 400 tok/s
  streaming with no dropped frames, 2,000 messages in 0.3 s, smooth scroll,
  less memory than Chromium. The bottlenecks found (per-delta full re-render,
  no virtualisation) are UI algorithms that would hurt in any engine,
  including gpui if it re-parsed the same way.
- Every surface that matters for Antre (canvas, artifacts, MCP Apps, mods)
  is HTML and works in the shell with no extra code; gpui would need a
  WebView anyway.
- The shell stays tiny and replaceable: ~770 lines, no IPC exposed to daemon
  pages, the UI is still served by the daemon (browser, PWA and shell share
  one build and update without a shell release).

Why sidecar, not embedded:

- Same binary as the server/CLI install, separate crash domain, no second
  copy of the gateway's dependency patches, fast shell builds. Embedding buys
  one process less at the cost of all of that.

Conditions before committing (in order):

1. **Measure WebKitGTK on real hardware, and Android WebView,** with the
   same bench.
   - Done so far: WebKitGTK in a container, in software rendering only
     (§2.7). The chat clears the bar there: 400 tok/s at 54 fps, scroll at
     58 fps.
   - Left for WebKitGTK: a mid-range Linux laptop with a GPU, the distro's
     WebKitGTK, no compositing workarounds, through `antre-shell --bench`.
     The procedure is in `apps/tauri/bench/webkitgtk/README.md`.
   - Left for Android: load `dist/` in a WebView or in Chrome for Android.
   - No-go trigger: scroll or 400 tok/s streaming below ~45 fps on that
     laptop.
   - Heavy animated canvases already fall well below that bar in software,
     so check them there too. If they stay slow, the fallback for canvases
     is to tone down or freeze heavy effects on WebKitGTK, not to drop the
     shell.
   - Fallback if the chat itself fails: Tauri 3's CEF runtime on Linux only,
     or Electron on Linux only.
2. **Fix the daemon's cold start** (time-box the Docker probe, listen first,
   initialise sandboxes in the background) and make fixed
   ports (`18791` proxy) configurable or ephemeral, so a sidecar can coexist
   with a user service, and make the daemon exit when its parent shell dies. Independent of the shell, but the shell makes the
   wait visible.
3. **Remote auth**: an API-key or pairing-code → session-cookie exchange in
   the gateway (needed for remote desktop mode and for every mobile client).
4. **Decide the canvas iframe origin model** knowing WebKit throttles
   opaque-origin frames until the user interacts with them.
   - Measured: ~20 fps in WKWebView and ~25 fps in WebKitGTK.
   - WebKitGTK reaches 49 fps after one real click (§2.7).
   - iOS WKWebView very likely behaves the same and caps at 60 Hz.
5. **Incremental Markdown rendering** for streaming (re-parse only the open
   block) before supporting very long replies.

Risks:

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| WebKitGTK too slow or glitchy on common Linux setups | Medium (chat: low; heavy canvases: high) | High (one of five platforms) | Chat passes in software (§2.7); measure on real GPUs; lighter canvas effects on WebKitGTK; Flatpak on the GNOME runtime for a current WebKitGTK; CEF runtime (Tauri 3) or Electron on Linux as fallback. |
| 60 Hz cap in WKWebView (macOS, iOS) | Certain | Low–medium (feel, not function) | Accept; revisit when WebKit lifts it; a private-preference plugin is possible but risky for the App Store. |
| Opaque-origin iframe throttling (WebKit) | Certain | Medium for animated canvases | A real click lifts it (verified in WebKitGTK, §2.7): accept "smooth after first touch", or test whether a dedicated canvas origin is treated better. |
| Tauri mobile gaps (push, background, share extension) | Certain | High for a mobile-first experience | Community push plugins; native Swift/Kotlin plugins; keep the Swift iOS app until parity; mobile is a remote client only. |
| Tauri 3 migration (alpha now) | Medium | Low | The shell is small; Tauri 3 mostly changes runtime selection. |
| Daemon assumptions about owning the machine (fixed ports, cold start, credential import) | Certain | Medium | Gateway fixes listed above; isolated sidecar data dir in the shell. |
| Divergent engines (WebView2 vs WebKit) cause UI bugs | Medium | Medium | Playwright E2E already exists for Chromium; add a WebKit project (Playwright WebKit is close to WKWebView). |

What this spike did not do: package or sign anything, run on Windows,
Linux hardware (WebKitGTK only ran in a container, §2.7), Android or iOS,
measure a release build of the shell, test real LLM
streaming end to end through the shell (the bench uses the UI's renderer on
synthetic deltas).

