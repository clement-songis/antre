# Deploy Antre on a VPS

Run your own AI agent on a $5/month VPS. This guide covers provisioning,
installation, and connecting channels (Telegram, Discord, etc.) so you can
talk to your agent from anywhere.

## Prerequisites

- A VPS with at least 1 GB RAM and 10 GB disk (any provider: Hetzner,
  DigitalOcean, Linode, Vultr, etc.)
- SSH access to the server
- An API key from at least one LLM provider (Anthropic, OpenAI, etc.)

## Option A: Docker (recommended)

Docker is the fastest path. It handles sandbox isolation and upgrades via image
pulls. For browser-trusted TLS on a VPS, put Antre behind a reverse proxy
such as Caddy, nginx, or Traefik and let the proxy manage public certificates.

### 1. Install Docker

```bash
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER
# Log out and back in for group membership to take effect
```

### 2. Deploy Antre

```bash
mkdir -p ~/antre && cd ~/antre
curl -fsSL https://raw.githubusercontent.com/antre-ai/antre/main/deploy/docker-compose.yml -o docker-compose.yml

# Set your password
export ANTRE_PASSWORD="your-secure-password"

# Start
docker compose up -d
```

### 3. Access the web UI

For a production VPS, use a domain name and terminate TLS at a reverse proxy:

```yaml
services:
  antre:
    image: ghcr.io/antre-ai/antre:latest
    command: ["--bind", "0.0.0.0", "--port", "13131", "--no-tls"]
    environment:
      - ANTRE_BEHIND_PROXY=true
      - ANTRE_EXTERNAL_URL=https://chat.example.com
```

Point your proxy at `http://<antre-host>:13131`, then open your public URL,
for example `https://chat.example.com`.

Antre can also auto-generate a local CA and server certificate, but that mode
is meant for local development or private networks. Trusting the generated CA
only makes the issuer trusted; the browser still requires the certificate's
Subject Alternative Name (SAN) to match the exact hostname or IP you open.
An IP-address URL such as `https://<your-server-ip>:13131` only works if the
certificate contains that IP address as an IP SAN, and normal public TLS setups
are domain-name based. Inside Docker, Antre usually cannot know your VPS public
IP or provider domain, so direct IP access may fail with a certificate name
mismatch even after importing the CA. To use direct IP access with the
auto-generated certificate, set the public IP explicitly:

```toml
[tls]
public_ip = "203.0.113.10"
```

Then restart Antre so it regenerates the server certificate and import the
generated CA from `http://<your-server-ip>:13132/certs/ca.pem`. If you want
Antre to serve HTTPS directly on a public domain, configure `tls.cert_path` and
`tls.key_path` with a certificate issued for that domain.

Log in with the password you set, then configure your LLM provider in
Settings.

## Option B: Binary + systemd

For servers without Docker, install the binary directly.

### 1. Download the binary

```bash
# Replace VERSION with the latest release (e.g. 20260420.01)
VERSION=$(curl -s https://api.github.com/repos/antre-ai/antre/releases/latest | grep tag_name | cut -d '"' -f 4)
ARCH=$(uname -m | sed 's/x86_64/x86_64/;s/aarch64/aarch64/')

curl -fsSL "https://github.com/antre-ai/antre/releases/download/${VERSION}/antre-${VERSION}-linux-${ARCH}.tar.gz" | sudo tar xz -C /usr/local/bin
```

### 2. Create user and directories

```bash
sudo useradd -r -m -d /var/lib/antre -s /usr/sbin/nologin antre
sudo mkdir -p /etc/antre
sudo chown antre:antre /var/lib/antre /etc/antre
```

### 3. Install the systemd service

```bash
sudo curl -fsSL https://raw.githubusercontent.com/antre-ai/antre/main/deploy/antre.service -o /etc/systemd/system/antre.service
```

The bundled unit keeps `NoNewPrivileges=true` and `ProtectHome=true` as secure
defaults. If Antre uses rootless Podman, install the Podman-specific override:

```bash
sudo mkdir -p /etc/systemd/system/antre.service.d
sudo curl -fsSL https://raw.githubusercontent.com/antre-ai/antre/main/deploy/antre-podman.conf -o /etc/systemd/system/antre.service.d/podman.conf
```

The override relaxes only the directives that block Podman's user namespace
re-exec path and places its writable home and runtime directories under paths
allowed by the unit. Rootless Podman also requires an unused subordinate UID and
GID range. Check `/etc/subuid` and `/etc/subgid`; if the `antre` user has none,
assign a non-overlapping range before starting the service, for example:

```bash
sudo usermod --add-subuids 200000-265535 --add-subgids 200000-265535 antre
```

If `allow_host_podman = true`, install the companion API service before Antre.
It runs as `antre` and creates `/run/antre/podman/podman.sock`, matching the
drop-in's `XDG_RUNTIME_DIR`:

```bash
sudo curl -fsSL https://raw.githubusercontent.com/antre-ai/antre/main/deploy/antre-podman-api.service -o /etc/systemd/system/antre-podman-api.service
sudo systemctl daemon-reload
sudo systemctl enable --now antre-podman-api.service
```

Then enable and start Antre:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now antre
```

### 4. Set your password

```bash
sudo -u antre ANTRE_DATA_DIR=/var/lib/antre ANTRE_CONFIG_DIR=/etc/antre antre auth reset-password
```

### 5. Check status

```bash
sudo systemctl status antre
sudo journalctl -u antre -f
```

## Connecting channels

Once Antre is running, add messaging channels from Settings > Channels in
the web UI. Each channel has its own setup flow:

| Channel | What you need |
|---------|--------------|
| Telegram | Bot token from [@BotFather](https://t.me/BotFather) |
| Discord | Bot token from the [Developer Portal](https://discord.com/developers) |
| Slack | Bot + App tokens from [api.slack.com](https://api.slack.com/apps) |
| Matrix | Homeserver URL + credentials |
| Nostr | Secret key (nsec) + relay URLs |

See the individual [channel docs](channels.md) for detailed setup
instructions.

## Firewall

If you use the recommended reverse proxy setup, expose only the proxy ports
(`80` and `443`) publicly and keep Antre' `13131` port private to the host or
container network.

Only expose `13131` directly if you intentionally serve Antre without a proxy.
Only expose `13132` if you intentionally use Antre' local CA download endpoint.

```bash
# UFW example for the recommended reverse proxy setup
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
```

## Upgrades

**Docker:** `docker compose pull && docker compose up -d`

**Binary:** Download the new release binary and restart the service:
```bash
sudo systemctl stop antre
# Download new binary (same curl as step 1)
sudo systemctl start antre
```

## Resource requirements

| Workload | RAM | CPU | Disk |
|----------|-----|-----|------|
| Chat only (no sandbox) | 512 MB | 1 vCPU | 5 GB |
| Chat + sandbox | 1 GB | 1 vCPU | 10 GB |
| Chat + sandbox + local LLM | 4+ GB | 2+ vCPU | 20+ GB |

LLM inference happens on the provider's API servers, so even a $5 VPS handles
chat workloads with external providers.
