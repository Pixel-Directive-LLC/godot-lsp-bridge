#!/usr/bin/env bash
# Install godot-lsp-bridge — detects OS/arch, fetches the latest release binary,
# and installs it to ~/.cargo/bin (if present) or ~/.local/bin.
set -euo pipefail

REPO="Pixel-Directive-LLC/godot-lsp-bridge"
BIN="godot-lsp-bridge"

# ── Detect target triple ──────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)          TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64 | arm64) TARGET="aarch64-unknown-linux-gnu" ;;
      *) echo "error: unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac ;;
  Darwin)
    case "$ARCH" in
      x86_64) TARGET="x86_64-apple-darwin" ;;
      arm64)  TARGET="aarch64-apple-darwin" ;;
      *) echo "error: unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac ;;
  *)
    echo "error: unsupported OS: $OS" >&2
    exit 1 ;;
esac

# ── Resolve latest version ────────────────────────────────────────────────────
VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep '"tag_name"' \
  | sed 's/.*"tag_name": *"v\([^"]*\)".*/\1/')"

if [ -z "$VERSION" ]; then
  echo "error: could not determine latest release version" >&2
  exit 1
fi

# ── Choose install directory ──────────────────────────────────────────────────
if [ -d "$HOME/.cargo/bin" ]; then
  INSTALL_DIR="$HOME/.cargo/bin"
elif [ -d "$HOME/.local/bin" ]; then
  INSTALL_DIR="$HOME/.local/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
fi

# ── Download and install ──────────────────────────────────────────────────────
ARCHIVE="${BIN}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ARCHIVE}"

echo "Installing ${BIN} v${VERSION} (${TARGET})"
echo "  source: ${URL}"
echo "  dest:   ${INSTALL_DIR}/${BIN}"
echo ""

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -fsSL "$URL" -o "$TMP/$ARCHIVE"
tar -xzf "$TMP/$ARCHIVE" -C "$TMP"
chmod +x "$TMP/$BIN"
mv "$TMP/$BIN" "$INSTALL_DIR/$BIN"

echo "Done. ${BIN} v${VERSION} installed."

# ── PATH hint ─────────────────────────────────────────────────────────────────
if ! printf '%s\n' "${PATH//:/$'\n'}" | grep -qx "$INSTALL_DIR"; then
  echo ""
  echo "Note: ${INSTALL_DIR} is not on your PATH."
  echo "Add the following line to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
  echo ""
  echo "  export PATH=\"\$PATH:${INSTALL_DIR}\""
fi
