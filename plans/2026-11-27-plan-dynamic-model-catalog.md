# Dynamic model catalog, context sizes and quota-aware retries

Status: proposed (replaces roadmap phase 1 "refresh fallback lists").
Background research: `plans/research/providers-inventory.md` §1 and §5.

## Problem

- Model lists are hard-coded fallbacks (`crates/providers/src/model_catalogs.rs`,
  `*/catalog.rs`). They go stale every few weeks: about 12 of the last 80
  provider commits were manual catalog fixes, and several defaults retire in
  December 2026 (Gemini 2.5 on 12-01, OpenAI gpt-5 on 12-11, Gemini TTS on
  12-18, Kimi K2 end of December).
- Live `/models` discovery exists for most providers but returns ids only and
  is kept in memory, so capabilities (context window, tools, vision, price,
  retirement) are guessed from the id (`model_capabilities.rs`).
- Features call hard-coded ids (image generation, TTS/STT defaults,
  Perplexity search) and break on every retirement.
- Compaction sizes its budget from `LlmProvider::context_window()`, which
  falls back to a 200 K guess (`crates/chat/src/compaction_run/runner.rs`).
  Small-context models then fail to compact.
- Quota and rate-limit errors are classified by substring matching on the
  error message (`crates/agents/src/provider_chain.rs`), and the only reaction
  is to rotate providers or trip a 60 s circuit breaker. Nothing waits for the
  provider's actual reset time.
- Pickers show slugs (`glm-4.6-air-0925`) instead of names.

## Goals

1. One catalog, updated without releases, that every feature reads.
2. Correct context window and max output per model, used by compaction and
   the context meter.
3. Human-readable model names everywhere.
4. Typed rate-limit and quota errors that carry the reset instant, and a run
   that resumes automatically after the reset.
5. Custom providers declared statically in config with the same schema, so a
   Nix module (or any config generator) can describe models fully.

## Model catalog

### Schema

`ModelInfo` (concrete types, no `serde_json::Value`):

| Field | Notes |
|---|---|
| `provider`, `id` | `id` is the wire id sent to the API |
| `display_name` | Human name, see "Names" below |
| `family`, `release_date`, `knowledge_cutoff` | Family groups versions (`claude-sonnet`, `glm`) |
| `kind` | `Chat`, `Embedding`, `Rerank`, `Image`, `Video`, `Tts`, `Stt`, `Realtime` |
| `context_window`, `max_output` | Tokens |
| `input_modalities`, `output_modalities` | Enum sets |
| `tools`, `structured_output`, `caching` | Booleans |
| `reasoning` | Supported efforts / budget form |
| `cost` | Per million input, output, cache read, cache write |
| `status`, `retires_at`, `replacement` | `Active`, `Beta`, `Deprecated`, `Retired` |
| `rate_limit_tz` | Provider-local timezone for reset times without an offset (see quotas) |

### Sources and priority (highest wins, field by field)

1. User overrides and static custom providers in `moltis.toml`
2. Runtime probe results (`models.detect_supported`)
3. Live provider metadata (`/models` with a typed parser per provider), cached on disk
4. models.dev snapshot refreshed in the background (ETag, every 6–24 h, cached under `data_dir()`)
5. models.dev snapshot bundled at build time, trimmed to known providers
6. Id heuristics (today's `model_capabilities.rs`), last resort
7. Defaults

Guard copied from OpenClaw: remote catalogs may only add or update metadata
for providers that are already configured. They never set base URLs,
headers or credentials. `catalog.refresh_url` can point to a mirror or be
disabled for offline installs.

### Roles instead of ids

Features name a role resolved against the catalog, for example
`image = { provider = "openai", kind = "image", pick = "newest-active" }`.
A small curated `recommended.toml` ranks picks per provider; every entry must
exist in the catalog and not be deprecated (CI check).

### Names

Resolution order for `display_name`:

1. User override
2. Name from the provider API when it has one (Anthropic `display_name`,
   OpenRouter `name`, NEAR AI `model_display_name`, Copilot)
3. models.dev `name`
4. Prettified id as last resort: split on `-`/`_`/`/`, drop date and
   snapshot suffixes into a secondary label, keep known casings (`GPT`,
   `GLM`, `Qwen`, `DeepSeek`, `o3`), so `glm-4.6-air-0925` becomes
   "GLM 4.6 Air" with "0925" shown as the version.

Pickers show the name, with the wire id in a secondary line and in tooltips.

### Static custom providers

Same schema in TOML, so generated config (Nix module, Ansible, plain file)
can describe models without any network:

```toml
[[providers.custom]]
name = "my-vllm"
base_url = "http://gpu-box:8000/v1"
wire_api = "openai"
catalog = "static"           # "static" | "live" | "live+static"

[[providers.custom.models]]
id = "qwen3.8-27b"
display_name = "Qwen 3.8 27B (box)"
context_window = 131072
max_output = 16384
tools = true
reasoning = ["low", "medium", "high"]
```

`live+static`: live `/models` decides what exists; static entries fill
metadata.

#### Lessons from an existing Nix setup (OpenClaw, then Moltis)

The maintainer's NixOS config (`~/.config/nix`, history up to commit
`b903b93`) shows what a config-generated catalog needs:

- `assets/openclaw/update-models.py` builds `<provider>-models.json` per
  provider from whatever each API exposes: GreenPT `/v1/pricing` (context,
  max output, EUR prices), CompactifAI `/v1/models` capabilities plus prices
  scraped from its docs, Supernova ids plus a scraped HTML table, Kimi Code
  `/models` (`display_name`, `context_length`, modalities; needs a coding-agent
  User-Agent), Z.AI Coding Plan `/models` (ids only).
- Rows use OpenClaw's model shape: `id`, `name`, `reasoning`, `input`
  (modalities), `contextWindow`, `maxTokens`, `cost` {`input`, `output`,
  `cacheRead`, `cacheWrite`} in USD per million tokens (EUR converted at the
  ECB rate, VAT added).
- Fields the API does not publish are owned by the JSON file and survive
  refreshes; models the provider stops listing are dropped.
- `openclaw-catalog.nix` `mergedModels`: for plugin-owned providers, keep the
  plugin's models the server still serves, add served models the plugin does
  not know, drop deprecated ones, and warn on both mismatches at evaluation.
- Context-window probe: CompactifAI publishes no window, but a request with an
  absurd `max_tokens` is rejected before inference with "maximum context
  length of N", for free. Replicas behind one endpoint report different
  windows, so the probe repeats and keeps the max.
- Current `moltis.nix`: custom providers only take a list of ids
  (`models = [ "kimi-k3" ]`), so every capability is guessed; a keyless local
  server needs `api_key = "unused"` because custom providers without a key are
  skipped; Ollama needs `fetch_models = false` so the embedding model is not
  offered for chat.

What the catalog must therefore support:

1. `models` entries as either a bare id or a full object, with the fields
   above (accept OpenClaw's camelCase names as aliases so existing files load
   unchanged).
2. `models_file = "/nix/store/…/greenpt-models.json"`: load an OpenClaw-shaped
   `{ "models": [...] }` file directly, so generated files are reused as-is.
3. `catalog = "live+static"` implements `mergedModels`: served ∩ known keeps
   static metadata, served-only models are added from live data, known-only
   models are dropped with a warning in logs and in the providers UI.
4. Optional `api_key` for custom providers (no auth for local servers).
5. `kind` on every model, so embedding models never show up in chat pickers
   and `fetch_models = false` is no longer needed as a workaround.
6. Display names never include the provider (`"kimi-k3 (Kimi)"` in the
   generated files): the UI shows the provider separately.
7. The context-window probe as an opt-in runtime probe
   (`models.detect_supported` already exists), storing the max seen.
8. Cost stays in USD per million tokens; a `cost_note` string can record
   currency and tax assumptions.
9. Per-provider extra headers (Kimi Code's User-Agent check) live in static
   config, not in code.

## Context sizes for compaction

- `LlmProvider::context_window()` reads the catalog entry of the *current*
  model; the 200 K fallback goes away. Unknown model: use the provider's
  declared default, and show "unknown context size" in the context meter
  instead of guessing silently.
- Usable input budget = `context_window - max_output_reserved - safety
  margin`. Compaction thresholds are fractions of that budget, not of the raw
  window.
- Recompute on model switch mid-session; if the new window is smaller than
  the current history, compact before the next request instead of failing
  with `context_window_exceeded`.
- The context meter in the UI uses the same number.

## Rate limits, quotas and resume after reset

### Typed errors

Replace substring classification with a typed error built by each provider
from status code, headers and body:

```rust
enum ProviderError {
    RateLimited { retry_after: Option<Duration>, scope: LimitScope },
    QuotaExhausted { resets_at: Option<OffsetDateTime>, window: QuotaWindow },
    ContextWindowExceeded { limit: Option<u32> },
    Auth,
    Billing,
    Server { status: u16 },
    Network,
    Other,
}
```

`QuotaWindow`: rolling 5 h, daily, weekly, monthly, unknown. `LimitScope`:
requests, input tokens, output tokens, concurrent.

### Parsing reset times (per provider, unit-tested with captured samples)

| Source | Format |
|---|---|
| `Retry-After` header | Seconds or HTTP-date |
| Anthropic `anthropic-ratelimit-*-reset` | RFC 3339 with offset |
| OpenAI `x-ratelimit-reset-requests/tokens` | Durations like `6m0s`, `1.5s` |
| Claude / Codex subscription windows | 5 h and weekly windows from response headers or the error body |
| Z.AI / GLM coding plan | Reset time in the error body **without an offset**, in China Standard Time (Asia/Shanghai, UTC+8). Verify against live samples before relying on it. |
| Kimi, MiniMax, DeepSeek, others | Capture samples during implementation |

Rules:

- Store every instant as UTC `OffsetDateTime` (`time` crate).
- A provider-local time without an offset is interpreted with the
  provider's `rate_limit_tz` from the catalog, never with the server's local
  timezone. China Standard Time has no DST, so a fixed UTC+8 offset is
  correct for GLM; providers with DST need a real tz database (`time` has
  none: use `jiff` or `time-tz` as a workspace dependency, decided at
  implementation).
- Display reset times in the user's timezone (already used for prompts,
  upstream PR #1319).
- Sanity bounds: a parsed reset in the past or more than 8 days ahead is
  logged and treated as unknown.

### Behaviour

1. A fallback chain exists: rotate to the next provider (today's behaviour),
   but mark the exhausted one with its `resets_at` instead of a 60 s breaker.
2. No fallback, reset known and within a configurable horizon (default 6 h):
   park the run as `WaitingForQuota { resets_at }`, persist it so it survives
   a restart, and resume automatically at `resets_at` plus small jitter.
3. Reset unknown: exponential backoff with a cap, then give up with a clear
   message.
4. The user always sees it: a countdown in the chat ("GLM quota resets at
   18:00, resuming automatically"), a cancel button, and a message on channels
   (no silent failures, project rule).
5. Cron jobs and background runs use the same mechanism.

## Hotfix before the catalog lands

The hard-coded ids in features break regardless of catalog work, so patch
them first: image generation (Codex orchestrator and default image model),
TTS/STT defaults (OpenAI, Mistral, Gemini TTS), Perplexity search (move to
the Search API), Groq and xAI off the genai path. Claude setup-tokens become
opt-in with a warning (Anthropic terms risk, see the providers report).

## Tests

- Catalog merge priority (property tests over sources).
- Bundled snapshot loads offline; refresh honours ETag; remote cannot set
  base URLs.
- CI: every model id referenced in code or the config template exists in the
  snapshot and is not deprecated.
- Name prettifier on a table of real ids.
- Reset-time parsers on captured samples per provider, including GLM's
  offset-less time across a UTC date boundary.
- Parked run resumes after a simulated reset, and survives a restart.
- Compaction uses the catalog window; model switch to a smaller window
  compacts before sending.

## Out of scope here

Pricing-based cost display and budgets use the same `cost` field but belong
to the chat UX phase.
