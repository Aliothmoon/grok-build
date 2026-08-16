#!/bin/sh
# igrok installer (macOS / Linux).
# Downloads the latest igrok binary from GitHub Releases into ~/.igrok/bin
# and adds it to PATH. Requires curl + unzip only.
#
# Install:  curl -fsSL https://raw.githubusercontent.com/Aliothmoon/grok-build/dev/install.sh | sh
set -eu

REPO='Aliothmoon/grok-build'

OS=$(uname -s)
case "$OS" in
    Darwin) OS='macos' ;;
    Linux) OS='linux' ;;
    *) echo "Unsupported OS: $OS"; exit 1 ;;
esac
ARCH=$(uname -m)
case "$ARCH" in
    x86_64|amd64) ARCH='x86_64' ;;
    arm64|aarch64) ARCH='aarch64' ;;
    *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac
PATTERN="igrok-*-${OS}-${ARCH}*"

BIN_DIR="$HOME/.igrok/bin"
mkdir -p "$BIN_DIR"

echo "Resolving latest release from $REPO ..."
ASSET_URL=$(curl -fsSL -H 'User-Agent: igrok-installer' \
    "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -o '"browser_download_url": *"[^"]*"' \
    | grep -o 'https[^"]*' \
    | grep -E "igrok-[0-9][^/]*-${OS}-${ARCH}" \
    | head -n 1 || true)
if [ -z "$ASSET_URL" ]; then
    echo "No release asset matching '$PATTERN' — this platform may not be published yet."
    exit 1
fi

ASSET_NAME="${ASSET_URL##*/}"
TMP=$(mktemp -d)
echo "Downloading $ASSET_NAME ..."
curl -fsSL -o "$TMP/$ASSET_NAME" "$ASSET_URL"

DEST="$BIN_DIR/igrok"
case "$ASSET_NAME" in
    *.zip) unzip -oq "$TMP/$ASSET_NAME" -d "$TMP/extracted" && mv "$TMP/extracted/"* "$DEST" ;;
    *) mv "$TMP/$ASSET_NAME" "$DEST" ;;
esac
chmod +x "$DEST"
rm -rf "$TMP"

# Add ~/.igrok/bin to PATH in the user's shell rc if missing.
RC_FILE=""
[ -n "${ZDOTDIR:-}" ] && [ -f "$ZDOTDIR/.zshrc" ] && RC_FILE="$ZDOTDIR/.zshrc"
[ -z "$RC_FILE" ] && for f in "$HOME/.zshrc" "$HOME/.bashrc"; do
    [ -f "$f" ] && RC_FILE="$f" && break
done
if [ -n "$RC_FILE" ] && ! grep -q '.igrok/bin' "$RC_FILE"; then
    printf '\n# added by igrok installer\nexport PATH="$HOME/.igrok/bin:$PATH"\n' >> "$RC_FILE"
    echo "Added ~/.igrok/bin to PATH in $RC_FILE (open a new shell to pick it up)."
fi

echo ""
echo "Installed: $DEST"
"$DEST" --version || true
echo ""
echo "Next steps:"
echo "  - Provider API keys via env vars, e.g. ANTHROPIC_API_KEY / OPENAI_API_KEY"
echo "  - Or 'igrok login' for grok models"
echo "  - Self-update: igrok update  (pulls from $REPO releases)"
