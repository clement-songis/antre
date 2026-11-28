#!/usr/bin/env bash
# Build "Antre Desktop.app" (the gpui client in apps/desktop) with the antre
# daemon inside, so the app needs nothing else installed: when no daemon is
# running, it installs the bundled one as a user service.
#
# Usage: scripts/package-desktop-macos.sh [OUTPUT_DIR]
#
# Environment:
#   ANTRE_VERSION          version for Info.plist and both binaries
#                           (default 0.1.0)
#   DESKTOP_SIGN_IDENTITY   codesign identity, e.g. "Developer ID Application".
#                           Signs with the hardened runtime and a timestamp, as
#                           notarization requires. Unset: ad-hoc (local only).
#   DESKTOP_UNIVERSAL=1     build arm64 + x86_64 and merge them with lipo
#   DESKTOP_TARGET          one cargo target triple (default: host); ignored
#                           with DESKTOP_UNIVERSAL
#   DESKTOP_DAEMON=0        leave the daemon out (the app then needs `antre`
#                           on PATH)
#   DESKTOP_DAEMON_BIN      ship this antre binary instead of building one
#   DESKTOP_DAEMON_FEATURES daemon cargo features (default "portable"; release
#                           CI adds "embedded-wasm" after building the WASM
#                           components, since the bundle has no share dir)
#
# The bundle is also required for system notifications on macOS.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/apps/desktop/target/bundle}"
VERSION="${ANTRE_VERSION:-0.1.0}"
BUNDLE_ID="ai.antre.desktop"
APP="$OUT/Antre Desktop.app"
ICONS="$ROOT/apps/macos/Assets.xcassets/AppIcon.appiconset"
DAEMON_ENTITLEMENTS="$ROOT/apps/desktop/daemon.entitlements"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS bundles can only be built on macOS" >&2
  exit 1
fi

targets=()
if [[ "${DESKTOP_UNIVERSAL:-0}" == "1" ]]; then
  targets=(aarch64-apple-darwin x86_64-apple-darwin)
elif [[ -n "${DESKTOP_TARGET:-}" ]]; then
  targets=("$DESKTOP_TARGET")
fi

# Path of a release binary for one target ("" = host).
release_path() {
  local target_dir="$1" target="$2" name="$3"
  if [[ -n "$target" ]]; then
    printf '%s/%s/release/%s' "$target_dir" "$target" "$name"
  else
    printf '%s/release/%s' "$target_dir" "$name"
  fi
}

# Build `name` for every target and leave one (possibly universal) binary at
# the output path. The remaining arguments are the build command.
build_binary() {
  local label="$1" workdir="$2" target_dir="$3" output="$4" name="$5"
  shift 5
  local built=()
  if [[ "${#targets[@]}" -eq 0 ]]; then
    echo "==> Building $label (release)"
    (cd "$workdir" && "$@" --release --locked)
    built+=("$(release_path "$target_dir" "" "$name")")
  else
    for target in "${targets[@]}"; do
      echo "==> Building $label (release, $target)"
      (cd "$workdir" && "$@" --release --locked --target "$target")
      built+=("$(release_path "$target_dir" "$target" "$name")")
    done
  fi
  if [[ "${#built[@]}" -gt 1 ]]; then
    lipo -create "${built[@]}" -output "$output"
  else
    cp "${built[0]}" "$output"
  fi
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

export ANTRE_VERSION="$VERSION"
build_binary antre-desktop "$ROOT/apps/desktop" "$ROOT/apps/desktop/target" \
  "$work/antre-desktop" antre-desktop cargo build

daemon=""
if [[ -n "${DESKTOP_DAEMON_BIN:-}" ]]; then
  daemon="$DESKTOP_DAEMON_BIN"
elif [[ "${DESKTOP_DAEMON:-1}" != "0" ]]; then
  export ANTRE_BUILD_FEATURES="${DESKTOP_DAEMON_FEATURES:-portable}"
  build_binary "antre daemon ($ANTRE_BUILD_FEATURES)" "$ROOT" "$ROOT/target" \
    "$work/antre" antre "$ROOT/scripts/cargo-build-antre.sh"
  daemon="$work/antre"
fi

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$work/antre-desktop" "$APP/Contents/MacOS/antre-desktop"
if [[ -n "$daemon" ]]; then
  echo "==> Bundling daemon"
  cp "$daemon" "$APP/Contents/MacOS/antre"
else
  echo "==> No daemon bundled: the app will need antre on PATH"
fi

# Icon set from the existing app icon PNGs.
iconset="$work/Antre.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  cp "$ICONS/icon-$size.png" "$iconset/icon_${size}x${size}.png"
  double=$((size * 2))
  if [[ -f "$ICONS/icon-$double.png" ]]; then
    cp "$ICONS/icon-$double.png" "$iconset/icon_${size}x${size}@2x.png"
  fi
done
iconutil -c icns "$iconset" -o "$APP/Contents/Resources/Antre.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Antre</string>
  <key>CFBundleDisplayName</key><string>Antre</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>antre-desktop</string>
  <key>CFBundleIconFile</key><string>Antre</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSLocalNetworkUsageDescription</key>
  <string>Antre finds antre daemons on your local network.</string>
  <key>NSBonjourServices</key>
  <array><string>_antre._tcp</string></array>
</dict>
</plist>
PLIST

if [[ -n "${DESKTOP_SIGN_IDENTITY:-}" ]]; then
  echo "==> Signing with $DESKTOP_SIGN_IDENTITY"
  sign_args=(--force --options runtime --timestamp --sign "$DESKTOP_SIGN_IDENTITY")
else
  echo "==> Ad-hoc signing (set DESKTOP_SIGN_IDENTITY for distribution)"
  sign_args=(--force --sign -)
fi
# Nested executables are signed before the bundle that seals them. The daemon
# runs WASM tools through wasmtime, which needs executable memory under the
# hardened runtime.
if [[ -f "$APP/Contents/MacOS/antre" ]]; then
  codesign "${sign_args[@]}" --entitlements "$DAEMON_ENTITLEMENTS" \
    "$APP/Contents/MacOS/antre"
fi
codesign "${sign_args[@]}" "$APP"
codesign --verify --strict --deep "$APP"

echo "==> Built $APP"
