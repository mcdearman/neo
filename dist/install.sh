#!/bin/sh
# Installs the Neo apps for the current user on Linux, so they appear in the
# app menu of GNOME, KDE Plasma or any other freedesktop.org desktop.
#
#   dist/install.sh              # build and install into ~/.local
#   PREFIX=/usr/local sudo -E dist/install.sh
#   dist/install.sh --uninstall
set -eu

APPS="files terminal settings monitor calculator code"
PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin"
DESKTOP="$PREFIX/share/applications"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ "${1:-}" = "--uninstall" ]; then
    for app in $APPS; do
        rm -f "$BIN/neo-$app"
    done
    rm -f "$DESKTOP"/org.neo.*.desktop
    echo "Removed the Neo apps from $PREFIX."
    exit 0
fi

packages=""
for app in $APPS; do
    packages="$packages -p neo-$app"
done
# shellcheck disable=SC2086
cargo build --release --manifest-path "$ROOT/Cargo.toml" $packages

mkdir -p "$BIN" "$DESKTOP"
for app in $APPS; do
    cp "$ROOT/target/release/neo-$app" "$BIN/neo-$app"
    chmod 755 "$BIN/neo-$app"
done
cp "$ROOT"/dist/applications/*.desktop "$DESKTOP/"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DESKTOP" >/dev/null 2>&1 || true
fi

echo "Installed the Neo apps into $PREFIX."
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) echo "Add $BIN to your PATH to launch them from a terminal." ;;
esac
