#!/usr/bin/env bash
set -euo pipefail

PLUGIN_UUID="us.elbert.foundryvtt"
BIN_NAME="foundryvtt-streamdeck-plugin"

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TRIPLE="$(rustc -vV | awk '/^host:/ { print $2 }')"
DEST="$REPO_DIR/dist/$PLUGIN_UUID.sdPlugin"

cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"

rm -rf "$DEST"
mkdir -p "$DEST/$TRIPLE/bin"
cp -R "$REPO_DIR/assets/." "$DEST/"
install -m 755 "$REPO_DIR/target/release/$BIN_NAME" "$DEST/$TRIPLE/bin/$BIN_NAME"

echo "Built $DEST"
echo "Copy it into OpenDeck's plugins directory, then restart OpenDeck:"
echo "  ~/.config/opendeck/plugins/                                  (native)"
echo "  ~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/   (flatpak)"
