#!/usr/bin/env bash
# Build a plain, distributable .dmg from the bundled .app.
#
# Tauri's own DMG step shells out to `create-dmg`, which drives Finder through
# AppleScript to lay out the window. That needs a GUI session with accessibility
# permission and fails outright without one (CI, ssh, a sandboxed terminal).
# `hdiutil` needs neither, so this always works — it just looks plainer.
set -euo pipefail

# BUNDLE_DIR / ARCH can be overridden, e.g. for a universal build:
#   BUNDLE_DIR=src-tauri/target/universal-apple-darwin/release/bundle ARCH=universal
BUNDLE_DIR="${BUNDLE_DIR:-src-tauri/target/release/bundle}"
APP_DIR="$BUNDLE_DIR/macos"
APP_NAME="ShareLocal.app"
OUT_DIR="$BUNDLE_DIR/dmg"
VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
ARCH="${ARCH:-$(uname -m)}"
DMG="$OUT_DIR/ShareLocal_${VERSION}_${ARCH}.dmg"

if [ ! -d "$APP_DIR/$APP_NAME" ]; then
  echo "error: $APP_DIR/$APP_NAME does not exist — run 'npm run app:build' first." >&2
  exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cp -R "$APP_DIR/$APP_NAME" "$STAGE/"

# Sign the whole bundle. The linker only ad-hoc signs the bare executable, which
# leaves Info.plist and resources unsealed; once the DMG is downloaded on another
# Mac (quarantined), Gatekeeper rejects that as "is damaged and can't be opened".
# Defaults to an ad-hoc signature ("-"); set SIGN_IDENTITY to a
# "Developer ID Application: …" identity for a build that can be notarized.
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
xattr -cr "$STAGE/$APP_NAME"
if [ "$SIGN_IDENTITY" = "-" ]; then
  codesign --force --deep --sign - "$STAGE/$APP_NAME"
else
  codesign --force --deep --options runtime --timestamp --sign "$SIGN_IDENTITY" "$STAGE/$APP_NAME"
fi
codesign --verify --deep --strict "$STAGE/$APP_NAME"

# The conventional drag-to-install affordance.
ln -s /Applications "$STAGE/Applications"

mkdir -p "$OUT_DIR"
rm -f "$DMG"
hdiutil create \
  -volname "ShareLocal" \
  -srcfolder "$STAGE" \
  -ov -format UDZO \
  "$DMG" >/dev/null

echo "Built $DMG"
