# Release Verification

Antre releases use **multiple signing layers** to provide strong supply chain guarantees:

| Method | Proves | Verification |
|--------|--------|-------------|
| **GitHub Artifact Attestations** (CI-generated) | Artifact was built by this repo's GitHub Actions workflow | `gh attestation verify` |
| **Sigstore** (keyless, CI-generated) | Artifact was built by GitHub Actions from this repo | `cosign verify-blob` |
| **GPG** (YubiKey-resident key, maintainer-signed) | A specific maintainer authorized the release | `gpg --verify` |
| **SHA256/SHA512 checksums** | File integrity (no corruption/tampering in transit) | `sha256sum --check` |

All attestations are publicly visible on the
[repository attestations page](https://github.com/antre-ai/antre/attestations).

## macOS Release Credentials

The macOS app is signed with a Developer ID Application certificate and notarized
using a **team App Store Connect API key** from
[Users and Access > Integrations](https://appstoreconnect.apple.com/access/integrations/api).
No Apple ID password or app-specific password is required for notarization.

Maintainers configure these GitHub Actions repository secrets:

| Secret | Value |
|--------|-------|
| `APPLE_API_PRIVATE_KEY` | Raw multiline contents of the downloaded `.p8` file, not base64 or a path |
| `APPLE_API_KEY_ID` | Key ID from App Store Connect |
| `APPLE_API_ISSUER_ID` | Issuer UUID for the team that created the key |
| `APPLE_CERTIFICATE_BASE64` | Base64 Developer ID Application signing certificate (`.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | Password protecting the `.p12` |
| `APPLE_TEAM_ID` | Developer team ID used for code signing, not the API issuer UUID |

Keep a secure backup of the API key; Apple permits only one download. Never commit
it or expose it in build logs. `scripts/notarize-macos.sh` writes it to a private
temporary directory, removes it on exit, and requires an `Accepted` JSON status
and successful submission command before the workflow staples the app.
The API key does not replace the signing certificate. Individual API keys (without
a team issuer) are not supported by this release setup.

Run `bash scripts/test-notarize-macos.sh` for mocked authentication and failure-path
tests. Release dry runs skip signing and notarization, so they cannot validate
Apple credentials. Validate credentials separately with `xcrun notarytool history
--key /secure/path/AuthKey.p8 --key-id KEY_ID --issuer ISSUER_UUID`; a real signed-app
submission is still needed to verify end-to-end notarization. Retire legacy
`APPLE_ID` and `APPLE_ID_PASSWORD` secrets after the migrated release succeeds.

## Quick Verification

The easiest way to verify a release is with the included script:

```bash
# Verify all artifacts for a release (fetches GPG key automatically)
./scripts/verify-release.sh --version VERSION

# Also check SHA256 checksums
./scripts/verify-release.sh --checksums --version VERSION

# Verify specific local files
./scripts/verify-release.sh antre-VERSION-x86_64-unknown-linux-gnu.tar.gz
```

### GitHub Artifact Attestations

GitHub artifact attestations provide cryptographic proof that release artifacts
were built inside this repository's GitHub Actions workflow. Verification uses
the [GitHub CLI](https://cli.github.com/):

```bash
# Verify a downloaded binary
gh attestation verify antre-VERSION-x86_64-unknown-linux-gnu.tar.gz \
  -R antre-ai/antre

# Verify a Docker image
gh attestation verify oci://ghcr.io/antre-ai/antre:VERSION \
  -R antre-ai/antre

# Verify an SBOM
gh attestation verify antre-sbom.spdx.json \
  -R antre-ai/antre
```

Browse all attestations at
<https://github.com/antre-ai/antre/attestations>.

### Manual Verification

#### 1. Verify checksums

```bash
# Download the artifact and its checksum
curl -LO https://github.com/antre-ai/antre/releases/download/VERSION/antre-VERSION-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/antre-ai/antre/releases/download/VERSION/antre-VERSION-x86_64-unknown-linux-gnu.tar.gz.sha256

sha256sum --check antre-VERSION-x86_64-unknown-linux-gnu.tar.gz.sha256
```

#### 2. Verify GPG signature

The maintainer's GPG key fingerprint is:

```
3103 20A8 CC1C 5BA8 6AD0  9040 C045 1BAD F764 9BBF
```

```bash
# Import the maintainer's public key (one-time)
curl -fsSL https://pen.so/gpg.asc | gpg --import

# Confirm the fingerprint matches
gpg --fingerprint F7649BBF

# Download the detached signature
curl -LO https://github.com/antre-ai/antre/releases/download/VERSION/antre-VERSION-x86_64-unknown-linux-gnu.tar.gz.asc

# Verify
gpg --verify \
  antre-VERSION-x86_64-unknown-linux-gnu.tar.gz.asc \
  antre-VERSION-x86_64-unknown-linux-gnu.tar.gz
```

You should see `Good signature from ...` with the maintainer's identity.

#### 3. Verify Sigstore signature

```bash
# Install cosign: https://docs.sigstore.dev/cosign/system_config/installation/

# Download signature and certificate
curl -LO https://github.com/antre-ai/antre/releases/download/VERSION/antre-VERSION-x86_64-unknown-linux-gnu.tar.gz.sig
curl -LO https://github.com/antre-ai/antre/releases/download/VERSION/antre-VERSION-x86_64-unknown-linux-gnu.tar.gz.crt

cosign verify-blob \
  --signature antre-VERSION-x86_64-unknown-linux-gnu.tar.gz.sig \
  --certificate antre-VERSION-x86_64-unknown-linux-gnu.tar.gz.crt \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp 'https://github\.com/antre-ai/antre/' \
  antre-VERSION-x86_64-unknown-linux-gnu.tar.gz
```

#### 4. Verify Docker images

```bash
cosign verify \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp 'https://github\.com/antre-ai/antre/' \
  ghcr.io/antre-ai/antre:VERSION
```

## What Each Layer Proves

**Checksums** detect download corruption or CDN tampering. They do not prove
who created the file.

**GitHub artifact attestations** create unfalsifiable provenance records tied
to the repository, workflow, commit SHA, and triggering event. They are stored
in GitHub's attestation ledger and verifiable with `gh attestation verify`.
This provides SLSA v1.0 Build Level 2 guarantees.

**Sigstore signatures** prove the artifact was built inside the
`antre-ai/antre` GitHub Actions workflow using OIDC-based keyless signing.
This guards against a compromised maintainer laptop — even if someone steals
credentials, they cannot reproduce a valid Sigstore certificate from the CI
environment. Signatures are recorded in Sigstore's Rekor transparency log.

**GPG signatures** prove the release was reviewed and authorized by a specific
maintainer holding the corresponding private key. Because the key lives on a
YubiKey hardware token, compromise requires physical access to the device plus
the PIN.

Together, these layers create a strong chain: GitHub attestations and Sigstore
prove *where* the artifact was built (CI), and GPG proves *who* authorized it
(maintainer with hardware key).

## Release Artifacts Per File

Each release artifact (`.deb`, `.rpm`, `.tar.gz`, `.exe`, etc.) may have:

| Extension | Source | Description |
|-----------|--------|-------------|
| `.sha256` | CI | SHA-256 checksum |
| `.sha512` | CI | SHA-512 checksum |
| `.sig` | CI | Sigstore detached signature |
| `.crt` | CI | Sigstore signing certificate |
| `.asc` | Maintainer | GPG detached armored signature |

## For Maintainers: Signing a Release

After CI publishes a release, sign the artifacts locally using your
YubiKey-resident GPG key:

```bash
# Sign the latest release (prompts for YubiKey PIN/touch)
./scripts/gpg-sign-release.sh

# Sign a specific version
./scripts/gpg-sign-release.sh 20260331.01

# Use a specific key
./scripts/gpg-sign-release.sh --key 0xABCD1234 20260331.01

# Dry run (sign locally, don't upload)
./scripts/gpg-sign-release.sh --dry-run 20260331.01
```

The script:
1. Downloads all release packages from GitHub
2. Verifies their SHA256 checksums against CI-generated values
3. Creates `.asc` detached GPG signatures for each artifact
4. Uploads the `.asc` files to the GitHub release

### Prerequisites

- [GitHub CLI](https://cli.github.com/) (`gh`) authenticated
- GPG with your signing key available (YubiKey inserted)
- The `GPG_KEY_ID` environment variable or `--key` flag if you have
  multiple secret keys

### Publish Your Public Key

Upload your public GPG key so users can verify signatures:

```bash
# Export and upload to a keyserver
gpg --export --armor YOUR_KEY_ID | \
  curl -T - https://keys.openpgp.org

# Also add to your GitHub profile:
# Settings > SSH and GPG keys > New GPG key
gpg --export --armor YOUR_KEY_ID
```
