# LLM Providers

Moltis supports multiple LLM providers through a trait-based architecture.
Configure providers through the web UI or directly in configuration files.

## Available Providers

### API Key Providers

| Provider | Config Name | Env Variable | Features |
|----------|-------------|--------------|----------|
| **Anthropic** | `anthropic` | `ANTHROPIC_API_KEY` | Streaming, tools, vision |
| **OpenAI** | `openai` | `OPENAI_API_KEY` | Streaming, tools, vision, model discovery |
| **Google Gemini** | `gemini` | `GEMINI_API_KEY` | Streaming, tools, vision, model discovery |
| **DeepSeek** | `deepseek` | `DEEPSEEK_API_KEY` | Streaming, tools, model discovery |
| **Mistral** | `mistral` | `MISTRAL_API_KEY` | Streaming, tools, model discovery |
| **Groq** | `groq` | `GROQ_API_KEY` | Streaming, tools, model discovery |
| **xAI (Grok)** | `xai` | `XAI_API_KEY` | Streaming, tools, model discovery |
| **OpenRouter** | `openrouter` | `OPENROUTER_API_KEY` | Streaming, tools, model discovery |
| **Cerebras** | `cerebras` | `CEREBRAS_API_KEY` | Streaming, tools, model discovery |
| **MiniMax** | `minimax` | `MINIMAX_API_KEY` | Streaming, tools, M3 vision |
| **Moonshot (Kimi)** | `moonshot` | `MOONSHOT_API_KEY` | Streaming, tools, model discovery |
| **Fireworks** | `fireworks` | `FIREWORKS_API_KEY` | Streaming, tools, model discovery |
| **Venice** | `venice` | `VENICE_API_KEY` | Streaming, tools, model discovery |
| **NEAR AI Cloud** | `nearai` | `NEARAI_API_KEY` | Streaming, TEE-aware model discovery |
| **Z.AI (Zhipu)** | `zai` | `Z_API_KEY` | Streaming, tools, model discovery |
| **Z.AI Coding Plan** | `zai-code` | `Z_CODE_API_KEY` | Streaming, tools, model discovery (Coding plan billing endpoint) |

### Hosted OpenAI-compatible providers

These hosts and routers are one table entry each. Models come from the
provider's `/models` endpoint; when it is unavailable, Moltis lists the
provider's active chat models from the bundled model catalog. Strict tool
schemas and the message `name` field are off by default, because these
hosts proxy many backends.

| Provider | Config Name | Env Variable | Default base URL |
|----------|-------------|--------------|------------------|
| **Together AI** | `together` | `TOGETHER_API_KEY` | `https://api.together.ai/v1` |
| **SambaNova** | `sambanova` | `SAMBANOVA_API_KEY` | `https://api.sambanova.ai/v1` |
| **Hugging Face** (router) | `huggingface` | `HF_TOKEN` | `https://router.huggingface.co/v1` |
| **NVIDIA** | `nvidia` | `NVIDIA_API_KEY` | `https://integrate.api.nvidia.com/v1` |
| **Novita** | `novita` | `NOVITA_API_KEY` | `https://api.novita.ai/openai` |
| **Chutes** | `chutes` | `CHUTES_API_KEY` | `https://llm.chutes.ai/v1` |
| **Nebius Token Factory** | `nebius` | `NEBIUS_API_KEY` | `https://api.tokenfactory.nebius.com/v1` |
| **Vercel AI Gateway** | `vercel` | `AI_GATEWAY_API_KEY` | `https://ai-gateway.vercel.sh/v1` |
| **Requesty** | `requesty` | `REQUESTY_API_KEY` | `https://router.requesty.ai/v1` |
| **Cloudflare Workers AI** | `cloudflare` | `CLOUDFLARE_API_KEY` | `https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1` (`CLOUDFLARE_ACCOUNT_ID`) |
| **Baseten** | `baseten` | `BASETEN_API_KEY` | `https://inference.baseten.co/v1` |
| **StepFun** | `stepfun` | `STEPFUN_API_KEY` | `https://api.stepfun.ai/v1` |
| **BytePlus ModelArk** | `byteplus` | `BYTEPLUS_API_KEY` | `https://ark.ap-southeast.bytepluses.com/api/v3` |
| **Tencent TokenHub** | `tencent` | `TENCENT_TOKENHUB_API_KEY` | `https://tokenhub-intl.tencentcloudmaas.com/v1` |
| **Xiaomi MiMo** | `xiaomi` | `XIAOMI_API_KEY` | `https://api.xiaomimimo.com/v1` |
| **LongCat** | `longcat` | `LONGCAT_API_KEY` | `https://api.longcat.chat/openai` |
| **Alibaba Model Studio** (Qwen pay-as-you-go) | `alibaba` | `DASHSCOPE_API_KEY` | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` |
| **Cohere** (compatibility API) | `cohere` | `COHERE_API_KEY` | `https://api.cohere.ai/compatibility/v1` |
| **Amazon Bedrock** | `bedrock` | `AWS_BEARER_TOKEN_BEDROCK` | `https://bedrock-mantle.{region}.api.aws/v1` (`AWS_REGION`, default `us-east-1`) |
| **Azure OpenAI** | `azure` | `AZURE_API_KEY` | `https://{resource}.openai.azure.com/openai/v1` (`AZURE_RESOURCE_NAME`) |

`{placeholder}` values come from the variable in parentheses and may only
contain letters, digits, `-` and `_`. Setting `base_url` (or
`<NAME>_BASE_URL`, for example `NOVITA_BASE_URL`) replaces the whole URL,
which is how you pick another region (the China endpoints of StepFun,
Tencent or Alibaba, for example).

#### Amazon Bedrock and Azure

Both use API keys sent as bearer tokens, not AWS SigV4 or Entra ID.

- **Bedrock:** create a Bedrock API key and set `AWS_BEARER_TOKEN_BEDROCK`.
  Model ids are Bedrock ids such as `anthropic.claude-sonnet-5-5` or
  `openai.gpt-6-astra`, optionally with a cross-region prefix (`us.`,
  `global.`).
- **Azure:** `model` is your **deployment name**, so list deployments in
  config; nothing is discovered.

```toml
[providers.azure]
api_key = "${AZURE_API_KEY}"
base_url = "https://contoso.openai.azure.com/openai/v1"
models = ["my-gpt-deployment"]
```

Claude on either cloud speaks the Anthropic Messages API, through a
custom Anthropic instance (both accept the key in `x-api-key`):

```toml
[providers.custom-bedrock-claude]
wire_api = "anthropic"
api_key = "${AWS_BEARER_TOKEN_BEDROCK}"
base_url = "https://bedrock-mantle.us-east-1.api.aws/anthropic"
models = ["anthropic.claude-sonnet-5-5"]

[providers.custom-azure-claude]
wire_api = "anthropic"
api_key = "${AZURE_API_KEY}"
base_url = "https://contoso.services.ai.azure.com/anthropic"
models = ["my-claude-deployment"]
```

### OAuth Providers

| Provider | Config Name | Notes |
|----------|-------------|-------|
| **OpenAI Codex** | `openai-codex` | OAuth flow via web UI |
| **GitHub Copilot** | `github-copilot` | Requires active Copilot subscription |

### Local

| Provider | Config Name | Notes |
|----------|-------------|-------|
| **Ollama** | `ollama` | Local or remote Ollama instance |
| **LM Studio** | `lmstudio` | Local LM Studio or any OpenAI-compatible server |
| **Local LLM** | `local-llm` | Runs GGUF models directly on your machine |

### Custom OpenAI-Compatible

Any OpenAI-compatible endpoint can be added with a `custom-` prefix:

```toml
[providers.custom-myservice]
enabled = true
api_key = "..."
base_url = "https://my-service.example.com/v1"
models = ["my-model"]
```

`api_key` is optional: a local server without authentication (llama.cpp,
vLLM, LM Studio) gets no `Authorization` header.

#### Describing models statically

A `models` entry can be a bare id or a full description, so a config
generator (a Nix module, Ansible, a plain file) can describe every model
without any network access. Field names follow the
[model catalog](model-catalog.md); OpenClaw's camelCase names
(`contextWindow`, `maxTokens`, `input`, `cacheRead`...) are accepted too.

```toml
[providers.custom-my-vllm]
base_url = "http://gpu-box:8000/v1"
catalog = "static"            # "static" | "live" | "live+static"
headers = { "User-Agent" = "my-agent/1.0" }

[[providers.custom-my-vllm.models]]
id = "qwen3.8-27b"
display_name = "Qwen 3.8 27B (box)"
context_window = 131072
max_output = 16384
tools = true
reasoning = ["low", "medium", "high"]

[[providers.custom-my-vllm.models]]
id = "nomic-embed-text"
kind = "embedding"            # never offered in chat pickers
```

- `models_file = "/nix/store/…/greenpt-models.json"` loads an OpenClaw-shaped
  `{ "models": [...] }` file as-is; `models` entries win field by field. A
  name that only repeats the id plus the provider (`"kimi-k3 (Kimi)"`) is
  dropped, since the UI shows the provider separately.
- `catalog` decides which models exist: `live` (default) lists configured
  models first, then what `/models` returns; `static` uses only the
  configured models and skips discovery; `live+static` lets `/models`
  decide, keeps the static description of served models, adds served models
  you did not describe, and drops described models the server no longer
  serves. The last two cases are logged and shown on the providers page.
- `headers` are sent with every request, including discovery (some
  endpoints check the User-Agent). They only ever come from your config.
- `probe_context_window = true` probes each model's window at startup for
  endpoints that publish none: a request with an absurd `max_tokens` is
  rejected before inference with "maximum context length is N". The probe
  runs three times and keeps the largest answer (replicas can differ), and
  the result is cached under `data_dir()/model-catalog/live/`. Only enable it
  for servers that validate `max_tokens`; one that accepts the request is
  detected and left alone.
- Prices stay in USD per million tokens; `cost_note` can record the
  currency and tax assumptions behind them.

### Custom Anthropic instances (`wire_api = "anthropic"`)

A `custom-*` provider can speak the Anthropic Messages API instead, which
makes several Claude accounts selectable as separate providers — the account
is visible in the model selector as `custom-<name>::<model>`:

```toml
# Personal account, via the builtin provider:
[providers.anthropic]
setup_token = "${ANTHROPIC_OAUTH_TOKEN}"

# Second account, selected explicitly:
[providers.custom-anthropic-backup]
wire_api = "anthropic"
setup_token = "${ANTHROPIC_BACKUP_OAUTH_TOKEN}"
models = ["claude-opus-5-5"]
# base_url defaults to https://api.anthropic.com
```

Each instance takes its own `api_key` or `setup_token` (same OAuth request
shape as the builtin). Configured `models` are preferred; without them the
catalog is discovered from the endpoint.

## Configuration

### Via Web UI (Recommended)

1. Open Moltis in your browser.
2. Go to **Settings** → **Providers**.
3. Choose a provider card.
4. Complete OAuth or enter your API key.
5. Select your preferred model.

### Via Configuration Files

Configure providers in `moltis.toml`:

```toml
[providers]
offered = ["anthropic", "openai", "gemini"]

[providers.anthropic]
enabled = true

[providers.openai]
enabled = true
models = ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]
stream_transport = "sse"              # "sse", "websocket", or "auto"

[providers.gemini]
enabled = true
models = ["gemini-3.8-flash", "gemini-4-carbon"]
# api_key = "..."                     # Or set GEMINI_API_KEY / GOOGLE_API_KEY env var
# fetch_models = true                 # Discover models from the API
# base_url = "https://generativelanguage.googleapis.com/v1beta/openai"

[chat]
priority_models = ["gpt-5.6-sol"]
```

### Provider Entry Options

Each provider supports these options:

| Option | Default | Description |
|--------|---------|-------------|
| `enabled` | `true` | Enable or disable the provider |
| `api_key` | — | API key (overrides env var) |
| `base_url` | — | Override API endpoint URL |
| `models` | `[]` | Preferred models shown first in the picker |
| `fetch_models` | `true` | Discover available models from the API |
| `stream_transport` | `"sse"` | `"sse"`, `"websocket"`, or `"auto"` |
| `alias` | — | Custom label for metrics |
| `tool_mode` | `"auto"` | `"auto"`, `"native"`, `"text"`, or `"off"` |

## Provider Setup

### Fireworks

The built-in Fireworks catalog includes Kimi K3 and GPT OSS 120B. For Kimi on
the serverless API, use `accounts/fireworks/models/kimi-k3`:

```toml
[providers.fireworks]
enabled = true
models = ["accounts/fireworks/models/kimi-k3"]
# Set FIREWORKS_API_KEY in the environment.
```

Fireworks retired Kimi K2.6 and K2.7 Code from serverless on September 25, 2026;
those models remain available for on-demand deployments. If your configuration
pins `accounts/fireworks/models/kimi-k2p6`, update it to Kimi K3 to avoid model-not-found
errors. See the [Fireworks Kimi K3 model page](https://fireworks.ai/models/fireworks/kimi-k3).

### Google Gemini

Google Gemini uses an API key from [Google AI Studio](https://aistudio.google.com/).

1. Get an API key from Google AI Studio.
2. Set `GEMINI_API_KEY` in your environment (or use `GOOGLE_API_KEY`).
3. Gemini models appear automatically in the model picker.

```toml
[providers.gemini]
enabled = true
models = ["gemini-3.8-flash", "gemini-4-carbon"]
```

Gemini supports native tool calling, vision/multimodal inputs, streaming, and automatic model discovery.

### Anthropic

1. Get an API key from [console.anthropic.com](https://console.anthropic.com/).
2. Set `ANTHROPIC_API_KEY` in your environment.

#### Claude subscription (`claude setup-token`)

A Claude Pro/Max subscription can be used instead of an API key. Run
`claude setup-token` on a machine with Claude Code installed, then configure
the token:

```toml
[providers.anthropic]
setup_token = "${ANTHROPIC_OAUTH_TOKEN}"
```

The token can also be set through the `ANTHROPIC_OAUTH_TOKEN` environment
variable, and an `sk-ant-oat…` value passed as `api_key`/`ANTHROPIC_API_KEY` is
detected automatically. Tokens imported from an OpenClaw installation by
`moltis import` (the `anthropic` entry of `oauth_tokens.json`) are used as a
fallback when nothing else is configured.

Additional Claude accounts belong in separate `custom-*` instances with
`wire_api = "anthropic"` (see below), so the active account is always the one
selected in the model selector — there is no automatic rotation between
accounts.

Because Anthropic only accepts subscription tokens for Claude Code traffic,
these requests authenticate with `Authorization: Bearer` and identify as the
Claude Code CLI (headers and system-prompt identity, the same request shape
OpenClaw uses). Setup-tokens are long-lived but not refreshed by Moltis: when
one expires, run `claude setup-token` again and replace it.

### OpenAI

1. Get an API key from [platform.openai.com](https://platform.openai.com/).
2. Set `OPENAI_API_KEY` in your environment.

### DeepSeek

Use `deepseek-flash` for the current Flash model and `deepseek-v4-pro` for Pro:

```toml
[providers.deepseek]
models = ["deepseek-flash", "deepseek-v4-pro"]
# Set DEEPSEEK_API_KEY in the environment.
```

Both have a 1M-token context window. Flash supports images; Pro is text-only.
DeepSeek still accepts `deepseek-v4-flash` as a legacy alias for the current
Flash model. When reasoning effort is configured, Moltis explicitly enables
thinking and maps minimal/low to `low`, medium/high/extra-high to `high`, and
max to `max`. Reasoning is replayed on prior assistant turns, including answers
without tool calls, when available. See the [model list](https://api-docs.deepseek.com/quick_start/pricing)
and [thinking-mode guide](https://api-docs.deepseek.com/guides/thinking_mode).

### Z.AI

The static catalog includes GLM-5.3, GLM-5.3-Flash, GLM-5.3-FlashX, and GLM-5.2,
alongside earlier supported models. These current models have 1M-token context
windows; the GLM-5.3 Flash variants also support image input. GLM-4.5-Flash
remains listed as a supported free model.

```toml
[providers.zai]
models = ["glm-5.3", "glm-5.3-flash", "glm-5.3-flashx"]
# Set Z_API_KEY for the standard API.
```

Use the separate `zai-code` provider and `Z_CODE_API_KEY` for Coding Plan
credentials. Model access depends on the endpoint and plan; FlashX is not
currently included in the Coding Plan. GLM-5.3 requires thinking to stay enabled;
Moltis maps configured effort to the supported `low`, `high`, and `max` levels
and preserves returned reasoning. See [Z.AI pricing](https://docs.z.ai/guides/overview/pricing)
and the [GLM-5.3 migration guide](https://docs.z.ai/guides/overview/migrate-to-glm-new).

Streaming transport failures include timeout/connection flags and the underlying
cause, with the request URL removed. These errors are distinct from HTTP model
availability or authentication errors.

### MiniMax

The built-in `minimax` provider uses the OpenAI-compatible chat completions
API and includes `MiniMax-M3.1-Flash` and `MiniMax-M3` in the static model catalog.
Both have a 1M-token context window and support image input. MiniMax has retired
the M2 API models, including their highspeed variants; update any pinned M2
model IDs to one of these current models to avoid HTTP 404 errors.

The built-in MiniMax provider preserves Flash reasoning across tool calls for
both streaming chat and non-streaming spawned agents. Simple tool calls may
omit reasoning; Moltis replays it only when the provider actually returns it.

```toml
[providers.minimax]
enabled = true
api_key = "..."                         # Or set MINIMAX_API_KEY
models = ["MiniMax-M3.1-Flash", "MiniMax-M3"]
fetch_models = false
base_url = "https://api.minimax.io/v1"  # Global endpoint
# For China, use "https://api.minimaxi.com/v1".
```

MiniMax also provides an Anthropic-compatible messages API. Use the existing
Anthropic adapter when that protocol is required:

```toml
[providers.anthropic]
enabled = true
api_key = "..."
models = ["MiniMax-M3.1-Flash", "MiniMax-M3"]
fetch_models = false
base_url = "https://api.minimax.io/anthropic" # Global endpoint
alias = "minimax-anthropic"
# For China, use "https://api.minimaxi.com/anthropic".
```

The adapter appends `/v1/messages` to the configured base URL. Keep the
`/anthropic` suffix and do not add `/v1` to the configured Anthropic base URL.

### NEAR AI Cloud

NEAR AI Cloud exposes an OpenAI-compatible chat completions API with a public
model catalog that includes TEE and attestation metadata.
The API accepts OpenAI-compatible tool schemas. Moltis currently keeps tool and
reasoning capabilities disabled for auto-discovered NEAR AI Cloud models pending
validation of the catalog's feature metadata against the adapter.

1. Get an API key from [cloud.near.ai](https://cloud.near.ai/).
2. Set `NEARAI_API_KEY` in your environment.
3. Models are discovered from `https://cloud-api.near.ai/v1/model/list`.

```toml
[providers.nearai]
enabled = true
models = ["z-ai/glm-5.3-flash"]
# api_key = "..."                    # Or set NEARAI_API_KEY
# base_url = "https://cloud-api.near.ai/v1"
```

Use the canonical `modelId` from the catalog when choosing a preferred model.
For example, `zai-org/GLM-5.1-FP8` remains a completion alias for
`z-ai/glm-5.3-flash`, but is no longer a separate model in discovery results.

### OpenAI Codex

OpenAI Codex uses OAuth-based access.

1. Go to **Settings** → **Providers** → **OpenAI Codex**.
2. Click **Connect** and complete the auth flow.
3. Choose a Codex model.

**Already logged in with the Codex CLI?** Moltis detects `~/.codex/auth.json`
but never reads or copies it on its own. Either copy those tokens once with
the `providers.import_external` RPC (`{"source": "codex-cli"}`), or let Moltis
read the Codex CLI's file live (it then follows the CLI's token refreshes):

```toml
[providers]
external_credentials = ["codex-cli"]
```

If the browser cannot reach `localhost:1455`, Moltis now supports a manual
fallback in both **Settings** and **Onboarding**: paste the callback URL (or
`code#state`) into the OAuth panel and submit it.

```admonish note title="Docker and cloud deployments"
The OAuth flow redirects your browser to `localhost:1455`. In Docker, make sure
port 1455 is published (`-p 1455:1455`). On cloud platforms where `localhost`
cannot reach the server, authenticate via the CLI instead:

~~~bash
# Docker
docker exec -it moltis moltis auth login --provider openai-codex

# Fly.io
fly ssh console -C "moltis auth login --provider openai-codex"
~~~

The CLI opens a browser on your machine and handles the callback locally. If
automatic callback capture fails, the CLI prompts you to paste the callback URL
(or `code#state`) directly in the terminal.
Tokens are saved to the config volume and picked up by the gateway automatically.
```

Once OpenAI Codex OAuth is connected, agents can use the built-in
`generate_image` tool to create `gpt-image-2.6-flare` (default) or `gpt-image-2.6-sunburst` images without an `OPENAI_API_KEY`.
Generated images are delivered through the same channel media path as
screenshots and `send_image`, so supported chat channels receive the image as a
native attachment.

### GitHub Copilot

GitHub Copilot uses OAuth authentication.

1. Go to **Settings** → **Providers** → **GitHub Copilot**.
2. Click **Connect**.
3. Complete the GitHub OAuth flow.

```admonish note title="Docker and cloud deployments"
GitHub Copilot uses device-flow authentication (a code you enter on github.com),
so it works from the web UI without extra port configuration. If you prefer the
CLI:

~~~bash
docker exec -it moltis moltis auth login --provider github-copilot
~~~
```

```admonish info
Requires an active GitHub Copilot subscription.
```

Moltis discovers Copilot models from GitHub's `/models` endpoint when possible.
If GitHub returns non-secret model limit metadata such as `context_window`,
`context_length`, or `max_input_tokens`, Moltis uses it for context-window
display and auto-compaction thresholds. Otherwise Moltis falls back to built-in
model heuristics. Fast-mode Claude variants are kept at the standard Claude
window unless GitHub documents a larger limit for that variant.

To inspect the redacted live model metadata Moltis can see:

```bash
moltis providers inspect github-copilot
```

If GitHub changes a model limit before Moltis has updated heuristics, override it
in config:

```toml
[providers.github-copilot.model_overrides."claude-opus-4.8"]
context_window = 1_000_000
```

### Ollama

Ollama auto-detects when running at `http://127.0.0.1:11434`. No API key needed.

```toml
[providers.ollama]
enabled = true
# base_url = "http://127.0.0.1:11434/v1"  # Override for remote Ollama
```

### LM Studio

LM Studio auto-detects when running at `http://127.0.0.1:1234`. No API key needed.
Also works with llama.cpp or any OpenAI-compatible local server.

```toml
[providers.lmstudio]
enabled = true
# base_url = "http://127.0.0.1:1234/v1"  # Override for different port/host
```

### Local LLM

Local LLM runs GGUF models directly on your machine.

1. Go to **Settings** → **Providers** → **Local LLM**.
2. Choose a model from the local registry or download one.
3. Save and select it as your active model.

## Switching Models

- **Per session**: Use the model selector in the chat UI.
- **Per message**: Use `/model <name>` in chat.
- **Global defaults**: Use `[providers].offered`, provider `models = [...]`, and
  `[chat].priority_models` in `moltis.toml`.

## Troubleshooting

### "Model not available"

- Check provider auth is still valid.
- Check model ID spelling.
- Check account access for that model.

### "Rate limited"

- Retry after a short delay.
- Switch provider/model.
- Upgrade provider quota if needed.

### "Invalid API key"

- Verify the key has no extra spaces.
- Verify it is active and has required permissions.
