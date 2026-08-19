#!/bin/sh
# Install the deckhand daemon + client into $CARGO_INSTALL_ROOT/bin (default: ~/.local/bin).
#
# `--force` so re-running picks up code changes (the workspace version stays 0.0.0, so cargo
# would otherwise say "already installed"). Extra args pass through to the *daemon* install —
# e.g. `./install.sh --no-default-features --features viiper` to select the VIIPER controller
# backend (deckhandctl is a thin client with no backend features).
set -e

# Optional flag: -f/--forwarder additionally installs the forwarder UI (Linux-only — it targets the
# Steam Deck; see the forwarder block below). It's not a cargo flag, so strip it out before the
# pass-through args that select the daemon backend.
forwarder=0
rest=""
for a in "$@"; do
    case "$a" in
        -f|--forwarder) forwarder=1 ;;
        *) rest="$rest $a" ;;
    esac
done
# shellcheck disable=SC2086
set -- $rest

root="${CARGO_INSTALL_ROOT:-$HOME/.local}"
cargo install --path crates/deckhandd  --root "$root" --force "$@"
cargo install --path crates/deckhandctl --root "$root" --force
echo "installed deckhandd + deckhandctl into $root/bin"

# The deckhand UI app, into the same $root/bin as the tools (it's a workspace member but not a
# default-member, so it's addressed by path). No backend features — it's a thin daemon client. On
# Windows the built .exe carries its own embedded icon and runs as a GUI app (no console window).
cargo install --path crates/deckhand --root "$root" --force
echo "installed deckhand (UI) into $root/bin"

# All three binaries above install on every platform. Everything below is Linux-only desktop
# integration for the UI (its .desktop entry + themed icon, bash completions, systemd user units), so
# on Windows/macOS the script stops here.
[ "$(uname -s)" = Linux ] || exit 0

# Desktop entry + icon so the UI shows up in the app launcher. The icon goes into the hicolor
# theme at its native 512×512 size; the .desktop's `Icon=deckhand` resolves to it by name.
apps_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icon_dir="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"
mkdir -p "$apps_dir" "$icon_dir"
cp crates/deckhand/assets/deckhand.desktop "$apps_dir/deckhand.desktop"
cp crates/deckhand/assets/deckhand.png "$icon_dir/deckhand.png"
echo "installed desktop entry into $apps_dir and icon into $icon_dir"

# Optional: the forwarder UI (Deck-as-network-client), only with -f/--forwarder. Its binary is
# installed here (inside the Linux-only section) since the app targets the Steam Deck. Same
# desktop-entry + icon layout as the main UI, plus a double-clickable launcher symlink on ~/Desktop
# (handy on the Deck; a symlink so it tracks the installed entry).
if [ "$forwarder" -eq 1 ]; then
    cargo install --path crates/forwarder-ui --root "$root" --force
    echo "installed deckhand-forwarder (UI) into $root/bin"
    cp crates/forwarder-ui/assets/deckhand-forwarder.desktop "$apps_dir/deckhand-forwarder.desktop"
    cp crates/forwarder-ui/assets/deckhand-forwarder.png "$icon_dir/deckhand-forwarder.png"
    echo "installed forwarder desktop entry into $apps_dir and icon into $icon_dir"
    desktop_dir="$HOME/Desktop"
    mkdir -p "$desktop_dir"
    ln -sf "$apps_dir/deckhand-forwarder.desktop" "$desktop_dir/deckhand-forwarder.desktop"
    echo "linked forwarder launcher into $desktop_dir"
fi

# Refresh the desktop + icon caches so a running GNOME/KDE picks up the new entry and icon
# immediately (all best-effort — absent tools / no index.theme are harmless).
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$apps_dir" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor" 2>/dev/null || true
fi

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
