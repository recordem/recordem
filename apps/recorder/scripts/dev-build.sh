#!/usr/bin/env bash
# Dev build on a Mac, also over SSH: builds Recordem.app, signs it with a local dev certificate, and installs it in
# /Applications (unless a recording is in progress). A stable signature is what makes macOS keep the permissions
# (Microphone, System Audio, Accessibility, Automation) across rebuilds; an ad-hoc one changes with every build.
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
APPLE_SIGNING_IDENTITY="$IDENTITY" cargo tauri build --bundles app

REC="$HOME/Library/Application Support/Recordem/recordings"
for d in "$REC"/*/; do
  [ -d "$d" ] && [ ! -f "$d/sent" ] && { echo "a recording is in progress or unsent: not installing (target/release/bundle/macos/Recordem.app)"; exit 0; }
done
osascript -e 'quit app "Recordem"' 2>/dev/null || true
sleep 2; pkill -f "/Applications/Recordem.app" || true
rm -rf /Applications/Recordem.app && cp -R target/release/bundle/macos/Recordem.app /Applications/
open -a /Applications/Recordem.app
echo "✓ installed /Applications/Recordem.app ($IDENTITY)"
