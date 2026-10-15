#!/usr/bin/env bash
# Builds the Parakeet helper and puts it where Tauri bundles sidecars from (src-tauri/binaries/<name>-<target triple>).
set -euo pipefail
cd "$(dirname "$0")"
swift build -c release --product recordem-parakeet
mkdir -p ../src-tauri/binaries
cp .build/release/recordem-parakeet "../src-tauri/binaries/recordem-parakeet-$(rustc -vV | sed -n 's/^host: //p')"
