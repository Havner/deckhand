#!/bin/sh
# Apply or remove an EDID override on the Steam Deck internal panel (eDP-1).
# Must run as root.  Not persistent: a reboot reverts it.
#
#   deck-edid-override.sh apply [edid.bin]
#   deck-edid-override.sh reset
#
# The panel's real EDID cannot be re-read at runtime (the AUX EDID read fails
# once booted; without an EDID amdgpu falls back to unusable VESA modes), so
# the link is never unplugged and the real EDID is never re-read.  Instead the
# connector is briefly forced on: amdgpu's .force hook then re-reads the EDID
# without a DDC probe, which returns the override straight from memory, and
# rebuilds the mode list from it.  "reset" therefore loads the saved factory
# EDID the same way rather than clearing the override.
set -eu

DIR=$(dirname "$(readlink -f "$0")")
# pick this panel's files by the EDID product ID (bytes 10-11, little endian)
case $(od -An -tx1 -j8 -N4 /sys/class/drm/card*-eDP-1/edid | tr -d ' \n') in
59960330) PANEL=oled-sdc ;;
59960430) PANEL=oled-boe ;;
59960130) PANEL=lcd-3001 ;;
59960230) PANEL=lcd-3002 ;;
*) PANEL= ;;
esac
if [ -n "$PANEL" ]; then
    EDID=${2:-$DIR/deck-$PANEL-multi.bin}
    ORIG=$DIR/deck-$PANEL-original.bin
else
    # no saved files for this panel: an EDID must be given, and the
    # connector's current EDID (still the factory one) becomes the fallback
    [ -n "${2:-}" ] || { echo "unknown panel: pass an EDID file, see README" >&2; exit 1; }
    EDID=$2
    ORIG=$(mktemp)
    cat /sys/class/drm/card*-eDP-1/edid > "$ORIG"
fi

mountpoint -q /sys/kernel/debug || mount -t debugfs none /sys/kernel/debug
# at boot amdgpu may still be probing; wait up to 15 s for the connector
for _ in $(seq 30); do
    CONN=$(ls -d /sys/kernel/debug/dri/*/eDP-1 2>/dev/null | head -n1)
    STATUS=$(ls /sys/class/drm/card*-eDP-1/status 2>/dev/null | head -n1)
    [ -n "$CONN" ] && [ -n "$STATUS" ] && break
    sleep 0.5
done
[ -n "$CONN" ] && [ -n "$STATUS" ] || { echo "eDP-1 not found" >&2; exit 1; }

case ${1:-} in
apply) ;;
reset) EDID=$ORIG ;;
*) echo "usage: $0 apply [edid.bin] | reset" >&2; exit 2 ;;
esac
[ -s "$EDID" ] && [ $(( $(wc -c < "$EDID") % 128 )) -eq 0 ] ||
    { echo "bad EDID file: $EDID" >&2; exit 1; }

# force must change value for the kernel to re-run the probe
load() {
    cat "$1" > "$CONN/edid_override"
    echo detect > "$STATUS"
    echo on > "$STATUS"
    echo detect > "$STATUS"
}
load "$EDID"
# sysfs reports st_size 0, and cmp -s on two regular files of different
# sizes says "differ" without reading them, so feed it through a pipe
if ! cat "$(dirname "$STATUS")/edid" | cmp -s "$EDID" -; then
    echo "EDID did not take effect, restoring factory EDID" >&2
    load "$ORIG"
fi

# tell the compositor to re-read connector modes
echo change > "$(dirname "$STATUS")/../uevent" 2>/dev/null || true
cat "$(dirname "$STATUS")/modes"
