<div align="center">

<a href="https://moltis.org"><img src="https://raw.githubusercontent.com/antre-ai/antre/main/website/favicon.svg" alt="Antre" width="64"></a>

# Antre — A secure persistent personal agent server in Rust

One binary — sandboxed, secure, yours.

[![CI](https://github.com/antre-ai/antre/actions/workflows/ci.yml/badge.svg)](https://github.com/antre-ai/antre/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/antre-ai/antre/graph/badge.svg)](https://codecov.io/gh/antre-ai/antre)
[![CodSpeed](https://img.shields.io/endpoint?url=https://codspeed.io/badge.json&style=flat&label=CodSpeed)](https://codspeed.io/antre-ai/antre)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.91%2B-orange.svg)](https://www.rust-lang.org)
[![Discord](https://img.shields.io/discord/1469505370169933837?color=5865F2&label=Discord&logo=discord&logoColor=white)](https://discord.gg/XnmrepsXp5)

[Installation](#installation) • [Comparison](#comparison) • [Architecture](#architecture--crate-map) • [Security](#security) • [Features](#features) • [How It Works](#how-it-works) • [Contributing](CONTRIBUTING.md)

</div>

---

> **Antre is a fork of [Moltis](https://github.com/moltis-org/moltis) by Fabien Penso.** <!-- rebrand:keep -->
> The lineage runs OpenClaw (the claw), Moltis (the molt), Antre (the lair: a self-hosted home for your agent). <!-- rebrand:keep -->
> Existing Moltis installs migrate on first start, see [Migrating from Moltis](docs/src/legacy-migration.md). <!-- rebrand:keep -->
> Antre has published nothing yet: the install script below still comes from moltis.org and the `antre-ai` packages and images do not exist yet, so build from source or use the Nix flake (`nix run github:clement-songis/antre`). <!-- rebrand:keep -->

**Secure by design** — Your keys never leave your machine. Every command runs in a sandboxed container, never on your host.

**Your hardware** — Runs on a Mac Mini, a Raspberry Pi, or any server you own. One Rust binary, no Node.js, no npm, no runtime.

**Full-featured** — Voice, memory, cross-session recall, automatic edit checkpoints, scheduling, Telegram, Signal, Discord, browser automation, MCP servers, SSH or node-backed remote exec, managed deploy keys with host pinning in the web UI, a live Settings → Tools inventory, Cursor-compatible project context, and context-file threat scanning — all built-in. No plugin marketplace to get supply-chain attacked through.

**Auditable** — The agent runner and model interface fit in ~7.5K lines, with providers in ~19K more. The Rust workspace is ~270K lines across 59 modular crates you can audit independently, with 470+ Rust files containing tests. Unsafe code is isolated to FFI and precompiled runtime boundaries, not the core agent loop.

## Installation

```bash
# One-liner install script (macOS / Linux)
curl -fsSL https://www.moltis.org/install.sh | sh

# macOS / Linux via Homebrew
brew install antre-ai/tap/antre

# Docker (multi-arch: amd64/arm64)
docker pull ghcr.io/antre-ai/antre:latest

# Or build from source
cargo install antre --git https://github.com/antre-ai/antre
```

## Comparison

| | OpenClaw | Hermes Agent | **Antre** |
|---|---|---|---|
| Primary stack | TypeScript + Swift/Kotlin companion apps | Python + TypeScript TUI/web surfaces | **Rust** |
| Runtime | Node.js + npm/pnpm/bun | Python + uv/pip, optional Node UI pieces | **Single Rust binary** |
| Local checkout size\* | ~1.1M app LoC | ~152K app LoC | **~270K Rust LoC** |
| Architecture | Broad gateway, channel, node, and app ecosystem | CLI/gateway agent with learning loop and research tooling | **Persistent personal agent server with modular crates** |
| Crates/modules | npm packages, extensions, apps | Python packages, plugins, tools, TUI | **59 Rust workspace crates** |
| Sandbox/backends | App-level permissions, browser/node tools | Local, Docker, SSH, Daytona, Singularity, Modal | **Docker/Podman + Apple Container + WASM + Seatbelt/bubblewrap** |
| Auth/access | Pairing and local gateway controls | CLI and messaging gateway setup | **Password + Passkey + API keys + Vault** |
| Voice I/O | Voice wake and talk modes | Voice memo transcription | **Built-in STT + TTS providers** |
| MCP | Plugin/integration support | MCP integration | **stdio + HTTP/SSE, resources, prompts, sampling, elicitation** |
| Skills | Bundled, managed, and workspace skills | Self-improving skills and Skills Hub support | **Bundled/workspace skills + autonomous improvement + OpenClaw import** |
| Memory/RAG | Plugin-backed memory and context engine | Agent-curated memory, session search, user modeling | **SQLite + FTS + vector memory** |

\* LoC measured with `tokei`, excluding `node_modules`, generated build output, `dist`, and `target`.

> [Full comparison in the docs →](https://docs.moltis.org/comparison.html)

## Architecture — Crate Map

Current Rust workspace: ~270K LoC across 59 crates. The table below groups the main crates by role so the architecture stays scannable.

**Core runtime**:

| Crate | LoC | Role |
|-------|-----|------|
| `antre-gateway` | 37.4K | HTTP/WS server, RPC, auth, startup wiring |
| `antre-tools` | 37.0K | Tool execution, sandboxing, WASM tools |
| `antre-providers` | 18.9K | LLM provider implementations |
| `antre-agents` | 14.5K | Agent loop, streaming, prompt assembly |
| `antre-chat` | 14.2K | Chat engine, agent orchestration |
| `antre-config` | 10.3K | Configuration, validation |
| `antre-httpd` | 9.9K | HTTP server primitives and middleware |
| `antre` (CLI) | 4.7K | Entry point, CLI commands |
| `antre-sessions` | 3.5K | Session persistence |
| `antre-common` | 1.5K | Shared utilities |
| `antre-service-traits` | 1.2K | Shared service interfaces |
| `antre-protocol` | 0.7K | Wire protocol types |

**Feature and integration crates**:

| Category | Crates | Combined LoC |
|----------|--------|-------------|
| Channels | `antre-telegram`, `antre-whatsapp`, `antre-signal`, `antre-discord`, `antre-msteams`, `antre-matrix`, `antre-slack`, `antre-nostr`, `antre-channels` | 34.0K |
| Web and APIs | `antre-web`, `antre-graphql`, `antre-webhooks` | 10.8K |
| Extensibility | `antre-mcp`, `antre-mcp-agent-bridge`, `antre-skills`, `antre-plugins` | 11.5K |
| Memory and context | `antre-memory`, `antre-qmd`, `antre-code-index`, `antre-projects` | 11.7K |
| Voice and browser | `antre-voice`, `antre-browser` | 9.2K |
| Auth and security | `antre-auth`, `antre-oauth`, `antre-vault`, `antre-secret-store`, `antre-network-filter`, `antre-tls` | 8.5K |
| Scheduling and automation | `antre-cron`, `antre-caldav`, `antre-auto-reply` | 4.7K |
| Setup and import | `antre-provider-setup`, `antre-openclaw-import`, `antre-onboarding` | 11.7K |
| Native and node hosts | `antre-swift-bridge`, `antre-node-host`, `antre-courier` | 5.7K |
| WASM tools | `antre-wasm-precompile`, `antre-wasm-calc`, `antre-wasm-web-fetch`, `antre-wasm-web-search` | 1.4K |
| Mods | `antre-mods` | 5.7K |
| Supporting crates | `antre-media`, `antre-metrics`, `antre-tailscale`, `antre-routing`, `antre-canvas`, `antre-schema-export`, `benchmarks` | 2.1K |

Use `--no-default-features --features lightweight` for constrained devices (Raspberry Pi, etc.).

## Security

- **Small unsafe surface** — core agent/gateway code stays safe Rust; unsafe is isolated to Swift FFI, local model wrappers, and precompiled WASM boundaries
- **Sandboxed execution** — Docker + Apple Container, per-session isolation, and an OS-level sandbox (macOS Seatbelt, Linux bubblewrap) when no container runtime is installed
- **Secret handling** — `secrecy::Secret`, zeroed on drop, redacted from tool output
- **Authentication** — password + passkey (WebAuthn), rate-limited, per-IP throttle
- **SSRF protection** — DNS-resolved, blocks loopback/private/link-local
- **Origin validation** — rejects cross-origin WebSocket upgrades
- **Hook gating** — `BeforeToolCall` hooks can inspect/block any tool invocation
- **Supply chain integrity** — [artifact attestations](https://github.com/antre-ai/antre/attestations), Sigstore keyless signing, GPG signing (YubiKey), SHA-256/SHA-512 checksums

See [Security Architecture](https://docs.moltis.org/security.html) for details.
Verify releases with `gh attestation verify <artifact> -R antre-ai/antre` or see [Release Verification](https://docs.moltis.org/release-verification.html).

## Features

- **Reasoning Defaults** - Configurable default thinking effort for model-less and new main sessions, with explicit per-session Off or level overrides
- **AI Gateway** — Multi-provider LLM support (OpenAI Codex, GitHub Copilot, Local, 40+ API-key providers including Together, Hugging Face, NVIDIA, Vercel, Cloudflare, Amazon Bedrock and Azure OpenAI), image generation through Codex or an OpenAI/Gemini/xAI key, streaming responses, agent loop with sub-agent delegation, session modes, parallel tool execution
- **Model Catalog** — One catalog of model names, context windows, capabilities, prices and retirement dates merged from overrides, live `/models`, a background-refreshed models.dev copy and a bundled snapshot; pickers show readable names and context sizes, compaction budgets from the current model's real window, and features pick models by role instead of hard-coded ids
- **Communication** — Web UI with command-palette agent prompts and Markdown copy/export, per-tool cards with inline diffs and a live task checklist, keyboard command approvals with persisted "always allow", rewind (conversation and/or files), regenerate and edit-as-fork, an editable message queue, `@file` mentions, and Mermaid/KaTeX rendering, Telegram, Signal, Microsoft Teams, Discord, Slack with native live tool cards, Nostr DMs + NIP-29 group chat (incl. Block's Buzz channels), API access, voice I/O (8 TTS + 8 STT providers with ordered fallback chains, including Gemini transcription, live transcription with OpenAI Realtime and Gemini Live) with voice-note transcription on Telegram, Signal, Discord, Matrix and WhatsApp, mobile PWA with push notifications
- **Reliable Delivery** — Channel replies go through a persisted outbox with retries, per-chat ordering and visible failures; one run per session, with sync callers, quota-parked runs and background sub-agent results waiting their turn instead of racing
- **Desktop App** — Native Rust/gpui client that attaches to a running daemon locally, over HTTPS, or through an SSH tunnel; keychain-stored scoped API keys, certificate pinning, streaming native markdown, approvals with system notifications
- **Memory & Recall** — Per-agent memory workspaces, embeddings-powered long-term memory, hybrid vector + full-text search with optional reranking (chat model, Cohere, Jina, Voyage or a local `/rerank`), session persistence with auto-compaction, cross-session recall, Cursor-compatible project context, context-file safety scanning
- **Managed Files** - Authenticated nested file management in Settings, with an agent-advertised host path and independent read-only-by-default mounts for Docker, Podman, and Apple Container sandboxes
- **Design Canvas** — Agent-built HTML designs in a live preview pane (versions and revert, phone/tablet/desktop viewports, element picking into the composer, comment pins sent to the agent), MCP Apps and HTML snippets rendered inline, starter design systems as skills, a sandboxed render origin (CSP sandbox, no network, vendored scripts only), single-file HTML/ZIP/PNG/PDF export and private-by-default publishing
- **Safer Agent Editing** — Automatic checkpoints before built-in skill and memory mutations, restore tooling, session branching
- **Learning Loop** — Opt-in background review that turns finished conversations into skill and memory patches under strict anti-hoarding rules, a curator that ages and archives unused skills, write-approval staging and an append-only ledger with single-change rollback (`learning.*` RPC)
- **Extensibility** — MCP servers (stdio + HTTP/SSE) with resources, prompts, roots, sampling with spend limits, elicitation, per-server trust tiers and deferred tool schemas, skill system, 15 lifecycle hook events with circuit breaker (Claude Code hook format accepted), destructive command guard
- **Mods** — sandboxed WASM mods that add tools and hooks, with capability manifests, a consent hash re-asked when an update widens access, and content-pinned shell hooks
- **Agent Client Protocol** — drives external ACP agents (`codex-acp`, `claude-agent-acp`, Cursor), and serves Antre to ACP clients over stdio via `antre acp`
- **First Run** — Opens onboarding in your browser with the setup code pre-filled, serves certificate-free `http://localhost`, offers once to trust a name-constrained local CA for LAN HTTPS, and pins the web UI URL below the terminal logs (with an `ssh -L` hint over SSH)
- **Security** — Encryption-at-rest vault (XChaCha20-Poly1305 + Argon2id), password + passkey + API key auth, sandbox isolation, SSRF/CSWSH protection
- **Operations** — Cron scheduling, durable local CalDAV, Gmail, Himalaya email, and reusable Slack/Discord/Matrix/Teams message datasets, provider-specific read-only agent tools, fast full-text search, JSONL/Markdown projections, agent instrumentation (Langfuse for LLM traces, OTLP for Grafana/Datadog), Prometheus metrics, cloud deploy (Fly.io, DigitalOcean), Tailscale integration, managed SSH deploy keys, host-pinned remote targets, live tool inventory in Settings, and CLI/web remote-exec doctor flows

## How It Works

Antre is a **local-first persistent agent server** — a single Rust binary that
sits between you and multiple LLM providers, keeps durable session state, and
can meet you across channels without handing your data to a cloud relay.

```
┌─────────────┐  ┌─────────────┐  ┌─────────────┐
│   Web UI    │  │  Telegram   │  │  Discord    │
└──────┬──────┘  └──────┬──────┘  └──────┬──────┘
       │                │                │
       └────────┬───────┴────────┬───────┘
                │   WebSocket    │
                ▼                ▼
        ┌─────────────────────────────────┐
        │          Gateway Server         │
        │   (Axum · HTTP · WS · Auth)     │
        ├─────────────────────────────────┤
        │        Chat Service             │
        │  ┌───────────┐ ┌─────────────┐  │
        │  │   Agent   │ │    Tool     │  │
        │  │   Runner  │◄┤   Registry  │  │
        │  └─────┬─────┘ └─────────────┘  │
        │        │                        │
        │  ┌─────▼─────────────────────┐  │
        │  │    Provider Registry      │  │
        │  │  Multiple providers       │  │
        │  │  (Codex · Copilot · Local)│  │
        │  └───────────────────────────┘  │
        ├─────────────────────────────────┤
        │  Sessions  │ Memory  │  Hooks   │
        │  (JSONL)   │ (SQLite)│ (events) │
        └─────────────────────────────────┘
                       │
               ┌───────▼───────┐
               │    Sandbox    │
               │ Docker/Apple  │
               │  Container    │
               └───────────────┘
```

See [Quickstart](https://docs.moltis.org/quickstart.html) for gateway startup, message flow, sessions, and memory details.

## Getting Started

### Build & Run

Requires [just](https://github.com/casey/just) (command runner) and Node.js (for Tailwind CSS).

```bash
git clone https://github.com/antre-ai/antre.git
cd antre
just build-css                  # Build Tailwind CSS for the web UI
just build-release              # Build in release mode
cargo run --release --bin antre
```

For a full release build including WASM sandbox tools:

```bash
just build-release-with-wasm    # Builds WASM artifacts + release binary
cargo run --release --bin antre
```

Open `https://antre.localhost:3000`. On first run, a setup code is printed to
the terminal — enter it in the web UI to set your password or register a passkey.

Optional flags: `--config-dir /path/to/config --data-dir /path/to/data`

### Docker

```bash
# Docker / OrbStack
docker run -d \
  --name antre \
  -p 13131:13131 \
  -p 13132:13132 \
  -p 1455:1455 \
  -v antre-config:/home/antre/.config/antre \
  -v antre-data:/home/antre/.antre \
  -v /var/run/docker.sock:/var/run/docker.sock \
  ghcr.io/antre-ai/antre:latest
```

Open `https://localhost:13131` and complete the setup. For unattended Docker
deployments, set `ANTRE_PASSWORD`, `ANTRE_PROVIDER`, and `ANTRE_API_KEY`
before first boot to skip the setup wizard. See [Docker docs](https://docs.moltis.org/docker.html)
for Podman, OrbStack, TLS trust, and persistence details.

### Cloud Deployment

| Provider | Deploy |
|----------|--------|
| DigitalOcean | [![Deploy to DO](https://www.deploytodo.com/do-btn-blue.svg)](https://cloud.digitalocean.com/apps/new?repo=https://github.com/antre-ai/antre/tree/main) |

**Fly.io** (CLI):

```bash
fly launch --image ghcr.io/antre-ai/antre:latest
fly secrets set ANTRE_PASSWORD="your-password"
```

All cloud configs use `--no-tls` because the provider handles TLS termination.
See [Cloud Deploy docs](https://docs.moltis.org/cloud-deploy.html) for details.

## License

MIT
