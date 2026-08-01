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

# Bash completions: this dir is lazily loaded *by command name* — bash-completion sources
# only a file named after the command (deckhandd / deckhandd.bash). The file registers both
# commands, so we install it as deckhandd.bash and symlink deckhandctl.bash to it.
comp_dir="${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"
mkdir -p "$comp_dir"
cp completions/deckhandd.bash "$comp_dir/deckhandd.bash"
ln -sf deckhandd.bash "$comp_dir/deckhandctl.bash"
echo "installed bash completions into $comp_dir"
