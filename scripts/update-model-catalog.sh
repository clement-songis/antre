#!/usr/bin/env bash
# Refresh the bundled models.dev snapshot (whole file, validated, compressed).
#   ./scripts/update-model-catalog.sh                 # refresh in place
#   ./scripts/update-model-catalog.sh --diff-out FILE # also write a Markdown diff
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
url="${MODELS_DEV_URL:-https://models.dev/api.json}"
snapshot="$repo_root/crates/model-catalog/data/models-dev.json.gz"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

diff_out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --diff-out) diff_out="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

curl -fsSL --max-time 120 -o "$work/api.json" "$url"
cp "$snapshot" "$work/previous.json.gz"
summary="$(cargo run --quiet -p moltis-model-catalog --bin pack-models-dev -- \
  "$work/api.json" "$snapshot" --diff "$work/previous.json.gz")"
echo "$summary"
if [ -n "$diff_out" ]; then
  printf '%s\n' "$summary" > "$diff_out"
fi
