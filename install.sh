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

# systemd user units (Linux). Installed but NOT enabled. The service's ExecStart is rewritten to
# the just-installed binary so a non-default $CARGO_INSTALL_ROOT still works. `deckhandd.socket`
# listens on $XDG_RUNTIME_DIR/deckhand.sock (matching the daemon/client default), so an
# un-configured deckhandctl connects there.
#
# Enable EITHER lazy socket activation OR the always-on service — not both (the service pulls the
# socket in via Requires=):
#   systemctl --user enable --now deckhandd.socket   # lazy: first deckhandctl call starts the daemon
#   systemctl --user enable --now deckhandd.service   # always-on: daemon runs from login
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
mkdir -p "$unit_dir"
sed "s|^ExecStart=.*|ExecStart=$root/bin/deckhandd --systemd --verbose|" \
    systemd/deckhandd.service > "$unit_dir/deckhandd.service"
cp systemd/deckhandd.socket "$unit_dir/deckhandd.socket"
echo "installed systemd user units into $unit_dir (not enabled)"
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload 2>/dev/null || true
fi
