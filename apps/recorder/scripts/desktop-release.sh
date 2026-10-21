#!/usr/bin/env bash
# Builds the Desktop SDK's executables (recordem-desktop and its Parakeet helper), signs them with the Developer ID
# (hardened runtime, the app's entitlements: microphone), notarizes them and writes the files a GitHub release
# desktop-v<version> carries: <name>-darwin-arm64 for each, and SHA256SUMS, which @recordem/desktop-sdk's engine.json
# is filled from at publish time. A host app re-signs them with its own certificate when it bundles them.
# Run on a Mac with the "Developer ID Application" certificate in a keychain (CI: .github/workflows/publish.yml).
#   APPLE_ID=… APPLE_APP_PASSWORD=… APPLE_TEAM_ID=… apps/recorder/scripts/desktop-release.sh
# (or NOTARY_PROFILE, as in release.sh)
set -euo pipefail

SCRIPTS="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPTS/.."
if [ -n "${APPLE_ID:-}" ]; then
  NOTARY=(--apple-id "$APPLE_ID" --password "${APPLE_APP_PASSWORD:?}" --team-id "${APPLE_TEAM_ID:?}")
else
  NOTARY=(--keychain-profile "${NOTARY_PROFILE:-recordem-notary}")
fi
IDENTITY="${SIGNING_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)}"
[ -n "$IDENTITY" ] || { echo "no 'Developer ID Application' certificate in the keychain" >&2; exit 1; }
# Same reason as release.sh: whisper.cpp must not tune for the build machine.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"

TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
[ "$TRIPLE" = "aarch64-apple-darwin" ] || { echo "the engine ships for Apple silicon only (host: $TRIPLE)" >&2; exit 1; }
OUT="target/release/desktop-dist"
rm -rf "$OUT" && mkdir -p "$OUT"

parakeet/build.sh
scripts/engine.sh
for name in recordem-desktop recordem-parakeet; do
  cp "src-tauri/binaries/$name-$TRIPLE" "$OUT/$name-darwin-arm64"
  codesign --force --sign "$IDENTITY" --timestamp --options runtime --entitlements src-tauri/Entitlements.plist \
    --identifier "app.recordem.$name" "$OUT/$name-darwin-arm64"
done

# A bare executable can't be stapled: notarizing records it with Apple, and Gatekeeper checks online.
(cd "$OUT" && /usr/bin/zip -q notarize.zip recordem-desktop-darwin-arm64 recordem-parakeet-darwin-arm64)
xcrun notarytool submit "$OUT/notarize.zip" "${NOTARY[@]}" --wait --timeout 30m | tee "$OUT/notary.log"
grep -q "status: Accepted" "$OUT/notary.log" || { echo "notarization failed: see $OUT/notary.log" >&2; exit 1; }
rm "$OUT/notarize.zip"

(cd "$OUT" && shasum -a 256 recordem-desktop-darwin-arm64 recordem-parakeet-darwin-arm64 > SHA256SUMS)
cat "$OUT/SHA256SUMS"
