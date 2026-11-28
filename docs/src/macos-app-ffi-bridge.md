# macOS App FFI Bridge (Work in Progress)

```admonish warning
This macOS app integration is not finished yet. It is currently being built.
```

This page documents how `apps/macos` currently bridges Swift to Rust through FFI.

## Runtime Architecture

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ Antre.app (single macOS process)                                        │
│                                                                          │
│  SwiftUI Views                                                           │
│  (ContentView, OnboardingView, SettingsView, ...)                        │
│                    │                                                     │
│                    ▼                                                     │
│  State stores                                                            │
│  (ChatStore, ProviderStore, LogStore)                                    │
│                    │                                                     │
│                    ▼                                                     │
│  Swift FFI facade: AntreClient.swift                                    │
│  - encodes requests to JSON                                              │
│  - calls C symbols from `antre_bridge.h`                                │
│  - decodes JSON responses / bridge errors                                │
└────────────────────┬─────────────────────────────────────────────────────┘
                     │
                     │ C ABI (`antre_*`)
                     ▼
┌──────────────────────────────────────────────────────────────────────────┐
│ Rust bridge static library: `libantre_bridge.a`                         │
│ crate: `crates/swift-bridge`                                             │
│                                                                          │
│  `extern "C"` exports                                                    │
│  (chat, streaming, providers, sessions, httpd, version, shutdown, ...)   │
│                    │                                                     │
│                    ▼                                                     │
│  Rust bridge internals                                                   │
│  - pointer/UTF-8 + JSON validation                                       │
│  - panic boundary (`catch_unwind`)                                       │
│  - tokio runtime + provider registry + session storage                   │
└────────────────────┬─────────────────────────────────────────────────────┘
                     │
                     ▼
┌──────────────────────────────────────────────────────────────────────────┐
│ Reused Antre crates                                                     │
│ (`antre-providers`, `antre-sessions`, `antre-gateway`, etc.)          │
└──────────────────────────────────────────────────────────────────────────┘

Reverse direction callbacks:
- Rust logs: `antre_set_log_callback(...)` -> Swift `LogStore`
- Rust streaming events: `antre_*_chat_stream(...)` callback -> Swift closures
- Rust session events: `antre_set_session_event_callback(...)` -> Swift `ChatStore`
```

## Build and Link Pipeline

```text
`just swift-build-rust`
        │
        ▼
scripts/build-swift-bridge.sh
  1) cargo build -p antre-swift-bridge --target x86_64-apple-darwin
  2) cargo build -p antre-swift-bridge --target aarch64-apple-darwin
  3) lipo -create -> universal `libantre_bridge.a`
  4) cbindgen -> `antre_bridge.h`
  5) copy both artifacts into `apps/macos/Generated/`
        │
        ▼
`just swift-generate` (xcodegen from `apps/macos/project.yml`)
        │
        ▼
Xcode build
  - header search path: `apps/macos/Generated`
  - library search path: `apps/macos/Generated`
  - links `-lantre_bridge`
  - uses `Sources/Bridging-Header.h` -> includes `antre_bridge.h`
```

## Main FFI Touchpoints

- Swift header import: `apps/macos/Sources/Bridging-Header.h`
- Swift facade: `apps/macos/Sources/AntreClient.swift`
- Rust exports: `crates/swift-bridge/src/lib.rs`
- Artifact builder: `scripts/build-swift-bridge.sh`
- Xcode linking config: `apps/macos/project.yml`

## Real-time Session Sync

Sessions created in the macOS app appear in the web UI (and vice versa) in
real time thanks to a shared `tokio::sync::broadcast` channel — the
`SessionEventBus`.

```text
┌──────────────┐  publish   ┌─────────────────┐  subscribe  ┌────────────────┐
│ Bridge FFI   │ ────────→ │ SessionEventBus  │ ────────→  │ FFI callback   │→ macOS app
│ (macOS app)  │           │ (broadcast chan)  │            │ (bridge lib.rs)│
└──────────────┘           └─────────────────┘            └────────────────┘
                                  ↑
┌──────────────┐  publish         │
│ Gateway RPCs │ ─────────────────┘
│ (sessions.*) │        (also broadcasts to WS clients directly)
└──────────────┘
```

When HTTPD is enabled, the bridge passes its bus instance to `prepare_gateway()`
so both share the same channel. Events:

| Kind      | Trigger                                    |
|-----------|--------------------------------------------|
| `created` | `sessions.resolve` (new), `sessions.fork`, bridge `antre_create_session` |
| `patched` | `sessions.patch`                           |
| `deleted` | `sessions.delete`                          |

Swift receives events via `antre_set_session_event_callback` — each event is a
JSON object `{"kind":"created","sessionKey":"..."}` dispatched to
`ChatStore.handleSessionEvent()` on the main thread.

## Current Status

The bridge is already functional for core flows (version, chat, streaming, providers, sessions, embedded httpd), but this is still a POC-stage macOS app and the integration surface is still evolving.
