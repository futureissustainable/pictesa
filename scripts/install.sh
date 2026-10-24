#!/bin/sh
# Installs the latest Pictesa release into ~/.local/bin (Linux and macOS).
#   curl -fsSL https://raw.githubusercontent.com/futureissustainable/pictesa/main/scripts/install.sh | sh
set -eu
repo="futureissustainable/pictesa"
os=$(uname -s)
arch=$(uname -m)
case "$os-$arch" in
  Linux-x86_64) asset="pictesa-linux-x86_64.tar.gz" ;;
  Linux-aarch64|Linux-arm64) asset="pictesa-linux-arm64.tar.gz" ;;
  Darwin-arm64) asset="pictesa-macos-arm64.tar.gz" ;;
  Darwin-x86_64) asset="pictesa-macos-x86_64.tar.gz" ;;
  *) echo "No prebuilt Pictesa for $os $arch yet. Build from source: https://github.com/$repo" >&2; exit 1 ;;
esac
dest="${PICTESA_INSTALL_DIR:-$HOME/.local/bin}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $asset ..."
curl -fsSL "https://github.com/$repo/releases/latest/download/$asset" -o "$tmp/pictesa.tar.gz"
tar -xzf "$tmp/pictesa.tar.gz" -C "$tmp"
mkdir -p "$dest"
install -m 0755 "$tmp/pictesa" "$dest/pictesa"
echo "Installed $dest/pictesa"
case ":$PATH:" in *":$dest:"*) ;; *) echo "Add $dest to your PATH, then run: pictesa" ;; esac
command -v ffmpeg >/dev/null 2>&1 || echo "Tip: install ffmpeg for video thumbnails and durations."
