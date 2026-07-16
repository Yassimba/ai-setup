#!/usr/bin/env bash
set -euo pipefail

NAME="herdr-project-switcher"
REPO="Yassimba/ai-setup"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="$ROOT/bin"
VERSION="$(grep -m1 '^version' "$ROOT/herdr-plugin.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
TAG="${NAME}-v${VERSION}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-gnu" ;;
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  *) echo "$NAME: unsupported platform; build with 'cargo build --release'" >&2; exit 1 ;;
esac

archive="${NAME}-${target}.tar.gz"
checksum="${NAME}-${target}.sha256"
base="https://github.com/${REPO}/releases/download/${TAG}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

download() { curl -fsSL --retry 5 --retry-delay 3 --retry-all-errors "$1" -o "$2"; }
download "$base/$archive" "$tmp/$archive"
download "$base/$checksum" "$tmp/$checksum"
expected="$(awk '{print $1}' "$tmp/$checksum")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
else
  actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
fi
[ "$expected" = "$actual" ] || { echo "$NAME: checksum mismatch" >&2; exit 1; }
mkdir -p "$BIN_DIR"
tar -xzf "$tmp/$archive" -C "$tmp"
install -m 0755 "$tmp/$NAME" "$BIN_DIR/$NAME"
echo "$NAME: installed $BIN_DIR/$NAME"
