# Antre Deployment Templates

This directory contains templates for deploying Antre on a VPS or bare-metal server.

## Files

| File | Purpose |
|------|---------|
| `docker-compose.yml` | Docker Compose for VPS deployment |
| `antre.service` | systemd unit file for bare-metal installs |
| `antre-podman.conf` | systemd override for rootless Podman |
| `antre-podman-api.service` | Optional host socket service for sandbox passthrough |

## Docker Compose (recommended)

```bash
cd deploy
export ANTRE_PASSWORD="your-secure-password"
docker compose up -d
```

Open `https://<your-server-ip>:13131` and configure your LLM provider.

## Systemd (bare-metal)

```bash
# Create user and directories. /var/lib/antre is also its Podman-compatible home.
sudo useradd -r -m -d /var/lib/antre -s /usr/sbin/nologin antre
sudo mkdir -p /etc/antre
sudo chown antre:antre /var/lib/antre /etc/antre

# Install the binary
sudo cp antre /usr/local/bin/antre

# Install the service
sudo cp deploy/antre.service /etc/systemd/system/
```

The default unit retains systemd privilege and home-directory hardening. If
Antre uses rootless Podman, install the required override before starting it:

```bash
sudo mkdir -p /etc/systemd/system/antre.service.d
sudo cp deploy/antre-podman.conf /etc/systemd/system/antre.service.d/podman.conf
# Assign an unused subordinate ID range if antre has none in /etc/subuid and /etc/subgid.
# sudo usermod --add-subuids 200000-265535 --add-subgids 200000-265535 antre
```

If `allow_host_podman = true`, also install and enable the API service before
Antre. It creates the socket at the path configured by the drop-in:

```bash
sudo cp deploy/antre-podman-api.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now antre-podman-api.service
```

Then start the service:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now antre
```

See [docs.moltis.org/deploy-vps](https://docs.moltis.org/deploy-vps.html) for
a complete walkthrough.
