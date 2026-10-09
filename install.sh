#!/bin/sh
set -e

REPO="sluisr/corex"
if [ -n "$PREFIX" ] && [ -d "$PREFIX/bin" ]; then
  INSTALL_DIR="$PREFIX/bin"
else
  INSTALL_DIR="/usr/local/bin"
fi

# Detect OS and architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
  linux)
    case "$ARCH" in
      x86_64)
        TARGET="x86_64-unknown-linux-gnu"
        ;;
      aarch64|arm64)
        TARGET="aarch64-unknown-linux-musl"
        ;;
      *)
        echo "Error: Unsupported architecture $ARCH on Linux."
        exit 1
        ;;
    esac
    ;;
  darwin)
    case "$ARCH" in
      arm64|aarch64)
        TARGET="aarch64-apple-darwin"
        ;;
      x86_64)
        TARGET="x86_64-apple-darwin"
        ;;
      *)
        echo "Error: Unsupported architecture $ARCH on macOS."
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Error: Unsupported operating system $OS. For Windows, download from https://github.com/$REPO/releases"
    exit 1
    ;;
esac

echo "[corex] Detecting latest version..."
LATEST_TAG=$(curl -sSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | grep '"tag_name":' | head -1 | cut -d '"' -f 4)
VERSION="${LATEST_TAG:-v0.3.0}"

ARCHIVE_NAME="corex-${VERSION}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/${VERSION}/${ARCHIVE_NAME}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "[corex] Downloading ${ARCHIVE_NAME}..."
curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/$ARCHIVE_NAME"

echo "[corex] Extracting binary..."
tar -xzf "$TMP_DIR/$ARCHIVE_NAME" -C "$TMP_DIR"

# Install destination check
if [ ! -w "$INSTALL_DIR" ]; then
  if command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
  else
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
    SUDO=""
  fi
else
  SUDO=""
fi

echo "[corex] Installing binaries to $INSTALL_DIR..."
$SUDO cp "$TMP_DIR/cx" "$INSTALL_DIR/cx"
$SUDO chmod +x "$INSTALL_DIR/cx"

if [ -f "$TMP_DIR/corex" ]; then
  $SUDO cp "$TMP_DIR/corex" "$INSTALL_DIR/corex"
  $SUDO chmod +x "$INSTALL_DIR/corex"
fi

echo "✓ Corex ${VERSION} installed successfully!"
echo "Run 'cx' in any project directory to launch."
