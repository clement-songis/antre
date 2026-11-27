# Tauri 2 shell spike (Antre)

Date: 2026-11-27. Branch: `claude/tauri-spike-957729`. Code: `apps/tauri/`.

Question: can the existing web UI, hosted by a thin Tauri 2 shell around the
Rust daemon, be the single product surface on Windows, Linux, macOS, Android
and iOS? Context: `compare-oss-landscape.md` §6 recommended web UI + Tauri,
conditional on this spike.

<!-- MEASUREMENTS -->

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
| embedded | `antre-shell --embedded` (built with `--features embedded`) | Calls `moltis_httpd::server::start_gateway` on a tokio task inside the shell. |
| remote | `ANTRE_API_KEY=… antre-shell --remote https://host:port` | Checks `/health`, checks the key with `GET /api/auth/status` (Bearer), then loads the URL. Plain `http` is refused except to loopback. The key is read from the environment, never from argv (visible in `ps`). |
| bench | `antre-shell --bench stream|transcript|canvas [--out FILE]` | Loads a bundled bench page, writes its JSON report, exits. |

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
- **Embedded mode** works from the code side (`start_gateway` is public) but
  costs: the shell workspace must copy the root `[patch.crates-io]` entries
  (sqlx fork, vendored ACP) to resolve at all, and compiling the gateway into
  the shell is a full gateway build in a second target directory
  (see §2.5). `start_gateway` also assumes it owns the process (global config
  dir, signal handling, tracing set up by the CLI). The sidecar is simpler and
  keeps the daemon's crash domain separate from the UI.
- **Graceful stop.** The sidecar is stopped with SIGKILL (`tokio` child kill);
  SQLite in WAL mode survives that, but a SIGTERM path (via a safe signal crate)
  should come before shipping.

## 3. Platforms

### 3.1 Windows (WebView2)

- Engine: Edge WebView2 (Chromium), the same engine family as the Chrome
  column in §2, so rendering and performance should track Chrome closely,
  including 120 Hz+ `requestAnimationFrame` (Chromium does not cap it).
- Runtime: preinstalled on Windows 11 and current Windows 10. Tauri's bundler
  can embed the bootstrapper (`webviewInstallMode: downloadBootstrapper`, the
  default), embed the offline installer (+~127 MB), or use a fixed runtime
  shipped with the app (+~180 MB, full control over the version).
- Packages: NSIS `.exe` and WiX `.msi` from `tauri build` on a Windows runner
  (cross-compiling from macOS/Linux works for NSIS only, experimental).
- Sidecar: `moltis.exe` declared as `bundle.externalBin` (target-triple
  suffixed), placed next to the shell exe; `find_moltis` already looks there.
  The daemon has no Windows service manager integration today
  (`moltis service` targets launchd/systemd), so on Windows the sidecar mode
  is the natural default.
- Signing: Authenticode through `bundle.windows.signCommand` (works with Azure
  Trusted Signing, a cloud HSM, or `signtool`). An EV certificate is no longer
  needed to avoid SmartScreen warnings with Azure Trusted Signing; reputation
  still builds over time.

### 3.2 Linux (WebKitGTK)

- Engine: WebKitGTK 4.1 (`webkit2gtk-4.1`), the weakest link. It is the same
  WebKit as WKWebView in theory, but distribution packages lag, GPU
  compositing is often disabled by default on NVIDIA/Wayland setups
  (`WEBKIT_DISABLE_COMPOSITING_MODE`, `WEBKIT_DISABLE_DMABUF_RENDERER` are
  common workarounds), and media codecs depend on the distro's GStreamer
  plugins. Expect lower frame rates than macOS on the same content; this spike
  could not measure it (no Linux machine) and that measurement is the main open
  item (§4).
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

<!-- RECOMMENDATION -->
