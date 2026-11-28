# Installation

Antre is distributed as a single self-contained binary. Choose the installation method that works best for your setup.

## Quick Install (Recommended)

The fastest way to get started on macOS or Linux:

```bash
curl -fsSL https://antre-ai.github.io/install.sh | sh
```

This downloads the latest release for your platform and installs it to `~/.local/bin`.

## Package Managers

### Homebrew (macOS / Linux)

```bash
brew install antre-ai/tap/antre
```

## Linux Packages

Package filenames are versioned on every release. Use the installer script below instead of hardcoding GitHub release asset names.

### Debian / Ubuntu (.deb)

```bash
# Install the latest .deb package
curl -fsSL https://antre-ai.github.io/install.sh | sh -s -- --method=deb
```

### Fedora / RHEL (.rpm)

```bash
# Install the latest .rpm package
curl -fsSL https://antre-ai.github.io/install.sh | sh -s -- --method=rpm
```

### Arch Linux (.pkg.tar.zst)

```bash
# Install the latest package
curl -fsSL https://antre-ai.github.io/install.sh | sh -s -- --method=arch
```

### Snap

```bash
sudo snap install antre
```

### AppImage

```bash
# Install the latest AppImage
curl -fsSL https://antre-ai.github.io/install.sh | sh -s -- --method=appimage
```

## Docker

Multi-architecture images (amd64/arm64) are published to GitHub Container Registry:

```bash
docker pull ghcr.io/antre-ai/antre:latest
```

See [Docker Deployment](docker.md) for full instructions on running Antre in a container.

## Build from Source

### Prerequisites

- Rust 1.91 or later
- A C compiler (for some dependencies)
- [just](https://github.com/casey/just) (command runner)
- Node.js (for building Tailwind CSS)

### Clone and Build

```bash
git clone https://github.com/antre-ai/antre.git
cd antre
just build-css           # Build Tailwind CSS for the web UI
just build-release       # Build in release mode
```

For a full release build including WASM sandbox tools:

```bash
just build-release-with-wasm
```

The binary will be at `target/release/antre`.

### Install via Cargo

```bash
cargo install antre --git https://github.com/antre-ai/antre
```

## First Run

After installation, start Antre:

```bash
antre
```

On first launch, when you run it from a terminal on your desktop, Antre opens
the web UI in your browser at `http://localhost:<port>/onboarding`. The
onboarding page walks you through securing the instance, configuring an LLM
provider, and picking a name for your agent.

- **The setup code is filled in for you.** The link Antre opens carries it in
  the URL fragment (`#setup=…`), which browsers never send to the server. The
  page removes it from the address bar after reading it.
- **No certificate warning.** On a loopback bind (the default), new installs
  serve plain HTTP. Browsers treat `http://localhost` as a secure context, so
  passkeys work without trusting a certificate. See
  [TLS / HTTPS](configuration.md#tls--https).
- **The URL stays visible.** In an interactive terminal, a status line pinned
  below the logs shows the URL and setup code until onboarding is finished,
  then switches to `● Antre running at …`. Without a terminal (Docker, a
  service, piped output), Antre prints a "next step" box after startup and
  repeats the URL in the log every minute until onboarding is finished.

Antre never opens a browser over SSH, in a container, in CI, or under a
service manager. Over SSH it prints the `ssh -L` command that forwards the port
to your computer instead. Use `--no-open-browser`, `--open-browser`, or
[`server.open_browser`](configuration.md#opening-the-browser-on-start) to
change this.

```admonish tip
Antre picks a random available port on first install to avoid conflicts. The port is saved in your config and reused on subsequent runs.
```

```admonish note
Authentication is only required when accessing Antre from a non-localhost address (e.g., over the network). When this happens, a one-time setup code is printed to the terminal for initial authentication setup.
```

## Verify Installation

```bash
antre --version
```

## Updating

### Homebrew

```bash
brew upgrade antre
```

### From Source

```bash
cd antre
git pull
just build-css
just build-release
```

## Uninstalling

### Homebrew

```bash
brew uninstall antre
```

### Remove Data

Antre stores data in two directories:

```bash
# Configuration
rm -rf ~/.config/antre

# Data (sessions, databases, memory)
rm -rf ~/.antre
```

```admonish warning
Removing these directories deletes all your conversations, memory, and settings permanently.
```
