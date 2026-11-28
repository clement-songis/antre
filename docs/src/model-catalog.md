# Model Catalog

Antre keeps one catalog of model metadata that every feature reads: model
pickers, compaction, the context meter, image generation and voice defaults.
It replaces the hand-maintained model lists that went stale every few weeks.

## What the catalog knows

For each provider model:

| Field | Meaning |
|---|---|
| `display_name` | Human name shown in pickers (the wire id stays visible as a secondary label) |
| `kind` | `chat`, `embedding`, `rerank`, `image`, `video`, `tts`, `stt` or `realtime`; only chat models appear in chat pickers |
| `context_window`, `max_output` | Token limits |
| `input_modalities`, `output_modalities` | `text`, `image`, `pdf`, `audio`, `video` |
| `tools`, `structured_output`, `caching`, `reasoning` | Capabilities (`reasoning` is a flag or the list of supported efforts) |
| `cost` | USD per million tokens: `input`, `output`, `cache_read`, `cache_write` |
| `status`, `retires_at`, `replacement` | Lifecycle: `active`, `alpha`, `beta`, `deprecated`, `retired` |
| `rate_limit_tz` | Timezone of quota reset times the provider prints without an offset (Z.AI: `+08:00`) |

## Where the data comes from

Sources are merged field by field; the first one that knows a field wins:

1. **Overrides**: `[models.<id>]` and `[providers.<name>.model_overrides.<id>]`
   in `antre.toml`.
2. **Runtime probes**: results of `models.detect_supported`.
3. **Live provider metadata**: what each provider's `/models` returns
   (Anthropic limits and lifecycle, OpenAI shutdown dates, Copilot and Codex
   context windows, display names). The last successful result per provider
   is kept under `data_dir()/model-catalog/live/`, so a failed discovery keeps
   the real model list.
4. **Refreshed models.dev**: downloaded in the background (see below) and
   cached under `data_dir()/model-catalog/`.
5. **Bundled snapshot**: the whole [models.dev](https://models.dev) catalog,
   compressed into the binary (about 700 KB), plus Antre's
   `crates/model-catalog/data/supplement.toml` for models models.dev does not
   list yet (mostly image and voice models).
6. **Id heuristics**: patterns on the model id, as a last resort.

A custom provider whose `base_url` matches the API URL a models.dev provider
publishes (GreenPT, CompactifAI and others that only return ids on `/models`)
inherits that provider's metadata. Models served by any provider under a
well-known id (`kimi-k3`, `openai/gpt-oss-120b`) fall back to the lab's own
metadata.

When no source knows a model's context window, the provider's default is used
for budgeting and the context meter says the size is unknown instead of
guessing.

## Refresh

```toml
[catalog]
refresh = true
refresh_urls = [
  "https://models.dev/api.json",
  "https://raw.githubusercontent.com/clement-songis/antre/main/crates/model-catalog/data/models-dev.json.gz",
]
refresh_interval_hours = 12
```

- URLs are tried in order. The second one is the snapshot committed to this
  repository, so an outage or a format change at models.dev falls back to the
  last known-good copy. Point both at your own mirror to avoid third-party
  requests, or set `refresh = false` for offline installs.
- A download replaces the cache only after it parses, stays under 64 MB and
  holds at least 500 models. Conditional requests (`ETag`) keep refreshes
  cheap.
- A refreshed catalog only describes providers that are configured, and it can
  never set base URLs, headers or credentials: those come only from your
  config.

The committed snapshot is refreshed daily by the `Refresh model catalog`
GitHub Actions workflow, which opens a pull request with a summary of added,
removed and newly deprecated models. To refresh it by hand:

```bash
./scripts/update-model-catalog.sh
```

## Roles instead of model ids

Features ask for a role, such as "OpenAI's image model", instead of naming an
id. `crates/model-catalog/data/recommended.toml` ranks the picks per provider
and kind; if none is available the newest active model of that kind is used.
A test fails CI when a recommended id, or an id in a provider's offline
fallback list, is missing from the catalog or deprecated.
