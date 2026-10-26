#!/bin/sh
set -e

REPO="sluisr/corex"
# Determine installation target directory
ACTIVE_CX="$(command -v cx 2>/dev/null || true)"
if [ -n "$PREFIX" ] && [ -d "$PREFIX/bin" ]; then
  INSTALL_DIR="$PREFIX/bin"
elif [ -d "/data/data/com.termux/files/usr/bin" ]; then
  INSTALL_DIR="/data/data/com.termux/files/usr/bin"
elif [ -n "$ACTIVE_CX" ] && [ -x "$ACTIVE_CX" ] && [ ! -L "$ACTIVE_CX" ]; then
  INSTALL_DIR="$(dirname "$ACTIVE_CX")"
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
        if [ -n "$PREFIX" ] || [ -d "/data/data/com.termux" ]; then
          TARGET="aarch64-linux-android"
        else
          TARGET="aarch64-unknown-linux-gnu"
        fi
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
LATEST_TAG=$(curl -fsSLI -o /dev/null -w "%{url_effective}" "https://github.com/$REPO/releases/latest" 2>/dev/null | sed 's|.*/tag/||' | tr -d '\r\n')
if [ -z "$LATEST_TAG" ] || [ "$LATEST_TAG" = "latest" ]; then
  LATEST_TAG=$(curl -sSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d '"' -f 4)
fi
VERSION="${LATEST_TAG:-v0.8.3}"

ARCHIVE_NAME="corex-${VERSION}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/${VERSION}/${ARCHIVE_NAME}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "[corex] Downloading ${ARCHIVE_NAME}..."
curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/$ARCHIVE_NAME"

# Verify SHA-256 integrity signature (fail closed)
echo "[corex] Downloading SHA-256 signature..."
if ! curl -fsSL "${DOWNLOAD_URL}.sha256" -o "$TMP_DIR/${ARCHIVE_NAME}.sha256" 2>/dev/null; then
  echo "Error: Could not download SHA-256 checksum from ${DOWNLOAD_URL}.sha256"
  exit 1
fi

EXPECTED_SHA=$(cut -d ' ' -f 1 < "$TMP_DIR/${ARCHIVE_NAME}.sha256" | tr -d '\r\n')
if [ -z "$EXPECTED_SHA" ] || [ "${#EXPECTED_SHA}" -ne 64 ]; then
  echo "Error: Invalid or empty SHA-256 checksum file."
  exit 1
fi

echo "[corex] Verifying SHA-256 signature..."
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA=$(sha256sum "$TMP_DIR/$ARCHIVE_NAME" | cut -d ' ' -f 1)
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL_SHA=$(shasum -a 256 "$TMP_DIR/$ARCHIVE_NAME" | cut -d ' ' -f 1)
elif command -v openssl >/dev/null 2>&1; then
  ACTUAL_SHA=$(openssl dgst -sha256 "$TMP_DIR/$ARCHIVE_NAME" | awk '{print $NF}')
else
  echo "Error: No SHA-256 verification tool found (sha256sum, shasum, or openssl required)."
  exit 1
fi

if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
  echo "Error: SHA-256 checksum verification failed!"
  echo "Expected: $EXPECTED_SHA"
  echo "Actual:   $ACTUAL_SHA"
  exit 1
fi
echo "  ✓ SHA-256 signature verified."

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
else
  $SUDO ln -sf "$INSTALL_DIR/cx" "$INSTALL_DIR/corex" 2>/dev/null || true
fi
$SUDO ln -sf "$INSTALL_DIR/cx" "$INSTALL_DIR/cxs" 2>/dev/null || true

# Synchronize any other cx/corex binaries in PATH to prevent older versions from shadowing
if command -v which >/dev/null 2>&1; then
  for existing_bin in $(which -a cx corex 2>/dev/null | sort -u); do
    if [ "$existing_bin" != "$INSTALL_DIR/cx" ] && [ "$existing_bin" != "$INSTALL_DIR/corex" ] && [ -f "$existing_bin" ] && [ ! -L "$existing_bin" ]; then
      if [ -w "$existing_bin" ]; then
        cp "$TMP_DIR/cx" "$existing_bin" 2>/dev/null || true
      elif [ -n "$SUDO" ]; then
        $SUDO cp "$TMP_DIR/cx" "$existing_bin" 2>/dev/null || true
      fi
    fi
  done
fi

# Set up shell autocompletions (Zsh, Bash, Fish)
CX_BIN="$INSTALL_DIR/cx"
if [ -x "$CX_BIN" ]; then
  # Zsh autocompletions
  for zsh_dir in "$HOME/.zfunc" "/usr/local/share/zsh/site-functions" "/usr/share/zsh/site-functions"; do
    if [ -d "$zsh_dir" ] || [ "$zsh_dir" = "$HOME/.zfunc" ]; then
      mkdir -p "$zsh_dir" 2>/dev/null || true
      if [ -w "$zsh_dir" ]; then
        "$CX_BIN" --completions zsh > "$zsh_dir/_cx" 2>/dev/null && echo "  ✓ Zsh autocompletions installed to $zsh_dir/_cx" && break || true
      elif [ -n "$SUDO" ] && [ -d "$zsh_dir" ]; then
        "$CX_BIN" --completions zsh | $SUDO tee "$zsh_dir/_cx" >/dev/null 2>&1 && echo "  ✓ Zsh autocompletions installed to $zsh_dir/_cx" && break || true
      fi
    fi
  done

  # Bash autocompletions
  for bash_dir in "/etc/bash_completion.d" "/usr/local/etc/bash_completion.d" "$HOME/.local/share/bash-completion/completions"; do
    if [ -d "$bash_dir" ] || [ "$bash_dir" = "$HOME/.local/share/bash-completion/completions" ]; then
      mkdir -p "$bash_dir" 2>/dev/null || true
      if [ -w "$bash_dir" ]; then
        "$CX_BIN" --completions bash > "$bash_dir/cx" 2>/dev/null && echo "  ✓ Bash autocompletions installed to $bash_dir/cx" && break || true
      elif [ -n "$SUDO" ] && [ -d "$bash_dir" ]; then
        "$CX_BIN" --completions bash | $SUDO tee "$bash_dir/cx" >/dev/null 2>&1 && echo "  ✓ Bash autocompletions installed to $bash_dir/cx" && break || true
      fi
    fi
  done

  # Fish autocompletions
  fish_dir="$HOME/.config/fish/completions"
  if [ -d "$fish_dir" ] || command -v fish >/dev/null 2>&1; then
    mkdir -p "$fish_dir" 2>/dev/null || true
    if [ -w "$fish_dir" ]; then
      "$CX_BIN" --completions fish > "$fish_dir/cx.fish" 2>/dev/null && echo "  ✓ Fish autocompletions installed to $fish_dir/cx.fish" || true
    fi
  fi

  # Invalidate cached zcompdump so new shell sessions reload completions immediately
  rm -f "$HOME/.zcompdump"* 2>/dev/null || true
fi

# Set up output styling
if [ -t 1 ]; then
  BOLD="\033[1m"
  GREEN="\033[1;32m"
  CYAN="\033[1;36m"
  YELLOW="\033[1;33m"
  BLUE="\033[1;34m"
  DIM="\033[2m"
  RESET="\033[0m"
else
  BOLD=""
  GREEN=""
  CYAN=""
  YELLOW=""
  BLUE=""
  DIM=""
  RESET=""
fi

printf "\n%b✓ Corex %s installed successfully to %s/cx%b\n\n" "$GREEN" "$VERSION" "$INSTALL_DIR" "$RESET"
printf "%b🚀 Quick Start:%b\n" "$BOLD" "$RESET"
printf "   %bcx%b                  Launch Corex in the current project\n" "$CYAN" "$RESET"
printf "   %bcx --help%b           View options, subagents, and CLI flags\n\n" "$CYAN" "$RESET"
printf "%b🌟 Links & Community:%b\n" "$BOLD" "$RESET"
printf "   %b⭐ GitHub:%b   https://github.com/sluisr/corex %b(Leave a star if you find it useful!)%b\n" "$YELLOW" "$RESET" "$DIM" "$RESET"
printf "   %b💬 Discord:%b  https://discord.gg/MGMt6M6Yx %b(Join the community)%b\n" "$BLUE" "$RESET" "$DIM" "$RESET"
printf "   %b🌍 Website:%b  https://corex.sluisr.com\n" "$CYAN" "$RESET"
printf "   %b👤 Author:%b   https://sluisr.com\n\n" "$DIM" "$RESET"
