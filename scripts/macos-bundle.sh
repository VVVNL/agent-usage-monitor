#!/usr/bin/env bash
# Build "Agent Usage Monitor.app" (universal: Apple Silicon + Intel) and a DMG. Run on macOS.
#
#   scripts/macos-bundle.sh <version>
#
# Signing is ad-hoc unless you provide a Developer ID:
#   CODESIGN_IDENTITY="Developer ID Application: Name (TEAMID)"
# Notarization runs only if all of these are set:
#   NOTARY_APPLE_ID, NOTARY_TEAM_ID, NOTARY_PASSWORD   (an app-specific password)
set -euo pipefail

VERSION="${1:?usage: macos-bundle.sh <version>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/dist"
APP="$OUT/Agent Usage Monitor.app"
DMG="$OUT/agent-usage-monitor-$VERSION-macos-universal.dmg"

rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release -p aum-app --locked --target aarch64-apple-darwin
cargo build --release -p aum-app --locked --target x86_64-apple-darwin

rm -rf "$APP" "$DMG"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/agent-usage-monitor" \
  target/aarch64-apple-darwin/release/agent-usage-monitor \
  target/x86_64-apple-darwin/release/agent-usage-monitor
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist.in > "$APP/Contents/Info.plist"
cp LICENSE README.md "$APP/Contents/Resources/"
cp packaging/macos/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
[ -f docs/THIRD_PARTY.md ] && cp docs/THIRD_PARTY.md "$APP/Contents/Resources/"

IDENTITY="${CODESIGN_IDENTITY:--}"
codesign --force --deep --options runtime --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict "$APP"

STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Agent Usage Monitor" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"

if [ -n "${NOTARY_APPLE_ID:-}" ] && [ -n "${NOTARY_TEAM_ID:-}" ] && [ -n "${NOTARY_PASSWORD:-}" ] && [ "$IDENTITY" != "-" ]; then
  codesign --force --sign "$IDENTITY" "$DMG"
  xcrun notarytool submit "$DMG" --apple-id "$NOTARY_APPLE_ID" --team-id "$NOTARY_TEAM_ID" \
    --password "$NOTARY_PASSWORD" --wait
  xcrun stapler staple "$DMG"
  echo "Notarized and stapled: $DMG"
else
  echo "Not notarized (no Developer ID / notary credentials). First launch needs right-click > Open."
fi

(cd "$OUT" && shasum -a 256 "$(basename "$DMG")" > "SHA256SUMS-macos.txt")
echo "Built: $DMG"
