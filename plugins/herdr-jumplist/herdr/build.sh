#!/usr/bin/env bash
set -euo pipefail

NAME="herdr-jumplist"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

command -v cargo >/dev/null 2>&1 || {
  echo "$NAME: cargo not found; install a Rust toolchain (https://rustup.rs)" >&2
  exit 1
}

if ! cargo build --release --manifest-path "$ROOT/Cargo.toml"; then
  echo "$NAME: build failed. If cargo reported an unsupported rustc version, run 'rustup update' and reinstall." >&2
  exit 1
fi
mkdir -p "$ROOT/bin"
install -m 0755 "$ROOT/target/release/$NAME" "$ROOT/bin/$NAME"
echo "$NAME: installed $ROOT/bin/$NAME"
