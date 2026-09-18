#!/bin/sh
# Fix up the local machine after deckhand's files are in place - refresh the desktop + icon
# caches and reload the systemd user units. Run this once after `install.sh` (install.sh runs it
# for you) or, for a release archive built with `install.sh -r`, after unpacking the archive into
# your $HOME. Best-effort and idempotent: safe to run standalone and repeatedly. Operates on the
# live $HOME (XDG-aware) and takes no arguments. Non-Linux has nothing to do.
set -e
[ "$(uname -s)" = Linux ] || exit 0

apps_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons_root="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"

# Desktop + icon caches so a running GNOME/KDE picks up the new .desktop entry and icon
# immediately (absent tools / no index.theme are harmless).
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$apps_dir" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$icons_root" 2>/dev/null || true
fi
# Make systemd re-scan the user units so `systemctl --user enable ...` sees deckhandd.{service,socket}.
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload 2>/dev/null || true
fi

# Ensure ~/.local/bin (where the binaries land) is on PATH; if not, add it to ~/.bash_profile.
# The path is the fixed standard-layout location, so this holds for the unpacked-zip case too.
# Single-quoted so $HOME/$PATH are written literally and expanded at login, not now.
bin_dir="$HOME/.local/bin"
case ":$PATH:" in
    *":$bin_dir:"*)
        ;;  # already on PATH - nothing to do
    *)
        profile="$HOME/.bash_profile"
        if grep -qs '# Added by deckhand' "$profile"; then
            echo "NOTE: $bin_dir is not on your PATH yet; $profile already has the entry - log out and back in."
        else
            printf '\n# Added by deckhand\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$profile"
            echo "NOTE: $bin_dir was not on your PATH; added it to $profile - log out and back in for it to take effect."
        fi
        ;;
esac

echo "deckhand setup complete"
