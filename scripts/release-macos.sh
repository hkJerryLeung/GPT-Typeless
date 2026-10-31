#!/bin/bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
signing_id=${APPLE_SIGNING_IDENTITY:-}
if [[ ! "$signing_id" =~ ^[0-9A-F]{40}$ ]]; then
  echo "Set APPLE_SIGNING_IDENTITY to the Developer ID certificate SHA-1; ad-hoc signing is not supported." >&2
  exit 1
fi
identities=$(security find-identity -v -p codesigning)
if [[ "$identities" != *"$signing_id"* ]]; then
  echo "The configured code-signing identity is unavailable. Restore it before building an update." >&2
  exit 1
fi

if [[ -z "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" || -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]]; then
  echo "Set TAURI_SIGNING_PRIVATE_KEY_PATH and TAURI_SIGNING_PRIVATE_KEY_PASSWORD to sign updater artifacts." >&2
  exit 1
fi

# Tauri's build command reads this variable as a key string or a file path.
export TAURI_SIGNING_PRIVATE_KEY="$TAURI_SIGNING_PRIVATE_KEY_PATH"

node node_modules/@tauri-apps/cli/tauri.js build --bundles app
bundle="src-tauri/target/release/bundle/macos/GPT-Typeless.app"
requirement="identifier com.gpttypeless and certificate leaf = H\"$signing_id\""
codesign --verify --deep --strict -R="$requirement" "$bundle"
designated=$(codesign -d -r- "$bundle" 2>&1)
if [[ "$designated" == *"cdhash "* ]]; then
  echo "The bundle has a version-specific ad-hoc requirement; refusing this release." >&2
  exit 1
fi
echo "Release verified with the fixed signing identity."
