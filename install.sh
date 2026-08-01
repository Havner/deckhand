#!/bin/sh
# Install the deckhand daemon + client into $CARGO_INSTALL_ROOT/bin (default: ~/.local/bin).
#
# `--force` so re-running picks up code changes (the workspace version stays 0.0.0, so cargo
# would otherwise say "already installed"). Extra args pass through to the *daemon* install —
# e.g. `./install.sh --no-default-features --features viiper` to select the VIIPER controller
# backend (deckhandctl is a thin client with no backend features).
set -e
root="${CARGO_INSTALL_ROOT:-$HOME/.local}"
cargo install --path crates/deckhandd  --root "$root" --force "$@"
cargo install --path crates/deckhandctl --root "$root" --force
echo "installed deckhandd + deckhandctl into $root/bin"
