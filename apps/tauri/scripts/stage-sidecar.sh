#!/usr/bin/env bash
# Build the release `antre` daemon and stage it where Tauri's bundler picks
# it up as a sidecar (`binaries/antre-<target triple>`). The bundle installs
# it next to the shell executable, where the shell looks for it first.
#
#   apps/tauri/scripts/stage-sidecar.sh [extra cargo build args]
#   cd apps/tauri && cargo tauri build --config tauri.bundle.conf.json
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
root="$(cd "$here/../.." && pwd)"
triple="$(rustc -vV | sed -n 's/^host: //p')"
exe=""
[[ "$triple" == *windows* ]] && exe=".exe"

(cd "$root" && cargo build --release -p antre "$@")
target_dir="${CARGO_TARGET_DIR:-$root/target}"
mkdir -p "$here/binaries"
cp "$target_dir/release/antre$exe" "$here/binaries/antre-$triple$exe"
echo "staged binaries/antre-$triple$exe"
