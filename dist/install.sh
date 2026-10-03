#!/bin/sh
# Installs the Neo apps into this computer's app launcher. The same as
# `cargo xtask install`, which also works on Windows.
#
#   dist/install.sh               # build and install
#   dist/install.sh --uninstall   # remove them
set -eu
cd "$(dirname "$0")/.."
if [ "${1:-}" = "--uninstall" ]; then
    exec cargo xtask uninstall
fi
exec cargo xtask install "$@"
