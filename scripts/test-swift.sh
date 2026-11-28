#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
MACOS_APP_DIR="${REPO_ROOT}/apps/macos"

if [ ! -d "${MACOS_APP_DIR}/Antre.xcodeproj" ]; then
  echo "error: Antre.xcodeproj does not exist. Run: just swift-generate" >&2
  exit 1
fi

if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
fi

xcodebuild \
  -project "${MACOS_APP_DIR}/Antre.xcodeproj" \
  -scheme Antre \
  -destination "platform=macOS" \
  test

echo "xcodebuild tests succeeded for Antre"
