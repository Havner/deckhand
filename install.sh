#!/bin/sh
# Install the deckhand daemon + client + UI, plus (on Linux) the desktop integration.
#
# Usage: ./install.sh [-f|--forwarder] [-r|--root-prefix DIR] [cargo args...]
#
#   -f, --forwarder      also install the forwarder UI (Linux-only; targets the Steam Deck).
#   -r, --root-prefix DIR  build a relocatable RELEASE tree under DIR instead of installing into
#                        the live $HOME. DIR is created if missing. It is NOT where the files will
#                        live - it mirrors a $HOME so the tree can be zipped and unpacked straight
#                        into a target user's $HOME. The standard layout is forced (.local/bin,
#                        .local/share, .config, Desktop) regardless of $XDG_*/$CARGO_INSTALL_ROOT,
#                        so the archive stays portable; symlink targets point at the real $HOME, not
#                        into DIR. NOTE: this relocates the build machine's output - it does NOT
#                        cross-compile, so the archive is for the same OS/arch you build on.
#   cargo args...        passed through to the *daemon* build only - e.g.
#                        `./install.sh --no-default-features --features viiper` selects the VIIPER
#                        controller backend (deckhandctl/deckhand are thin clients, no backend).
#
# `--force` is always passed so re-running picks up code changes: the workspace version rarely
# changes between edits, so without --force cargo would say "already installed" and skip the rebuild.
#
# After a normal (no -r) Linux install this runs `setup.sh` for you (cache refresh, daemon-reload,
# PATH check). With -r it instead copies setup.sh into DIR as `setup-deckhand.sh`, to be run once
# after the archive is unpacked into $HOME.
set -e

# Arg parsing: pull out -f and -r DIR; everything else passes through to the daemon cargo build.
prefix=""
forwarder=0
rest=""
while [ $# -gt 0 ]; do
    case "$1" in
        -f|--forwarder) forwarder=1 ;;
        -r|--root-prefix) shift; prefix="$1" ;;
        *) rest="$rest $1" ;;
    esac
    shift
done
# shellcheck disable=SC2086
set -- $rest

# Resolve install roots. With -r we force the standard $HOME-relative layout under DIR so the tree
# is a portable archive; without it we honor $CARGO_INSTALL_ROOT/$XDG_* exactly as a live install.
if [ -n "$prefix" ]; then
    mkdir -p "$prefix"
    root="$prefix/.local"
    data="$prefix/.local/share"
    config="$prefix/.config"
    desktop_dir="$prefix/Desktop"
else
    root="${CARGO_INSTALL_ROOT:-$HOME/.local}"
    data="${XDG_DATA_HOME:-$HOME/.local/share}"
    config="${XDG_CONFIG_HOME:-$HOME/.config}"
    desktop_dir="$HOME/Desktop"
fi
apps_dir="$data/applications"
icon_dir="$data/icons/hicolor/512x512/apps"
comp_dir="$data/bash-completion/completions"
unit_dir="$config/systemd/user"

# The three always-installed binaries (every platform). Only the daemon takes the pass-through
# backend feature args; deckhandctl and the deckhand UI are thin daemon clients.
cargo install --locked --path crates/deckhandd  --root "$root" --force "$@"
cargo install --locked --path crates/deckhandctl --root "$root" --force
cargo install --locked --path crates/deckhand    --root "$root" --force
echo "installed deckhandd + deckhandctl + deckhand (UI) into $root/bin"

# Everything below is Linux-only desktop integration for the UI (.desktop entry + themed icon,
# bash completions, systemd user units), so on Windows/macOS the script stops here (after stripping
# cargo's install metadata from a -r archive - see the note at the finalize step).
if [ "$(uname -s)" != Linux ]; then
    if [ -n "$prefix" ]; then rm -f "$root/.crates.toml" "$root/.crates2.json"; fi
    exit 0
fi

# Desktop entry + icon so the UI shows up in the app launcher. The icon goes into the hicolor
# theme at its native 512x512 size; the .desktop's `Icon=deckhand` resolves to it by name.
mkdir -p "$apps_dir" "$icon_dir"
cp crates/deckhand/assets/deckhand.desktop "$apps_dir/deckhand.desktop"
cp crates/deckhand/assets/deckhand.png "$icon_dir/deckhand.png"
echo "installed desktop entry into $apps_dir and icon into $icon_dir"

# Optional: the forwarder UI (Deck-as-network-client), only with -f/--forwarder. Its binary is
# installed here (inside the Linux-only section) since the app targets the Steam Deck. Same
# desktop-entry + icon layout as the main UI, plus a double-clickable launcher symlink on ~/Desktop
# (handy on the Deck). With -r the launcher is a RELATIVE symlink (../.local/share/applications/...)
# so it stays valid after the archive is unpacked into a differently-named $HOME (e.g. /home/deck);
# a live install keeps the absolute link as before.
if [ "$forwarder" -eq 1 ]; then
    cargo install --locked --path crates/forwarder-ui --root "$root" --force
    echo "installed deckhand-forwarder (UI) into $root/bin"
    cp crates/forwarder-ui/assets/deckhand-forwarder.desktop "$apps_dir/deckhand-forwarder.desktop"
    cp crates/forwarder-ui/assets/deckhand-forwarder.png "$icon_dir/deckhand-forwarder.png"
    echo "installed forwarder desktop entry into $apps_dir and icon into $icon_dir"
    mkdir -p "$desktop_dir"
    if [ -n "$prefix" ]; then
        ln -sf ../.local/share/applications/deckhand-forwarder.desktop "$desktop_dir/deckhand-forwarder.desktop"
    else
        ln -sf "$apps_dir/deckhand-forwarder.desktop" "$desktop_dir/deckhand-forwarder.desktop"
    fi
    echo "linked forwarder launcher into $desktop_dir"
fi

# Bash completions: this dir is lazily loaded *by command name* - bash-completion sources
# only a file named after the command (deckhandd / deckhandd.bash). The file registers both
# commands, so we install it as deckhandd.bash and symlink deckhandctl.bash to it (a relative
# symlink, so it survives relocation).
mkdir -p "$comp_dir"
cp completions/deckhandd.bash "$comp_dir/deckhandd.bash"
ln -sf deckhandd.bash "$comp_dir/deckhandctl.bash"
echo "installed bash completions into $comp_dir"

# systemd user units (Linux). Installed but NOT enabled. `deckhandd.socket` listens on
# $XDG_RUNTIME_DIR/deckhand.sock (matching the daemon/client default), so an un-configured
# deckhandctl connects there.
#
# Enable EITHER lazy socket activation OR the always-on service - not both (the service pulls the
# socket in via Requires=):
#   systemctl --user enable --now deckhandd.socket   # lazy: first deckhandctl call starts the daemon
#   systemctl --user enable --now deckhandd.service   # always-on: daemon runs from login
#
# The service's ExecStart uses %h/.local/bin/deckhandd (systemd expands %h to the target user's
# home at runtime). For a -r archive that stock unit is already portable, so we copy it verbatim;
# for a live install we rewrite ExecStart so a non-default $CARGO_INSTALL_ROOT still works.
mkdir -p "$unit_dir"
if [ -n "$prefix" ]; then
    cp systemd/deckhandd.service "$unit_dir/deckhandd.service"
else
    sed "s|^ExecStart=.*|ExecStart=$root/bin/deckhandd --systemd --verbose|" \
        systemd/deckhandd.service > "$unit_dir/deckhandd.service"
fi
cp systemd/deckhandd.socket "$unit_dir/deckhandd.socket"
echo "installed systemd user units into $unit_dir (not enabled)"

# Finalize. A live install fixes up this machine now; a -r archive ships setup.sh as
# setup-deckhand.sh to be run once after unpacking into $HOME.
if [ -n "$prefix" ]; then
    # Drop cargo's install bookkeeping (.crates.toml/.crates2.json): in an archive it only leaks the
    # build machine's absolute paths and would clobber the target's own ~/.local/.crates.toml on
    # unpack. A live install keeps it so `cargo uninstall` still works.
    rm -f "$root/.crates.toml" "$root/.crates2.json"
    cp setup.sh "$prefix/setup-deckhand.sh"
    echo "copied setup script to $prefix/setup-deckhand.sh (run it after unpacking into \$HOME)"
else
    sh setup.sh
fi
