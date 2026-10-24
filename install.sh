#!/bin/sh
# rsread installer: Linux, macOS and Android/Termux.
#   curl -fsSL https://raw.githubusercontent.com/cumulus13/rsread/main/install.sh | sh
# Options (env): RSREAD_REPO=owner/repo  RSREAD_INSTALL_DIR=/path  RSREAD_DRY_RUN=1
set -eu

REPO="${RSREAD_REPO:-cumulus13/rsread}"
BIN="rsread"

os="$(uname -s)"
arch="$(uname -m)"

if [ "$(uname -o 2>/dev/null || true)" = "Android" ] || [ -n "${TERMUX_VERSION:-}" ]; then
  os="Android"
fi

case "$os/$arch" in
  Android/aarch64|Android/arm64)       target="aarch64-linux-android" ;;
  Android/armv7l|Android/armv8l|Android/arm) target="armv7-linux-androideabi" ;;
  Android/x86_64)                      target="x86_64-linux-android" ;;
  Linux/x86_64|Linux/amd64)            target="x86_64-unknown-linux-musl" ;;
  Linux/aarch64|Linux/arm64)           target="aarch64-unknown-linux-musl" ;;
  Linux/armv7l|Linux/armv8l)           target="armv7-unknown-linux-gnueabihf" ;;
  Darwin/arm64)                        target="aarch64-apple-darwin" ;;
  Darwin/x86_64)                       target="x86_64-apple-darwin" ;;
  *)
    echo "error: unsupported platform $os/$arch (build from source: cargo install --git https://github.com/$REPO)" >&2
    exit 1
    ;;
esac

if [ "$os" = "Android" ] && [ -n "${PREFIX:-}" ]; then
  dir="${RSREAD_INSTALL_DIR:-$PREFIX/bin}"
elif [ "$(id -u)" = "0" ]; then
  dir="${RSREAD_INSTALL_DIR:-/usr/local/bin}"
else
  dir="${RSREAD_INSTALL_DIR:-$HOME/.local/bin}"
fi

url="https://github.com/$REPO/releases/latest/download/$BIN-$target.tar.gz"
echo "platform : $os/$arch -> $target"
echo "download : $url"
echo "install  : $dir/$BIN"
[ "${RSREAD_DRY_RUN:-0}" = "1" ] && exit 0

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$url" -o "$tmp/pkg.tar.gz"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$tmp/pkg.tar.gz" "$url"
else
  echo "error: need curl or wget" >&2
  exit 1
fi

tar xzf "$tmp/pkg.tar.gz" -C "$tmp"
mkdir -p "$dir"
cp "$tmp/$BIN-$target/$BIN" "$dir/$BIN"
chmod +x "$dir/$BIN"

echo "installed: $dir/$BIN"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "note: $dir is not in your PATH" ;;
esac
