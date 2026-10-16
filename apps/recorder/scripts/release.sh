#!/usr/bin/env bash
# Builds, signs (Developer ID) and notarizes the recorder, and makes the .dmg people install.
# Run on a Mac, in Terminal (codesign needs the login keychain unlocked, which an SSH session doesn't have).
#
# Once per Mac:
#   - "Developer ID Application" certificate in the login keychain (Xcode → Settings → Accounts → Manage Certificates)
#   - xcrun notarytool store-credentials recordem-notary --apple-id <you> --team-id <TEAM> --password <app-specific>
#   - cargo install tauri-cli --version "^2" --locked; cmake on PATH
# Each release: bump "version" in src-tauri/tauri.conf.json (and Cargo.toml), then
#   RECORDEM_API_URL=… RECORDEM_AUTH_URL=… RECORDEM_WEB_URL=… apps/recorder/scripts/release.sh
# (the addresses a fresh install points at; people can change them in the app's settings).
# APPLE_ID / APPLE_APP_PASSWORD / APPLE_TEAM_ID replace the stored notary profile (CI).
set -euo pipefail

SCRIPTS="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPTS/../src-tauri"
if [ -n "${APPLE_ID:-}" ]; then
  NOTARY=(--apple-id "$APPLE_ID" --password "${APPLE_APP_PASSWORD:?}" --team-id "${APPLE_TEAM_ID:?}")
else
  NOTARY=(--keychain-profile "${NOTARY_PROFILE:-recordem-notary}")
fi
IDENTITY="${SIGNING_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)}"
[ -n "$IDENTITY" ] || { echo "no 'Developer ID Application' certificate in the keychain" >&2; exit 1; }
VERSION="$(/usr/bin/python3 -c 'import json; print(json.load(open("tauri.conf.json"))["version"])')"
APP="target/release/bundle/macos/Recordem.app"
OUT="target/release/dist"
DMG="$OUT/Recordem-$VERSION.dmg"

# A release runs on any Apple Silicon Mac, so whisper.cpp must not tune for the build machine (-mcpu=native): ggml
# turns that off when SOURCE_DATE_EPOCH is set. On a CI VM, native also misreads the CPU (i8mm) and fails to build.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"

echo "→ Recordem $VERSION, signed by: $IDENTITY"
../parakeet/build.sh   # the Parakeet engine's helper, bundled next to the app's executable
APPLE_SIGNING_IDENTITY="$IDENTITY" cargo tauri build --bundles app

notarize() {   # $1: a .zip or .dmg
  xcrun notarytool submit "$1" "${NOTARY[@]}" --wait --timeout 30m | tee "$OUT/notary.log"
  grep -q "status: Accepted" "$OUT/notary.log" || { echo "notarization failed: see $OUT/notary.log" >&2; exit 1; }
}

rm -rf "$OUT" && mkdir -p "$OUT/stage"
echo "→ notarizing the app"
ditto -c -k --keepParent "$APP" "$OUT/app.zip"
notarize "$OUT/app.zip"
xcrun stapler staple "$APP"

echo "→ disk image"
ditto "$APP" "$OUT/stage/Recordem.app"
swift "$SCRIPTS/applications-alias.swift" "$OUT/stage"   # an alias with the folder icon, not a bare symlink
hdiutil create -volname "Recordem" -srcfolder "$OUT/stage" -ov -format UDZO "$DMG" >/dev/null
codesign --sign "$IDENTITY" --timestamp "$DMG"
notarize "$DMG"
xcrun stapler staple "$DMG"
spctl -a -t open --context context:primary-signature -v "$DMG"
echo "✓ $DMG"
