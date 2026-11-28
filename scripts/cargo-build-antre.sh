#!/usr/bin/env bash
set -euo pipefail

features="${ANTRE_BUILD_FEATURES:-full}"

cargo build -p antre --no-default-features --features "$features" "$@"
