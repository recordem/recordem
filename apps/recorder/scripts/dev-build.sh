#!/usr/bin/env bash
# Dev build on a Mac, also over SSH: builds Recordem.app, signs it with a local dev certificate, and installs it in
# /Applications (unless a recording is in progress). A stable signature is what makes macOS keep the permissions
# (Microphone, System Audio, Accessibility, Automation) across rebuilds; an ad-hoc one changes with every build.
# It is "Recordem Dev" (dev.recordem.recorder.dev), an app of its own next to the release: macOS ties each permission to
# one signer, so a dev build installed over the release would leave the release's permissions pointing at the wrong
# certificate (on in System Settings, refused in practice). It keeps its own ~/Library/Application Support/Recordem Dev
# (config, sign-in, recordings) and links (recordem-dev://), so it points at the RECORDEM_*_URL server given here while
# the release keeps its own. The release is quit while the dev build runs: both would record the same call.
#
# Once per Mac: `scripts/dev-build.sh --setup` makes a self-signed "Recordem Dev" code-signing certificate in a keychain
# of its own (~/Library/Keychains/recordem-dev.keychain-db, password below: it guards nothing but this certificate),
# so signing works from SSH without the login keychain's password. Releases are signed by scripts/release.sh instead.
#
#   RECORDEM_API_URL=… RECORDEM_AUTH_URL=… RECORDEM_WEB_URL=… apps/recorder/scripts/dev-build.sh
set -euo pipefail

KC="$HOME/Library/Keychains/recordem-dev.keychain-db"
KC_PASS="recordem-dev"
NAME="Recordem Dev"
export PATH="$HOME/.cargo/bin:$HOME/Library/Python/3.9/bin:/opt/homebrew/bin:$PATH"

if [ "${1:-}" = "--setup" ]; then
  d="$(mktemp -d)"; trap 'rm -rf "$d"' EXIT
  /usr/bin/openssl req -x509 -newkey rsa:2048 -keyout "$d/key.pem" -out "$d/cert.pem" -days 3650 -nodes -subj "/CN=$NAME" \
    -addext "extendedKeyUsage=codeSigning" -addext "keyUsage=critical,digitalSignature" -addext "basicConstraints=critical,CA:false"
  /usr/bin/openssl pkcs12 -export -out "$d/dev.p12" -inkey "$d/key.pem" -in "$d/cert.pem" -passout pass:x -name "$NAME"
  [ -f "$KC" ] || security create-keychain -p "$KC_PASS" "$KC"
  security unlock-keychain -p "$KC_PASS" "$KC"
  security set-keychain-settings "$KC"          # no auto-lock
  security import "$d/dev.p12" -k "$KC" -P x -T /usr/bin/codesign
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KC_PASS" "$KC" >/dev/null
  # Keep the existing keychains in the search list, add this one.
  current=(); while IFS= read -r k; do current+=("$k"); done < <(security list-keychains -d user | tr -d ' "')   # bash 3.2: no mapfile
  security list-keychains -d user -s "${current[@]}" "$KC"
  echo "✓ $NAME ready"; exit 0
fi

cd "$(dirname "$0")/../src-tauri"
IDENTITY="-"
if [ -f "$KC" ]; then
  security unlock-keychain -p "$KC_PASS" "$KC"
  IDENTITY="$(security find-identity -p codesigning "$KC" | sed -n "s/.*) \([0-9A-F]\{40\}\) \"$NAME\".*/\1/p" | head -1)"
fi
[ "$IDENTITY" = "-" ] && echo "! no dev certificate (run --setup): ad-hoc signature, permissions reset on every build" >&2
../parakeet/build.sh   # the Parakeet engine's helper, bundled next to the app's executable
RECORDEM_APP="Recordem Dev" RECORDEM_SCHEME=recordem-dev APPLE_SIGNING_IDENTITY="$IDENTITY" cargo tauri build --bundles app \
  --config '{"productName": "Recordem Dev", "identifier": "dev.recordem.recorder.dev", "bundle": {"icon": ["icons/dev/32x32.png", "icons/dev/128x128.png", "icons/dev/128x128@2x.png", "icons/dev/icon.icns"]}, "plugins": {"deep-link": {"desktop": {"schemes": ["recordem-dev"]}}}}'

REC="$HOME/Library/Application Support/Recordem Dev/recordings"
# A recording in progress writes its WAVs every 100 ms; one left unsent (offline, a crash) is not in progress.
if [ -d "$REC" ] && [ -n "$(find "$REC" -name '*.wav' -mtime -15s 2>/dev/null | head -1)" ]; then
  echo "a recording is in progress: not installing (target/release/bundle/macos/Recordem Dev.app)"; exit 0
fi
APP="/Applications/Recordem Dev.app"
for a in Recordem "Recordem Dev"; do osascript -e "quit app \"$a\"" 2>/dev/null || true; done
sleep 2; pkill -f "/Applications/Recordem.app" || true; pkill -f "$APP" || true
rm -rf "$APP" && cp -R "target/release/bundle/macos/Recordem Dev.app" /Applications/
open -a "$APP"
echo "✓ installed $APP ($IDENTITY); the release is quit — open it again when done"
