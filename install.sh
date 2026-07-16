#!/usr/bin/env sh
set -eu

NAME="ai-setup"
VERSION="0.6.1"
REPO="Yassimba/ai-setup"
TAG="ai-setup-v${VERSION}"
INSTALL_DIR="${AI_SETUP_INSTALL_DIR:-${YASSIMBA_INSTALL_DIR:-$HOME/.local/bin}}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-gnu" ;;
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  MINGW* | MSYS* | CYGWIN*)
    echo "$NAME: this looks like Git Bash/MSYS on Windows; use the PowerShell installer instead:" >&2
    echo '  powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/Yassimba/ai-setup/main/install.ps1 | iex"' >&2
    exit 1 ;;
  *) echo "$NAME: unsupported platform $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

archive="${NAME}-${target}.tar.gz"
checksum="${NAME}-${target}.sha256"
base="https://github.com/${REPO}/releases/download/${TAG}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

download() {
  curl -fsSL --retry 5 --retry-delay 3 --retry-all-errors "$1" -o "$2"
}

download "$base/$archive" "$tmp/$archive"
download "$base/$checksum" "$tmp/$checksum"
expected="$(awk '{print $1}' "$tmp/$checksum")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
else
  echo "$NAME: sha256sum or shasum is required" >&2
  exit 1
fi
[ "$expected" = "$actual" ] || { echo "$NAME: checksum mismatch" >&2; exit 1; }

mkdir -p "$INSTALL_DIR"
tar -xzf "$tmp/$archive" -C "$tmp"
install -m 0755 "$tmp/$NAME" "$INSTALL_DIR/$NAME"
rm -f "$INSTALL_DIR/yassimba"
echo "$NAME $VERSION installed to $INSTALL_DIR/$NAME"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add $INSTALL_DIR to PATH, then run: ai-setup setup" ;;
esac
