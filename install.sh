#!/bin/sh
set -e

REPO="sluisr/corex"
BETA="${COREX_BETA:-0}"
REQUESTED_VERSION="${COREX_VERSION:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    --beta|--pre)
      BETA=1
      ;;
    --version)
      shift
      REQUESTED_VERSION="${1:-}"
      ;;
    --version=*)
      REQUESTED_VERSION="${1#*=}"
      ;;
  esac
  shift
done

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
        if [ -d "/data/data/com.termux" ] || [ "${PREFIX#*com.termux}" != "$PREFIX" ]; then
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

semver_gt() {
  awk -v a="$1" -v b="$2" '
    function parse(v, arr,    core, pre, n, i, p) {
      sub(/^v/, "", v)
      sub(/\+.*$/, "", v)
      i = index(v, "-")
      if (i > 0) {
        core = substr(v, 1, i - 1)
        pre = substr(v, i + 1)
      } else {
        core = v
        pre = ""
      }
      split(core, arr, ".")
      arr["maj"] = arr[1] + 0
      arr["min"] = arr[2] + 0
      arr["pat"] = arr[3] + 0
      arr["stable"] = (pre == "" ? 1 : 0)
      arr["pre"] = pre
    }
    BEGIN {
      parse(a, A)
      parse(b, B)
      if (A["maj"] != B["maj"]) exit (A["maj"] > B["maj"] ? 0 : 1)
      if (A["min"] != B["min"]) exit (A["min"] > B["min"] ? 0 : 1)
      if (A["pat"] != B["pat"]) exit (A["pat"] > B["pat"] ? 0 : 1)
      if (A["stable"] != B["stable"]) exit (A["stable"] > B["stable"] ? 0 : 1)
      if (A["stable"] == 1) exit 1
      na = split(A["pre"], pa, ".")
      nb = split(B["pre"], pb, ".")
      n = (na > nb ? na : nb)
      for (k = 1; k <= n; k++) {
        if (k > na) exit 1
        if (k > nb) exit 0
        ia = pa[k]; ib = pb[k]
        da = (ia ~ /^[0-9]+$/); db = (ib ~ /^[0-9]+$/)
        if (da && db) {
          if ((ia + 0) != (ib + 0)) exit ((ia + 0) > (ib + 0) ? 0 : 1)
        } else if (da != db) {
          exit (db ? 0 : 1)
        } else if (ia != ib) {
          exit (ia > ib ? 0 : 1)
        }
      }
      exit 1
    }
  '
}

if [ -n "$REQUESTED_VERSION" ]; then
  case "$REQUESTED_VERSION" in
    v*) VERSION="$REQUESTED_VERSION" ;;
    *) VERSION="v$REQUESTED_VERSION" ;;
  esac
  echo "[corex] Installing requested version ${VERSION}..."
else
  echo "[corex] Detecting latest version..."
  STABLE_TAG=$(curl -fsSLI -o /dev/null -w "%{url_effective}" "https://github.com/$REPO/releases/latest" 2>/dev/null | sed 's|.*/tag/||' | tr -d '\r\n')
  case "$STABLE_TAG" in
    v[0-9]*) ;;
    *) STABLE_TAG=$(curl -sSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d '"' -f 4) ;;
  esac
  VERSION="$STABLE_TAG"

  if [ "$BETA" = "1" ] || [ "$BETA" = "true" ]; then
    RELEASES_JSON=$(curl -sSL "https://api.github.com/repos/$REPO/releases?per_page=15" 2>/dev/null || true)
    CANDIDATES=$(printf '%s' "$RELEASES_JSON" | awk '
      /"tag_name":/ {
        if (match($0, /"tag_name": *"[^"]*"/)) {
          tag = substr($0, RSTART, RLENGTH)
          gsub(/.*"tag_name": *"|"$/, "", tag)
          draft = 0
        }
      }
      /"draft": *true/ { draft = 1 }
      /"prerelease":/ {
        if (!draft && tag ~ /^v[0-9]/) print tag
        tag = ""
      }
    ')
    for cand in $CANDIDATES; do
      if [ -z "$VERSION" ] || semver_gt "$cand" "$VERSION"; then
        VERSION="$cand"
      fi
    done
  fi
fi

case "$VERSION" in
  v[0-9]*) ;;
  *)
    echo "Error: Could not detect the latest Corex release. Check your connection and try again."
    exit 1
    ;;
esac

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

# Copy next to the target and rename over it: a running binary cannot be overwritten in place.
install_bin() {
  $1 cp "$2" "$3.new.$$" && $1 chmod +x "$3.new.$$" && $1 mv -f "$3.new.$$" "$3"
}

echo "[corex] Installing binaries to $INSTALL_DIR..."
install_bin "$SUDO" "$TMP_DIR/cx" "$INSTALL_DIR/cx"

if [ -f "$TMP_DIR/corex" ]; then
  install_bin "$SUDO" "$TMP_DIR/corex" "$INSTALL_DIR/corex"
else
  $SUDO ln -sf "$INSTALL_DIR/cx" "$INSTALL_DIR/corex" 2>/dev/null || true
fi
$SUDO ln -sf "$INSTALL_DIR/cx" "$INSTALL_DIR/cxs" 2>/dev/null || true

# Synchronize any other cx/corex binaries in PATH to prevent older versions from shadowing
if command -v which >/dev/null 2>&1; then
  for existing_bin in $(which -a cx corex 2>/dev/null | sort -u); do
    if [ "$existing_bin" != "$INSTALL_DIR/cx" ] && [ "$existing_bin" != "$INSTALL_DIR/corex" ] && [ -f "$existing_bin" ] && [ ! -L "$existing_bin" ] \
      && "$existing_bin" --version 2>/dev/null | grep -qE '^(cx|corex) [0-9]'; then
      if [ -w "$(dirname "$existing_bin")" ]; then
        install_bin "" "$TMP_DIR/cx" "$existing_bin" 2>/dev/null || true
      elif [ -n "$SUDO" ]; then
        install_bin "$SUDO" "$TMP_DIR/cx" "$existing_bin" 2>/dev/null || true
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
