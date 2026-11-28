# Cloud Deploy

Antre publishes a multi-arch Docker image (`linux/amd64` and `linux/arm64`)
to `ghcr.io/antre-ai/antre`. You can deploy it to any cloud provider that
supports container images.

## Common configuration

All cloud providers terminate TLS at the edge, so Antre must run in plain
HTTP mode. The key settings are:

| Setting | Value | Purpose |
|---------|-------|---------|
| `--no-tls` or `ANTRE_NO_TLS=true` | Disable TLS | Provider handles HTTPS |
| `--bind 0.0.0.0` | Bind all interfaces | Required for container networking |
| `--port <PORT>` | Listen port | Must match provider's expected internal port |
| `ANTRE_CONFIG_DIR=/data/config` | Config directory | Persist antre.toml, credentials |
| `ANTRE_DATA_DIR=/data` | Data directory | Persist databases, sessions, memory |
| `ANTRE_DEPLOY_PLATFORM` | Deploy platform | Hides local-only providers (see below) |
| `ANTRE_PASSWORD` | Initial password | Set auth password via environment variable |

```admonish tip
If requests to your domain are redirected to `:13131`, Antre TLS is still
enabled behind a TLS-terminating proxy. Use `--no-tls` (or
`ANTRE_NO_TLS=true`).

Only keep Antre TLS enabled when your proxy talks HTTPS to Antre (or uses
TCP TLS passthrough). In that case, set `ANTRE_ALLOW_TLS_BEHIND_PROXY=true`.
```

```admonish tip
**Sandbox on cloud deploys**: Most cloud providers do not support
Docker-in-Docker. To enable sandboxed command execution, configure a
[remote sandbox backend](sandbox-remote.md) — set `VERCEL_TOKEN` for Vercel
Firecracker microVMs, or `DAYTONA_API_KEY` for Daytona cloud sandboxes
(including self-hosted). Antre auto-detects these when no local Docker is
available.
```

### `ANTRE_DEPLOY_PLATFORM`

Set this to the name of your cloud provider (e.g. `flyio`, `digitalocean`,
`render`). When set, Antre hides local-only LLM providers
(local-llm and Ollama) from the provider setup page since they cannot run
on cloud VMs. The included deploy templates for Fly.io, DigitalOcean, and
Render already set this variable.

## ngrok

Antre can also expose a public HTTPS endpoint through ngrok without running
an external `ngrok` binary. This is useful when you want a public callback
URL or temporary team access from a local machine or private host.

For a side-by-side guide to Tailscale, NetBird, ngrok, and Cloudflare Tunnel,
see [Remote Access](remote-access.md).

Configuration:

```toml
[ngrok]
enabled = true
authtoken = "${NGROK_AUTHTOKEN}" # or set NGROK_AUTHTOKEN in the environment
# domain = "team-gateway.ngrok.app" # optional reserved/static domain
```

Notes:

- The tunnel is feature-gated. Standard CLI builds include it by default, but
  custom minimal builds can opt out of the `ngrok` feature.
- In the web UI, configure Tailscale, NetBird, ngrok, and Cloudflare Tunnel from Settings -> Remote Access.
- ngrok forwards into a loopback-only internal HTTP listener. Your normal
  local TLS and bind settings remain unchanged.
- Keep authentication enabled. Exposing ngrok with `auth.disabled = true`
  creates a public unauthenticated endpoint, which is exactly as bad as it
  sounds.
- Passkeys are hostname-bound. If you use ephemeral ngrok URLs, existing
  passkeys may not work on the new hostname. Use a reserved domain if you want
  stable passkey behavior.

## Cloudflare Tunnel

Antre can start `cloudflared` and expose the gateway through a Cloudflare
Tunnel. This is useful when your public DNS is already managed by Cloudflare or
you want a stable HTTPS callback URL without opening inbound firewall ports.

Configuration:

```toml
[cloudflare_tunnel]
enabled = true
token = "${CLOUDFLARE_TUNNEL_TOKEN}" # or set CLOUDFLARE_TUNNEL_TOKEN
hostname = "antre.example.com"      # optional but recommended for passkeys
```

Notes:

- Install `cloudflared` on the host before enabling the connector.
- Standard CLI builds include the feature by default; custom minimal builds can
  opt out of `cloudflare-tunnel`.
- Keep authentication enabled. A public tunnel with `auth.disabled = true`
  exposes the setup-required page until authentication is configured.
- Set `hostname` when you know the public route so Antre can update WebAuthn
  passkey origins consistently.

## NetBird

NetBird provides private mesh access similar to Tailscale Serve. Antre detects
the local NetBird peer address and displays the private URL from Settings ->
Remote Access.

Configuration:

```toml
[netbird]
mode = "serve" # or "off"
```

NetBird does not provide a public Funnel equivalent. Use Cloudflare Tunnel,
ngrok, or Tailscale Funnel when you need a public HTTPS endpoint.

## Coolify (self-hosted, e.g. Hetzner)

Coolify deployments can run Antre with sandboxed exec tools, as long as the
service mounts the host Docker socket.

- Use [`examples/docker-compose.coolify.yml`](https://github.com/antre-ai/antre/blob/main/examples/docker-compose.coolify.yml)
  as a starting point.
- Run Antre with `--no-tls` (Coolify terminates HTTPS at the proxy).
- Set `ANTRE_BEHIND_PROXY=true` so client IP/auth behavior is correct behind
  reverse proxying.
- Mount `/var/run/docker.sock:/var/run/docker.sock` to enable container-backed
  sandbox execution.

## Fly.io

The repository includes a `fly.toml` ready to use.

### Quick start

```bash
# Install the Fly CLI if you haven't already
curl -L https://fly.io/install.sh | sh

# Launch from the repo (uses fly.toml)
fly launch --image ghcr.io/antre-ai/antre:latest

# Set your password
fly secrets set ANTRE_PASSWORD="your-password"

# Create persistent storage
fly volumes create antre_data --region iad --size 1
```

### How it works

- **Image**: pulled from `ghcr.io/antre-ai/antre:latest`
- **Port**: internal 8080, Fly terminates TLS and routes HTTPS traffic
- **Storage**: a Fly Volume mounted at `/data` persists the database, sessions,
  and memory files
- **Auto-scaling**: machines stop when idle and start on incoming requests

### Custom domain

```bash
fly certs add your-domain.com
```

Then point a CNAME to `your-app.fly.dev`.

## DigitalOcean App Platform

[![Deploy to DO](https://www.deploytodo.com/do-btn-blue.svg)](https://cloud.digitalocean.com/apps/new?repo=https://github.com/antre-ai/antre/tree/main)

Click the button above or create an app manually:

1. Go to **Apps** > **Create App**
2. Choose **Container Image** as source
3. Set image to `ghcr.io/antre-ai/antre:latest`
4. Set the run command: `antre --bind 0.0.0.0 --port 8080 --no-tls`
5. Set environment variables:
   - `ANTRE_DATA_DIR` = `/data`
   - `ANTRE_PASSWORD` = your password
6. Set the HTTP port to `8080`

```admonish info title="No persistent disk"
DigitalOcean App Platform does not support persistent disks for image-based
services in the deploy template. Data will be lost on redeployment. For
persistent storage, consider using a DigitalOcean Droplet with Docker instead.
```

## Render

[![Deploy to Render](https://render.com/images/deploy-to-render-button.svg)](https://render.com/deploy?repo=https://github.com/antre-ai/antre)

The repository includes a `render.yaml` blueprint. Click the button above or:

1. Go to **Dashboard** > **New** > **Blueprint**
2. Connect your fork of the Antre repository
3. Render will detect `render.yaml` and configure the service

### Configuration details

- **Port**: Render uses port 10000 by default
- **Persistent disk**: 1 GB mounted at `/data` (included in the blueprint)
- **Environment**: set `ANTRE_PASSWORD` in the Render dashboard under
  **Environment** > **Secret Files** or **Environment Variables**

<!-- TODO: Railway deploy does not work yet
## Railway

The repository includes a `railway.json` configuration that sets the required
environment variables (`ANTRE_CONFIG_DIR`, `ANTRE_DATA_DIR`,
`ANTRE_DEPLOY_PLATFORM`) automatically.

1. Create a new project on [Railway](https://railway.com)
2. Add a service from **Docker Image**: `ghcr.io/antre-ai/antre:latest`
3. Railway injects the `$PORT` variable automatically; the `railway.json` start
   command handles the rest
4. Set additional environment variables in the Railway dashboard:
   - `ANTRE_PASSWORD` = your password

### Persistent storage

Railway supports persistent volumes. Add one in the service settings and mount
it at `/data`.
-->

## OAuth Providers (OpenAI Codex, GitHub Copilot)

OAuth providers that redirect to `localhost` (like OpenAI Codex) cannot
complete the browser flow when Antre runs on a remote server — `localhost`
on the user's browser points to their own machine, not the cloud instance.

**Use the CLI to authenticate instead:**

```bash
# Fly.io
fly ssh console -C "antre auth login --provider openai-codex"

# DigitalOcean (Droplet with Docker)
docker exec -it antre antre auth login --provider openai-codex

# Generic container
docker exec -it <container> antre auth login --provider openai-codex
```

The CLI opens a browser on the machine where you run the command. If automatic
callback capture fails, Antre prompts you to paste the callback URL (or
`code#state`) directly in the terminal. After you log in, tokens are saved to
the config volume and the running gateway picks them up automatically — no
restart needed.

```admonish tip
GitHub Copilot uses device-flow authentication (a code you enter on
github.com), so it works from the web UI without this workaround.
```

## Authentication

On first launch, Antre requires a password or passkey to be set. In cloud
deployments the easiest approach is to set the `ANTRE_PASSWORD` environment
variable (or secret) before deploying. This pre-configures the password so the
setup code flow is skipped.

```bash
# Fly.io
fly secrets set ANTRE_PASSWORD="your-secure-password"

```

For Render and DigitalOcean, set the variable in the dashboard's environment
settings.

## Health checks

All provider configs use the `/health` endpoint which returns HTTP 200 when
the gateway is ready. Configure your provider's health check to use:

- **Path**: `/health`
- **Method**: `GET`
- **Expected status**: `200`
