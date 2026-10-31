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

# Paths can otherwise be embedded in Rust panic and tracing metadata. Cargo's
# encoded form preserves paths containing spaces and existing encoded flags.
if [[ -n "${RUSTFLAGS:-}" && -z "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
  echo "Use CARGO_ENCODED_RUSTFLAGS instead of RUSTFLAGS for release builds." >&2
  exit 1
fi
encoded_flags=${CARGO_ENCODED_RUSTFLAGS:-}
append_remap() {
  local prefix=$1 replacement=$2
  if [[ -n "$prefix" && "$prefix" != / ]]; then
    [[ -z "$encoded_flags" ]] || encoded_flags+=$'\x1f'
    encoded_flags+="--remap-path-prefix=$prefix=$replacement"
  fi
}
append_remap "$HOME" /redacted-home
append_remap "$PWD" /gpt-typeless-source
append_remap "${CARGO_HOME:-}" /cargo-home
append_remap "${RUSTUP_HOME:-}" /rustup-home
export CARGO_ENCODED_RUSTFLAGS="$encoded_flags"

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
python3 - "$bundle" "$HOME" "$PWD" "${CARGO_HOME:-}" "${RUSTUP_HOME:-}" <<'PY'
from pathlib import Path
import re
import sys

bundle = Path(sys.argv[1])
local_paths = [value.encode() for value in sys.argv[2:] if len(value) > 1]
patterns = (
    re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----"),
    re.compile(rb"sk-(?:proj-)?[A-Za-z0-9_-]{20,}"),
    re.compile(rb"AIza[A-Za-z0-9_-]{35}"),
    re.compile(rb"sk_(?:live|test)_[A-Za-z0-9]{16,}"),
    re.compile(rb"gh[pousr]_[A-Za-z0-9]{36,255}|github_pat_[A-Za-z0-9_]{22,}"),
    re.compile(rb"AKIA[A-Z0-9]{16}"),
    re.compile(rb"eyJ[A-Za-z0-9_-]{15,}\.[A-Za-z0-9_-]{15,}\.[A-Za-z0-9_-]{12,}"),
    re.compile(rb"/Users/[A-Za-z0-9._-]+/"),
)
for path in bundle.rglob("*"):
    if path.is_symlink() or not path.is_file():
        continue
    data = path.read_bytes()
    if any(value in data for value in local_paths) or any(pattern.search(data) for pattern in patterns):
        print("Bundle contains a local path or credential-like value; refusing release.", file=sys.stderr)
        sys.exit(1)
print("Bundle privacy patterns checked.")
PY
echo "Release verified with the fixed signing identity."
