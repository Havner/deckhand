# Steam Deck: extra refresh rates in Desktop Mode

Adds lower (and, on OLED, in-between) refresh rates to the Steam Deck's
internal panel for KDE Plasma (Wayland) in SteamOS Desktop Mode. In Game
Mode, gamescope generates such modes itself, but the panel EDIDs only list
the native rate, so KDE has no other choice.

This works by loading a modified EDID from the kernel command line. The new
modes use exactly the timings gamescope generates for the same panel.

This folder is a companion to deckhand, not part of it: deckhand doesn't need
it, and deckhand's `install.sh` doesn't install any of it, because installing
an EDID needs root. Requirements: `python3` (standard library only) for the
generator; optionally `edid-decode` and `drm_info` for checking the result.

## Panels

Every EDID carries one identity: a vendor code and a product number, in
bytes 8-11. All Deck panels are named `ANX7530 U` and are told apart by that
identity:

| Profile | EDID identity | Panel | Native | Added rates (default) | Status |
|---|---|---|---|---|---|
| `oled-sdc` | `VLV 0x3003` | OLED, Samsung | 90 Hz | 45 48 50 55 60 66 72 76 80 85 | **Tested** on hardware: installed, survives reboot and suspend |
| `oled-boe` | `VLV 0x3004` | OLED, BOE | 90 Hz | 45 48 50 55 60 66 72 76 80 85 | Untested, generated from a public EDID |
| `lcd` | `VLV 0x3001` | LCD | 60 Hz | 30 35 40 45 48 50 55 | **Tested** on hardware: installed, survives reboot and suspend |
| `lcd` | `VLV 0x3002` | LCD | 60 Hz | 30 35 40 45 48 50 55 | Untested, generated from a public EDID |
| `lcd` | `ANX 0x7539` | LCD | 60 Hz | 30 35 40 45 48 50 55 | Recognised by the generator; no factory EDID here |

To see which one a Deck has:

```
od -An -tx1 -j8 -N4 /sys/class/drm/card0-eDP-1/edid
# 59 96 03 30 = VLV 0x3003   59 96 01 30 = VLV 0x3001   05 d8 39 75 = ANX 0x7539
```

The vendor is three letters packed 5 bits each (`0x5996` = VLV), and the
product is stored low byte first (`03 30` = 0x3003).

**The LCD has several identities.** They most likely come from the display
bridge firmware rather than from the panel maker:
- **Same timing, different ID:** `ANX 0x7539` has exactly the same native
  timing as `VLV 0x3001` (69.27 MHz, H 32/20/20, V 16/2/26).
- **Newer timing:** `VLV 0x3002` uses a different native timing, the very
  one gamescope programs on every LCD.
- **Gamescope matches by name:** its LCD script treats every LCD the same
  way. It accepts the vendors `VLV`, `ANX` and `WLC` and the names
  `ANX7530 U` and `Jupiter`, so more identities probably exist.

`oled-sdc` and `lcd` (`VLV 0x3001`) were tested on SteamOS with KDE Plasma 6.7,
kernel 7.2 (neptune), on 2026-10-25. Their rates were confirmed in hardware with
`drm_info`.

## How it works

### OLED: fixed pixel clock, stretched vertical front porch

The OLED panels run at a fixed pixel clock and horizontal frequency (about
118 kHz). Their EDIDs declare a range of 45-90 Hz. Lower refresh rates come
from stretching the **vertical front porch (VFP)**, so each frame gets more
blank lines. That's what gamescope does:
`/usr/share/gamescope/scripts/00-gamescope/displays/valve.steamdeck.oled.lua`
has one VFP table per panel, applied by `gamescope.modegen.adjust_front_porch`.

| Rate | SDC VFP | SDC actual | BOE VFP | BOE actual |
|-----:|-----:|----------:|-----:|----------:|
| 90 (native) | 9 | 90.004 Hz | 8 | 90.061 Hz |
| 85 | 86 | 85.014 Hz | 84 | 85.158 Hz |
| 80 | 173 | 80.003 Hz | 172 | 80.109 Hz |
| 76 | 250 | 76.037 Hz | 252 | 76.011 Hz |
| 72 | 337 | 72.003 Hz | 336 | 72.137 Hz |
| 66 | 486 | 66.006 Hz | 488 | 66.045 Hz |
| 60 | 665 | 60.003 Hz | 664 | 60.163 Hz |
| 55 | 829 | 55.387 Hz | 828 | 55.552 Hz |
| 50 | 1058 | 50.015 Hz | 1064 | 50.034 Hz |
| 48 | 1157 | 48.002 Hz | 1156 | 48.169 Hz |
| 45 | 1321 | 45.002 Hz | 1320 | 45.168 Hz |

- **SDC:** 133.2 MHz, H 800/1076/1084/1128, V sync 1, back porch 22.
- **BOE:** 102.0 MHz, H 800/818/822/858, V sync 2, back porch 30.
- **About the BOE rates:** its VFP values are mostly multiples of 4, and the
  resulting rates sit up to 0.3% above the nominal value (60.16 Hz). That's
  what gamescope programs too.
- **Valve's rate list:** Valve deliberately leaves out some rates (46, 52,
  54, 57, 58, ...), and the SDC "55 Hz" really is 55.39 Hz. Both look like
  per-panel tuning, so the generator only accepts rates from Valve's list
  for the OLED profiles.

### LCD: fixed timing, scaled pixel clock

The LCD works the other way round. Gamescope
(`valve.steamdeck.lcd.lua`) sets fixed timings: H 40/4/40 and V 30/4/8, for
a total of 884x1322. It then picks the pixel clock for each rate with
`calc_max_clock`, so the horizontal frequency changes with the rate.

| Rate | Pixel clock | H freq | Actual |
|-----:|------:|------:|------:|
| 60 (native, 0x3001 / 0x3002) | 69.27 / 70.11 MHz | 79.4 kHz | 60.00 / 59.99 Hz |
| 55 | 64.28 MHz | 72.7 kHz | 55.004 Hz |
| 50 | 58.44 MHz | 66.1 kHz | 50.007 Hz |
| 48 | 56.10 MHz | 63.5 kHz | 48.004 Hz |
| 45 | 52.59 MHz | 59.5 kHz | 45.001 Hz |
| 40 | 46.75 MHz | 52.9 kHz | 40.003 Hz |
| 35 | 40.91 MHz | 46.3 kHz | 35.006 Hz |
| 30 | 35.06 MHz | 39.7 kHz | 30.000 Hz |

- **30 and 35 Hz are never used by Valve.** Gamescope offers the LCD only
  40-60 Hz, and that list hasn't changed since it was added (commit
  `b7f319ae`, Oct 2023). The 30/35 options in Game Mode are **frame-rate
  limits**, not panel rates:
  - The Frame Limit slider uses the Steam client's `supported_frame_rates`,
    and falls back to 10-60 when that list is empty (`steamui/sp.js`).
  - Steam then picks the largest whole multiple of the limit that's in the
    refresh list. "30 FPS" runs the panel at 60 Hz with each frame shown
    twice, and 35 FPS has no multiple between 40 and 60.
  - With the frame limit disabled, the separate Refresh Rate slider shows
    gamescope's refresh list. A user script in `~/.config/gamescope/scripts/`
    can replace that list, as in a Steam Community tutorial that sets 30-48
    Hz. With such a script the slider starts at 30.
  - Gamescope's LCD mode generator itself has no lower limit; it has scaled
    the pixel clock to any requested rate since 2021.
- **Below 40 Hz is at your own risk.** The generator allows 30-59 Hz and
  labels anything below 40 Hz "untested by Valve".
- **Clock rounding:** DisplayID Type I clocks are in 10 kHz units, so each
  clock is gamescope's value rounded up to the next 10 kHz (under 0.03%
  difference).
- **Native timing kept:** the native 60 Hz mode keeps the panel's own timing.

### Why DisplayID, and why a kernel parameter

**DisplayID:** a standard EDID detailed timing descriptor has only 6 bits for
the VFP (max 63), and the OLED modes need up to 1321. DisplayID Type I
timings use 16-bit fields. The Linux kernel parses them
(`drm_mode_displayid_detailed` in `drm_edid.c`). The LCD modes would fit in
a standard descriptor, but its 128-byte EDID has only two free slots. So all
profiles use the same layout: the original blocks unchanged (native mode
stays preferred, OLED HDR metadata kept), followed by DisplayID 1.3
extension blocks with up to five Type I timings each.

**Kernel parameter:** amdgpu reads the panel EDID once at boot and keeps that
copy. Every resume from suspend rebuilds the connector from it (`dm_resume` ->
`amdgpu_dm_update_connector_after_detect`). With `drm.edid_firmware`, the very
first read returns this file, so the copy amdgpu keeps is the modified one
and it survives suspend. amdgpu loads from the root filesystem, not the
initramfs, about 6 s into boot. By then the `/etc` overlay is mounted, so
`firmware_class.path=/etc/deck-edid` works and nothing has to go into the
read-only `/usr`.

## Files

| File | Purpose |
|------|---------|
| `deck-<profile>-original.bin` | Factory EDIDs. `oled-sdc` and `lcd-3001` were read from the tested Decks. `oled-boe` and `lcd-3002` come from the [linuxhw EDID database](https://github.com/linuxhw/EDID/tree/master/Digital/Valve). |
| `deck-<profile>-multi.bin` | The modified EDIDs, built by `gen_deck_edid.py` with the default rates. |
| `gen_deck_edid.py` | Generator for all panels. Detects the panel from the product ID, checks the native timing, and builds the modes exactly as gamescope does. |
| `deck-edid.cfg` | GRUB drop-in that adds the kernel parameters. **This is the installed method.** It names the SDC file; see below for other panels. |
| `deck-edid-override.sh` | Runtime override through debugfs; no reboot needed, but lost on suspend. For experiments only. Picks the panel's files by product ID. |

The rate switcher lives outside this folder, as `scripts/deckhand-refresh`
(it works on any KDE display). See
[Switching rates](#switching-rates-deckhand-refresh).

## Install

**On a Deck other than the tested SDC and 3001 units, build the EDID from that
Deck's own panel first.** The public EDIDs come from other people's units, and
firmware revisions differ. For example, the database's SDC EDID has
different HDR luminance values from the one tested here. The generator
checks that the native timing matches before building anything.

```
cd edid    # in the deckhand checkout
cp /sys/class/drm/card0-eDP-1/edid my-original.bin  # keep a copy of the factory EDID
./gen_deck_edid.py my-original.bin my-multi.bin     # prints the profile and modes
cmp my-original.bin deck-<profile>-original.bin    # optional: same as the reference?
```

The sysfs path can also be passed to the generator directly. sysfs reports a
size of 0, but the file reads fine. Only do this while the factory EDID is
active: once `drm.edid_firmware` or an override is in place, sysfs shows the
modified EDID, and the generator refuses it ("source already has DisplayID
blocks").

The generator keeps every original block byte for byte. It changes only the
extension count and checksum of block 0, then appends the new DisplayID
blocks.

The generator then does one of three things:
- **Known identity, matching timing:** it builds the EDID. This is the normal
  case.
- **Known identity, different native timing:** it stops with
  `unexpected native timing`. The firmware has changed something, so look
  at `edid-decode my-original.bin` before going further.
- **Unknown identity:** it stops with `unknown panel (...); use --profile`. If
  the EDID is clearly an 800x1280 Deck LCD, `--profile lcd` builds it anyway
  (without the timing check); then add the identity to `PROFILES`.

Then install it. `deck-edid.cfg` names `deck-oled-sdc-multi.bin`, so on
another panel either copy the EDID under that name or edit the name in the
`.cfg`:

```
sudo mkdir -p /etc/deck-edid /etc/default/grub.d
sudo cp my-multi.bin /etc/deck-edid/deck-oled-sdc-multi.bin   # or edit deck-edid.cfg
sudo cp deck-edid.cfg /etc/default/grub.d/
sudo update-grub
```

Reboot. `deck-edid.cfg` appends this to `GRUB_CMDLINE_LINUX_DEFAULT`:

```
drm.edid_firmware=eDP-1:deck-oled-sdc-multi.bin firmware_class.path=/etc/deck-edid
```

SteamOS runs `update-grub` after every system update
(`steamos-finalize-install`), and `/etc` survives updates, so the setting
persists. If the file is missing or invalid, the kernel logs an error and uses
the panel's real EDID, so a broken install can't stop the display working.

**Try before installing (untested panels):** `sudo ./deck-edid-override.sh
apply my-multi.bin` loads the EDID until the next suspend or reboot, without
touching the boot setup. Then pick a rate in System Settings. If a mode shows
a black screen, KDE reverts after 15 seconds. Start with a rate gamescope
uses (OLED 60, LCD 50) before the LCD's untested 30/35 Hz.
`sudo ./deck-edid-override.sh reset` loads the factory EDID back.

For a panel without saved files here, `deck-edid-override.sh` needs the EDID
file as an argument. It then uses the connector's current EDID as the
fallback for `reset`, which is the factory one only before the first `apply`.

## Verify

```
grep -o 'drm.edid_firmware=[^ ]*' /proc/cmdline         # parameter present
cat /sys/class/drm/card0-eDP-1/edid | cmp - /etc/deck-edid/deck-oled-sdc-multi.bin && echo active
journalctl -k -b | grep -iE 'EDID firmware|Invalid firmware EDID'   # should be empty
drm_info | grep -A12 Modes | grep 800                   # all modes
drm_info | grep -m1 'Mode:'                             # mode currently driven
```

`drm_info` takes the active refresh from the programmed timings, so
`800x1280@45.00` means the panel really runs at 45 Hz.

Choose the rate in **System Settings -> Display**. That window offers to
revert after 15 seconds if the screen goes black. `kscreen-doctor` changes
the mode with no safety net.

## Switching rates: `deckhand-refresh`

`deckhand-refresh` (`scripts/deckhand-refresh` in the deckhand tree) is a
small bash wrapper around `kscreen-doctor` (KDE Plasma) and `gdctl` (GNOME)
for switching rates from a terminal, a shortcut, SSH or a deckhand chord. It
isn't tied to the Deck: it works on any display either desktop drives, with
whatever modes that display offers. deckhand's `install.sh` installs it
into `~/.local/bin` on Linux (no root needed, unlike the EDID); by hand it's
`install -m 755 scripts/deckhand-refresh ~/.local/bin/`. Usage:

```
deckhand-refresh 61     # nearest available rate -> 60
deckhand-refresh inc    # next higher rate, stops at the highest (no wrap-around)
deckhand-refresh dec    # next lower rate; at the lowest it does nothing
deckhand-refresh        # print the output, its current rate and all available ones
```

- It picks the desktop from `XDG_CURRENT_DESKTOP`, or, without a session
  environment (SSH, TTY), from the running compositor (`kwin_wayland`/`kwin_x11`
  or `gnome-shell`).
- It only considers modes at the current resolution, sorted by refresh rate
  (one per rounded rate, so 59.94 and 60.00 count as one).
- KDE: modes come from `kscreen-doctor -j` (parsed with `jq`), the switch is
  `kscreen-doctor output.<name>.mode.<id>`.
- GNOME: modes come from Mutter's `GetCurrentState` D-Bus call (`busctl
  --json`, the same state `gdctl show` prints). VRR is a separate mode there
  (`...@120.000+vrr`), so only modes of the current kind (variable or fixed)
  are offered. `gdctl set` replaces the whole display configuration, so the
  script restates every logical monitor as it is (position, scale, transform,
  primary, each monitor's mode, color mode and RGB range) and changes only the
  mode of the target output.
- "Nearest" uses the actual rate, e.g. `55` picks 55.39 on SDC and `63` picks
  60.00 (60.003 is closer than 66.006). On an exact tie the lower rate wins.
- It acts on the primary output (KDE: the enabled one with the lowest
  priority, on the Deck that's `eDP-1`; GNOME: the primary logical
  monitor's). Set `OUTPUT=` to pick another by name, e.g. `OUTPUT=DP-1 deckhand-refresh inc`.
- When run over SSH it fills in `WAYLAND_DISPLAY=wayland-0`,
  `XDG_RUNTIME_DIR=/run/user/<uid>` and the session D-Bus address itself.
- There's no 15-second revert prompt on either desktop. KWin saves the chosen
  rate and restores it at the next login; on GNOME the change is temporary
  (not written to `monitors.xml`) and lasts until the next display
  reconfiguration or login.
- From deckhand, bind it with `CommandExecute` chords, e.g. Steam + D-pad
  up/down as two entries of the `chords` list in `chords.ron`:

  ```
  (buttons: [Steam, DpadUp],   action: CommandExecute(command: "deckhand-refresh", args: ["inc"])),
  (buttons: [Steam, DpadDown], action: CommandExecute(command: "deckhand-refresh", args: ["dec"])),
  ```

## Uninstall

```
sudo rm /etc/default/grub.d/deck-edid.cfg
sudo update-grub
sudo rm -r /etc/deck-edid     # optional
```

Then reboot.

## Changing the set of rates

```
./gen_deck_edid.py my-original.bin my-multi.bin 45 50 55 60 72   # e.g. OLED
./gen_deck_edid.py my-original.bin my-multi.bin 30 40 45 50       # e.g. LCD
edid-decode my-multi.bin | grep -A2 DTD                           # check
sudo cp my-multi.bin /etc/deck-edid/deck-oled-sdc-multi.bin       # then reboot
```

The installed file keeps the name `deck-edid.cfg` refers to.

- **Rates accepted:** the OLED profiles take only rates from gamescope's list
  (45, 47, 48, 49, 50, 51, 53, 55, 56, 59, 60, 62, 64, 65, 66, 68, 72, 73,
  76, 77, 78, 80, 81, 82, 84, 85, 86, 87, 88). The LCD profile takes 30-59.
- **Layout:** one DisplayID block holds 5 timings, and the generator adds
  blocks as needed.
- **Other panel IDs:** `--profile NAME` forces a profile for an EDID with an
  unknown identity, such as gamescope's `WLC`/`Jupiter` LCD variants.
  The native-timing check is then skipped with a warning.
- **No `update-grub` needed:** the kernel parameter doesn't change.

`edid-decode -c` reports a few conformance failures. The factory EDIDs
already have the ones in Block 0 and the CTA block. The two DisplayID ones
(no Display Parameters block, no preferred timing) are intentional: the
native rate stays the preferred mode. The kernel doesn't check either.

## Pitfalls

Found with the runtime method (`deck-edid-override.sh`) on the SDC OLED.
They matter to anyone experimenting without a reboot:

- **Never simulate an unplug/replug of eDP** (`trigger_hotplug 0` then `1` in
  amdgpu debugfs). On replug amdgpu could not read the panel's EDID
  (`EDID err: 2 ... No EDID read`). It fell back to 640x480, the panel went
  black, and only a hard reboot recovered it.
- **The panel's real EDID can't be re-read while the system is running.**
  Even a forced read with the screen on fails (`No EDID found on connector`).
  The kernel then uses generic VESA modes (1024x768 ... 640x480). So "reset" in
  the script loads the factory EDID back as an override instead of reading
  the panel.
- **A runtime override needs a forced re-probe.** Write debugfs
  `edid_override`, then sysfs `status` `detect` -> `on` -> `detect`. amdgpu's
  `.force` hook re-reads the EDID without probing the panel and gets the
  override. The link stays up.
- **A runtime override is lost on every resume** (see "Why DisplayID, and why
  a kernel parameter"), which is why the install uses `drm.edid_firmware`.
- The debugfs `edid_override` reset string must be exactly 5 bytes: use
  `printf reset`, not `echo reset`.
- `cmp -s file /sys/.../edid` always says "differ". sysfs reports size 0, and
  `cmp -s` gives up on a size mismatch without reading. Pipe it instead:
  `cat /sys/.../edid | cmp -s file -`.
- `amdgpu.freesync_video=1` doesn't help. It only adds modes on
  VRR-capable connectors, and the SDC panel reports `vrr_capable = 0`.
- Custom modes don't help either: `kscreen-doctor ... addCustomMode` (Plasma
  6.6 and later) generates CVT timings. Those change the horizontal frequency, which falls
  outside the OLED panels' 118-119 kHz range.

## References

- gamescope display scripts and modegen:
  https://github.com/ValveSoftware/gamescope
  (`scripts/00-gamescope/displays/valve.steamdeck.oled.lua`,
  `valve.steamdeck.lcd.lua`, `scripts/00-gamescope/common/modegen.lua`)
- linuxhw EDID database, Valve panels:
  https://github.com/linuxhw/EDID/tree/master/Digital/Valve
  (`VLV3001`-`VLV3004`)
- Linux 6.18 DRM sources: `drivers/gpu/drm/drm_edid.c`, `drm_edid_load.c`,
  `drm_displayid.c`, `drm_sysfs.c`, `amd/display/amdgpu_dm/amdgpu_dm.c`,
  https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git
- Arch Wiki, Steam Deck: https://wiki.archlinux.org/title/Steam_Deck
- LCD 30-48 Hz gamescope override tutorial:
  https://steamcommunity.com/app/1675200/discussions/0/599643530220043659/
- Panel IDs `0x3001`/`0x3003`/`0x3004`:
  https://github.com/2-X/steamdeck-oled-120hz

## Licence of the EDID files

The scripts here fall under deckhand's MIT OR Apache-2.0 licence. The EDID
data files don't all:

- `deck-oled-boe-original.bin` and `deck-lcd-3002-original.bin` are taken from
  the [linuxhw EDID database](https://github.com/linuxhw/EDID) (collected by
  Linux users at https://linux-hardware.org). They are licensed under [CC BY
  4.0](https://creativecommons.org/licenses/by/4.0/) and provided without
  warranty. Each is the first 128 x (extensions + 1) bytes of the database dump,
  otherwise unchanged.
- `deck-oled-boe-multi.bin` and `deck-lcd-3002-multi.bin` are **modified**
  versions of those files (extension count and checksum changed, DisplayID
  blocks appended by `gen_deck_edid.py`) and are likewise licensed under
  CC BY 4.0.
- `deck-oled-sdc-original.bin` and `deck-lcd-3001-original.bin` were read
  from Decks' own panels, and the matching `-multi.bin` files are built from
  them.
