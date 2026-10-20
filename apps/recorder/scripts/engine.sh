#!/usr/bin/env bash
# Builds recordem-desktop (the engine the app records with, also the Desktop SDK's executable) and puts it where Tauri
# bundles sidecars from (src-tauri/binaries/<name>-<target triple>), next to the Parakeet helper.
#   apps/recorder/scripts/engine.sh [--debug]
set -euo pipefail
cd "$(dirname "$0")/.."
PROFILE=release; FLAGS=(--release)
[ "${1:-}" = "--debug" ] && { PROFILE=debug; FLAGS=(); }
cargo build "${FLAGS[@]}" -p recordem-desktop-engine --bin recordem-desktop
mkdir -p src-tauri/binaries
cp "target/$PROFILE/recordem-desktop" "src-tauri/binaries/recordem-desktop-$(rustc -vV | sed -n 's/^host: //p')"
