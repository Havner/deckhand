# PLAN.md

Project plan for a Steam Input-like controller mapping tool.

## 0. GENERAL INFO

**Name.** **`deckhand`** - the product and Cargo-workspace name (the working directory
can be renamed to match later). Internal crates keep descriptive names (`steam-hid`,
`virt-out`, `vocab-hid`, `vocab-out`, `config`, `mapper`, `engine`, `ipc`, ...). "Steam"
is deliberately avoided in the product name (Valve trademark).

**What it is.** An implementation of something akin to Steam Input: it reads input
from Steam controllers and maps that input to mouse, keyboard, and gamepad output.

**Supported input devices.**
- Steam Controller (original)
- Steam Controller (new)
- Steam Deck (built-in controls)

**Output targets.**
- Mouse
- Keyboard
- Gamepad (virtual controller)

**Language.** Rust.

**Toolchain.** Rust **edition 2024**; track latest stable, **no MSRV policy** (fits the
break-freely stance - no stale-compiler baggage).

**License.** Dual **MIT OR Apache-2.0** (the Rust-ecosystem default). We take only
protocol *facts* from the GPL `hid-steam` kernel driver and the C# reference - our code
and API are our own - so we're free to choose.

**Multiplatform.** Targets multiple operating systems (Linux, Windows, macOS). The
working folder is synced by the user across several machines during development.

**Dependencies.** Only well-maintained, widely-used crates. No small, untested, or
unproven dependencies. A crate is acceptable when it is popular and actively
maintained (e.g. the HID access crate `hidapi`, which is the de-facto cross-platform
choice) and rejected otherwise, even if convenient.

**API design.** Always prioritize the cleanest, best API design for what we need now.
The public API is **not stable and does not need to be** (probably ever) - refactor or
break it freely whenever that yields a better design. **Never carry technical debt to
preserve a stable API.** Forward-extensibility tools (`#[non_exhaustive]`, etc.) are
fine when they genuinely help, but never as a crutch to avoid improving the design.
(One future exception, deferred: the on-disk **config format** becomes a versioned,
non-breaking user contract at stable release - see 3.)

**Cross-cutting conventions.** Logging/diagnostics via the `log` facade (with `env_logger`
in the daemon). Errors via `thiserror` in the HAL/engine library crates; binaries surface
those. Pin shared crate versions in `[workspace.dependencies]`.

**Structure.** A Rust workspace of **twelve crates** in a ports-and-adapters shape, layered
so the pure mapping core links no hardware. Bottom to top (the numbered ones keep their
own section below):
- **`vocab-hid`** / **`vocab-out`** - the leaf **vocabularies** (deps: serde, plus `bitflags`
  for `vocab-hid`). `vocab-hid` = controller **input** vocabulary; `vocab-out` = **output**
  vocabulary. Each is split `core` (the names `config` needs) + `state`/`event` (the snapshot /
  emit types the mapper adds): `vocab-hid` = `Button` | `Buttons`/`Axis`/`ControllerState`;
  `vocab-out` = `Key`/`MouseButton`/`GamepadButton`/`GamepadAxis` | `OutputEvent`.
- **`steam-hid`** (1) - input **HAL**: decode Steam-controller HID into a `vocab-hid::ControllerState`.
  The only crate that speaks `hidapi`.
- **`virt-out`** (2) - output **HAL**: realize a `vocab-out::OutputEvent` as a virtual mouse/
  keyboard/gamepad per OS.
- **`config`** (3) - the **authoring model**: on-disk RON, validation, the names of inputs/
  outputs. Depends on both vocab crates, **neither HAL**.
- **`mapper`** - the **pure mapping core**: evaluates a compiled `Program` (its own IR) over a
  `vocab-hid` frame into `vocab-out` events. Depends only on the vocab crates + config; **no HAL,
  no OS** - golden-testable with an injected clock. (`ConfigDoc -> Program` compilation lives in
  `engine`, the runtime's contract.)
- **`engine`** (4) - the headless **runtime / manager**: owns and drives the real devices
  (`steam-hid`) and the output sink (`virt-out`), compiles `config` into a `mapper::Program`, and
  runs the mapper on threads (a reader per device + a central mapping loop). Exposes an in-process
  API; also hosts the 6 network seam. **Not** a pure core - that is the `mapper` crate.
- **`ipc`** - the daemon control protocol (`Request`/`Response`/`Event` over a local socket).
- **`deckhandd`** / **`deckhandctl`** - the daemon (embeds `engine`, serves its API over `ipc`)
  and the thin CLI client.
- **`deckhand`** (5, iced) - the GUI: a tray configurator + profile editor, and an `ipc` client.
  **`forwarder-ui`** - a minimal touch-first `ipc` client for the Steam Deck forwarder case.

Build order: `vocab-out`/`vocab-hid` -> `steam-hid`/`virt-out`/`config` -> `mapper` -> `engine`
-> `ipc` -> `deckhandd`/`deckhandctl`/`deckhand`/`forwarder-ui`.

The **networked / remote-controller** capability (6 - e.g. a Steam Deck driving a gaming PC)
reuses these same crates: the network is another adapter around the engine (raw `ControllerState`
on the wire, the mapper on the sink), an extension, not a rewrite.

### 0.1 Design at a glance (the crate split)

One organizing idea: **the mapper is pure**. It reads one vocabulary and writes another, and
depends on nothing else - not a HAL, not the OS, not the engine. Everything hardware- or
process-shaped is layered around it.

**The core - read input vocab, write output vocab:**

    vocab-hid  -->  mapper (+ Program)  -->  vocab-out
   (input vocab)    the pure core:          (output vocab)
                    no HAL, no OS,
                    injected clock

**The mapping path - real hardware to virtual hardware:**

    hardware --> steam-hid --> vocab-hid --> mapper --> vocab-out --> virt-out --> virtual hardware
    (Steam        input HAL     input         pure       output        output        (Xbox pad,
     controller)  (hidapi)      vocab         core       vocab         HAL            kbd, mouse)
                                               ^
                                               |
                                            Program   (config compiled to the mapper's IR)

**The runtime path - `engine` composes the above into a running process:**

    config  --engine::compile()-->  Program ----.
                                                 v
            steam-hid  <--reader threads--  [ engine ]  --mapping loop, drives mapper-->  virt-out

`engine` owns the threads (one reader per device feeding a central loop over channels), compiles the
`ConfigDoc` it receives through its API into a `mapper::Program`, and feeds the mapper. It is headless
- no UI, no notion of a desktop or running apps.

**The process path - daemon + clients over `ipc`:**

    deckhandctl      deckhand (UI)      forwarder-ui       <- three ipc clients
         \                |                  /
          `----------->  ipc  <-------------'              <- control protocol (its own crate)
                          |
                     deckhandd        <- the daemon: embeds `engine`, exposes its API over `ipc`
                          |
                       engine

The **UI also edits configs directly** (not through the daemon): it uses the `config` crate to
build/validate a `ConfigDoc`, and `vocab-hid::core` + `vocab-out::core` to present the input/output
names a binding can choose. The compiled config is then shipped to the daemon over `ipc`.

**Note on the rest of this PLAN.** Sections 1-6 were written as each piece was designed, and several
crates above (`vocab-hid`/`vocab-out`, `mapper`, `ipc`, the split of the UI into `deckhand` /
`forwarder-ui`) did not exist when their surrounding sections were first drafted - the mapper and its
IR, for example, are described under section 4 (ENGINE) because they lived there originally. So the
per-section detail is **not** a complete per-crate reference; this 0.1 is the current map.

## 1. STEAM-HID

### 1.1 Scope

A standalone library crate that (a) discovers and opens Steam input devices, (b)
reads their full input state (digital, analog, gyro/IMU), and (c) sends commands to
them (lizard mode on/off, gyro on/off, haptics/rumble, LED, idle timeout, power off,
plus a raw escape hatch). It is the input device layer only - no mapping (that is
`engine`, 4), no virtual output (that is `virt-out`, 2). Built to be embedded cleanly
by the rest of the workspace (consumed by `engine`; its types feed the mapping core).

### 1.2 Reference implementation & attribution

Initial work is based on the C# Steam HID API implementation in the sibling folder
`../steam-hidapi.net`. We take from it **only the device-level HID protocol** (report
byte layouts, feature-report command bytes, init sequences, VID/PIDs) - documented in
1.4. We do **not** mirror the C# library's public API surface; the Rust API (1.5) is
our own idiomatic design.

Additional protocol references (all siblings of the working dir): the Linux **`hid-steam`**
kernel driver (authoritative command/setting IDs - we adopt its naming), **SDL**
(`../SDL`, the Triton driver + the shared `steam/controller_{constants,structs}.h` - e.g.
`MsgSimpleRumbleCmd`/`MsgHapticRumble`/`NCHapticPacket2`), **sc-controller** (Triton BLE +
v2 framing), and **InputPlumber** (`../InputPlumber`, a Rust device-translation daemon -
useful cross-check for the Deck's HID report + haptic structs, though it mislabels a few
fields; see the `0xea`/`0xeb` notes in 1.4 and the emulated-PID caveat in 1.3).

### 1.3 Supported devices

Valve VID = `0x28DE`. Valve codenames: **Gordon** = original Steam Controller,
**Neptune** = Steam Deck, **Triton** = new Steam Controller (2026).

| Device | Codename | PID | Input event type |
|---|---|---|---|
| Steam Controller, wired | Gordon | `0x1102` | `INPUT_DATA` (0x01) |
| Steam Controller, wireless dongle | Gordon | `0x1142` | `INPUT_DATA` (0x01) |
| Steam Controller, Bluetooth | Gordon | `0x1106` | compact BLE stream (1.4 BLE) |
| Steam Deck | Neptune | `0x1205` | `DECK_INPUT_DATA` (0x09) |
| New Steam Controller (2026), wired | Triton | `0x1302` | Triton state, id in byte 0 (1.4 Triton) |
| New Steam Controller (2026), Bluetooth | Triton | `0x1303` | Triton state (1.4 Triton) |
| New Steam Controller (2026), "puck" dongle | Triton | `0x1304` | Triton state (1.4 Triton) |

**Bluetooth (BLE) is HW-verified** (1.9): same Gordon commands, different transport +
input format - see "BLE transport" in 1.4.

**New Steam Controller (Triton) - HW-verified** on real hardware across all three transports
(puck/wired/BT), reverse-engineered from SDL + sc-controller - see **1.4 Triton**. SDL's 2nd
puck variant "Nereid" `0x1305` is not added until one is seen in the wild.

**`0x28de:0x12f0`-`0x12ff` are NOT real Valve hardware - do not enumerate them.** This range
is **InputPlumber's *emulated* Steam Deck** target: it reads a Linux handheld's native
controller and re-emits a virtual Steam-Deck-protocol device under Valve's VID, one PID per
brand so SDL/Steam can still tell them apart - `0x12f0` Generic, `0x12fa` MSI Claw, `0x12fb`
Lenovo Legion Go 2, `0x12fc` Zotac Zone, `0x12fd` ASUS ROG Ally, `0x12fe` Lenovo Legion Go,
`0x12ff` Lenovo Legion Go S (from `../InputPlumber` `src/drivers/steam_deck/mod.rs`). They
speak the Neptune protocol by construction, so matching them would "work" but bind the wrong
physical device - keep enumeration pinned to the real PIDs above (this also resolves the old
mystery of the C#/reference `0x12f0`). The `0x12f0` value is also what InputPlumber's uhid
path emits for an unbranded emulated Deck.

### 1.4 Protocol reference (ground truth)

**Framing.** 64-byte HID reports. Input reports begin `01 00 <event>` where `event` is
`INPUT_DATA=0x01`, `CONNECT=0x03`, `BATTERY=0x04`, or `DECK_INPUT_DATA=0x09`. Gordon
delivers state as `INPUT_DATA`; Neptune as `DECK_INPUT_DATA`.

**Commands** are HID **feature reports** shaped `[cmd_id, payload_len, ...payload]`.
Full command-ID set from the Linux `hid-steam` driver (v6.18.30) - **kernel naming**,
the authoritative superset (C# used a small subset). Names credited to Valve/SDL.

```
0x80 ID_SET_DIGITAL_MAPPINGS          0xAF ID_RADIO_ERASE_RECORDS
0x81 ID_CLEAR_DIGITAL_MAPPINGS        0xB0 ID_RADIO_WRITE_RECORD
0x82 ID_GET_DIGITAL_MAPPINGS          0xB1 ID_SET_DONGLE_SETTING
0x83 ID_GET_ATTRIBUTES_VALUES         0xB2 ID_DONGLE_DISCONNECT_DEVICE
0x84 ID_GET_ATTRIBUTE_LABEL           0xB3 ID_DONGLE_COMMIT_DEVICE
0x85 ID_SET_DEFAULT_DIGITAL_MAPPINGS  0xB4 ID_DONGLE_GET_WIRELESS_STATE
0x86 ID_FACTORY_RESET                 0xB5 ID_CALIBRATE_GYRO
0x87 ID_SET_SETTINGS_VALUES           0xB6 ID_PLAY_AUDIO
0x88 ID_CLEAR_SETTINGS_VALUES         0xB7 ID_AUDIO_UPDATE_START
0x89 ID_GET_SETTINGS_VALUES           0xB8 ID_AUDIO_UPDATE_DATA
0x8A ID_GET_SETTING_LABEL             0xB9 ID_AUDIO_UPDATE_COMPLETE
0x8B ID_GET_SETTINGS_MAXS             0xBA ID_GET_CHIPID
0x8C ID_GET_SETTINGS_DEFAULTS         0xBF ID_CALIBRATE_JOYSTICK
0x8D ID_SET_CONTROLLER_MODE           0xC0 ID_CALIBRATE_ANALOG_TRIGGERS
0x8E ID_LOAD_DEFAULT_SETTINGS         0xC1 ID_SET_AUDIO_MAPPING
0x8F ID_TRIGGER_HAPTIC_PULSE          0xC2 ID_CHECK_GYRO_FW_LOAD
0x9F ID_TURN_OFF_CONTROLLER           0xC3 ID_CALIBRATE_ANALOG
0xA1 ID_GET_DEVICE_INFO               0xC4 ID_DONGLE_GET_CONNECTED_SLOTS
0xA7 ID_CALIBRATE_TRACKPADS           0xCE ID_RESET_IMU
0xA8 ID_RESERVED_0                    0xEA ID_TRIGGER_HAPTIC_CMD
0xA9 ID_SET_SERIAL_NUMBER             0xEB ID_TRIGGER_RUMBLE_CMD
0xAA ID_GET_TRACKPAD_CALIBRATION
0xAB ID_GET_TRACKPAD_FACTORY_CALIBRATION
0xAC ID_GET_TRACKPAD_RAW_DATA
0xAD ID_ENABLE_PAIRING
0xAE ID_GET_STRING_ATTRIBUTE
```

Two further command IDs are **not in the kernel/C# refs** but appear in InputPlumber's
Steam Deck `ReportType` enum (as `UnknownDc`/`UnknownE2`): `0xDC` and `0xE2` - undecoded
(no known payload). Their presence in the *Deck's* enum means they're part of the shared
Valve command space, not Triton-only. sc-controller's Triton **v2 pairing** capture groups
them with `0xED` (a third undecoded, v2-only ID **not** in SDL/InputPlumber) and `0xAD` -
but `0xAD` is the already-known `ID_ENABLE_PAIRING` (in SDL's `controller_constants.h` and
the kernel, listed above), merely *reused* in that flow, not a mystery. SDL's Triton driver
references none of these (it does input+haptics only; puck pairing is out of its scope).
Left as reverse-engineering leads; needs a USB packet dump to decode ((!) pairing/radio-
adjacent - probe with care).

C#->kernel notes: C#'s `DEFAULT_MOUSE` (`0x8e`) is really `ID_LOAD_DEFAULT_SETTINGS`;
C#'s "register" ops = kernel "settings" ops (`0x87`-`0x8c`); C#'s `GET_SERIAL` (`0xae`)
is the generic `ID_GET_STRING_ATTRIBUTE` (serial = string-attribute id 1).

**Settings** (C# "registers") are written via `ID_SET_SETTINGS_VALUES`
(`87 <len> (<id> <lo> <hi>)...`, little-endian). The kernel `SETTING_*` enum is a flat
list where the **index is the setting id**. Full list recorded below (kernel naming):

```
 0 MOUSE_SENSITIVITY                          41 HAPTIC_INTENSITY_MOUSE_MODE
 1 MOUSE_ACCELERATION                         42 LEFT_DPAD_REQUIRES_CLICK
 2 TRACKBALL_ROTATION_ANGLE                   43 RIGHT_DPAD_REQUIRES_CLICK
 3 HAPTIC_INTENSITY_UNUSED                    44 LED_BASELINE_BRIGHTNESS
 4 LEFT_GAMEPAD_STICK_ENABLED                 45 LED_USER_BRIGHTNESS
 5 RIGHT_GAMEPAD_STICK_ENABLED                46 ENABLE_RAW_JOYSTICK
 6 USB_DEBUG_MODE                             47 ENABLE_FAST_SCAN
 7 LEFT_TRACKPAD_MODE                         48 IMU_MODE
 8 RIGHT_TRACKPAD_MODE                        49 WIRELESS_PACKET_VERSION
 9 MOUSE_POINTER_ENABLED                      50 SLEEP_INACTIVITY_TIMEOUT
10 DPAD_DEADZONE                              51 TRACKPAD_NOISE_THRESHOLD
11 MINIMUM_MOMENTUM_VEL                       52 LEFT_TRACKPAD_CLICK_PRESSURE
12 MOMENTUM_DECAY_AMMOUNT                     53 RIGHT_TRACKPAD_CLICK_PRESSURE
13 TRACKPAD_RELATIVE_MODE_TICKS_PER_PIXEL     54 LEFT_BUMPER_CLICK_PRESSURE
14 HAPTIC_INCREMENT                           55 RIGHT_BUMPER_CLICK_PRESSURE
15 DPAD_ANGLE_SIN                             56 LEFT_GRIP_CLICK_PRESSURE
16 DPAD_ANGLE_COS                             57 RIGHT_GRIP_CLICK_PRESSURE
17 MOMENTUM_VERTICAL_DIVISOR                  58 LEFT_GRIP2_CLICK_PRESSURE
18 MOMENTUM_MAXIMUM_VELOCITY                  59 RIGHT_GRIP2_CLICK_PRESSURE
19 TRACKPAD_Z_ON                              60 PRESSURE_MODE
20 TRACKPAD_Z_OFF                             61 CONTROLLER_TEST_MODE
21 SENSITIVY_SCALE_AMMOUNT                    62 TRIGGER_MODE
22 LEFT_TRACKPAD_SECONDARY_MODE               63 TRACKPAD_Z_THRESHOLD
23 RIGHT_TRACKPAD_SECONDARY_MODE              64 FRAME_RATE
24 SMOOTH_ABSOLUTE_MOUSE                      65 TRACKPAD_FILT_CTRL
25 STEAMBUTTON_POWEROFF_TIME                  66 TRACKPAD_CLIP
26 UNUSED_1                                   67 DEBUG_OUTPUT_SELECT
27 TRACKPAD_OUTER_RADIUS                      68 TRIGGER_THRESHOLD_PERCENT
28 TRACKPAD_Z_ON_LEFT                         69 TRACKPAD_FREQUENCY_HOPPING
29 TRACKPAD_Z_OFF_LEFT                        70 HAPTICS_ENABLED
30 TRACKPAD_OUTER_SPIN_VEL                    71 STEAM_WATCHDOG_ENABLE
31 TRACKPAD_OUTER_SPIN_RADIUS                 72 TIMP_TOUCH_THRESHOLD_ON
32 TRACKPAD_OUTER_SPIN_HORIZONTAL_ONLY        73 TIMP_TOUCH_THRESHOLD_OFF
33 TRACKPAD_RELATIVE_MODE_DEADZONE            74 FREQ_HOPPING
34 TRACKPAD_RELATIVE_MODE_MAX_VEL             75 TEST_CONTROL
35 TRACKPAD_RELATIVE_MODE_INVERT_Y            76 HAPTIC_MASTER_GAIN_DB
36 TRACKPAD_DOUBLE_TAP_BEEP_ENABLED           77 THUMB_TOUCH_THRESH
37 TRACKPAD_DOUBLE_TAP_BEEP_PERIOD            78 DEVICE_POWER_STATUS
38 TRACKPAD_DOUBLE_TAP_BEEP_COUNT             79 HAPTIC_INTENSITY
39 TRACKPAD_OUTER_RADIUS_RELEASE_ON_TRANSITION 80 STABILIZER_ENABLED
40 RADIAL_MODE_ANGLE                          81 TIMP_MODE_MTE
```

**Used initially** (only these low, known settings; the rest are recorded for later
testing, **not** for the first implementation): `7 LEFT_TRACKPAD_MODE` (C# LPAD_MODE),
`8 RIGHT_TRACKPAD_MODE` (C# RPAD_MODE), `45 LED_USER_BRIGHTNESS` (C# LED_INTENSITY),
`48 IMU_MODE` (C# GYRO_MODE), `50 SLEEP_INACTIVITY_TIMEOUT` (C# IDLE_TIMEOUT),
`52/53 LEFT/RIGHT_TRACKPAD_CLICK_PRESSURE`.

`IMU_MODE` (48) bitmask - **matches C# `GCGyroMode` exactly**: `OFF=0`, `STEERING=1`,
`TILT=2`, `SEND_ORIENTATION=4`, `SEND_RAW_ACCEL=8`, `SEND_RAW_GYRO=0x10`. Trackpad modes
(for `LEFT/RIGHT_TRACKPAD_MODE`): `ABSOLUTE_MOUSE=0`, `RELATIVE_MOUSE=1`, dpad 2-4,
`RADIAL=5`, `ABSOLUTE_DPAD=6`, `NONE=7` (disables the pad for raw input),
`GESTURE_KEYBOARD=8`. Pad ids: `LEFT=0`, `RIGHT=1`, `BOTH=2`.

**Key command sequences** (kernel naming):

| Purpose | Sequence |
|---|---|
| Write settings | `87 <len> (<setting_id> <lo> <hi>)...` |
| Lizard OFF (raw) | `81` (`CLEAR_DIGITAL_MAPPINGS`), then write `LEFT/RIGHT_TRACKPAD_MODE=NONE(7)`; Deck also click-pressures `0xFFFF` + `STEAM_WATCHDOG_ENABLE=0` |
| Lizard ON | `85` (`SET_DEFAULT_DIGITAL_MAPPINGS`), then `8e` (`LOAD_DEFAULT_SETTINGS`) |
| Gyro/IMU enable | write `IMU_MODE(48) = SEND_RAW_ACCEL\|SEND_RAW_GYRO (0x18)`; `0` to disable |
| Power off | `9f 04 6f 66 66 21` ("off!") |
| Get serial | `ae 15 01` (`ATTRIB_STR_UNIT_SERIAL`); reply serial from offset 4 |

**Haptics - HW-verified** (full exploration in 1.9; the byte layouts below are confirmed).
Each reference decodes a different subset: the kernel `hid-steam` and SDL for `0x8f`/`0xeb`
(and Triton's output reports), **C# and InputPlumber for `0xea`** (kernel/SDL only *define* the
id and never send it), sc-controller for Triton framing.

- **`0x8f` `ID_TRIGGER_HAPTIC_PULSE`** - trackpad-actuator pulse (Gordon's only haptic; works on
  the Deck too). `[8f 08 <pad> <duration:u16> <interval:u16> <count:u16> <gain:i8>]`. Pad map
  HW-verified: wire **0 = RIGHT, 1 = LEFT** (kernel legacy swap); **`pad=2`/BOTH no-ops on Gordon**
  (fire both pads separately). `gain` (dB -24..+6) is **honored on the Deck, ignored on Gordon**
  (Gordon's amplitude lever is the **duty cycle**). Subsumes C#'s len-7 no-gain variant.
- **`0xeb` `ID_TRIGGER_RUMBLE_CMD`** - the Deck's dual-motor rumble (kernel `FF_RUMBLE`).
  `[eb 09 <unRumbleType> <intensity:u16> <left_speed:u16> <right_speed:u16> <left_gain:i8> <right_gain:i8>]`
  (kernel `steam_haptic_rumble` writes `report[3]`=intensity LSB / `report[4]`=MSB; SDL
  `MsgSimpleRumbleCmd`). **`intensity` is a u16 LE**, an *inverted* fine amplitude lever (`0` =
  strongest) - a low-**byte**-only HW sweep *looks* inert but that's just the LSB (same as the low
  bytes of `left`/`right`); **InputPlumber's "event_type:u8 + intensity:u8" split is wrong.**
  **`unRumbleType` (report[2]) is HW-confirmed inert** (swept 0..255, no effect) - every reference
  sends 0. `left`/`right` = pulse **rate**; `*_gain` dB = coarse amplitude. Fixed ~0.5 s burst,
  re-issue to sustain, `(0,0)` stops. **Deck-only** (no motors on Gordon).
- **`0xea` `ID_TRIGGER_HAPTIC_CMD`** - the Deck's finely-tuned trackpad **click** (now our
  command-click; strongest beats a full `0x8f`). SDL/kernel only *define* the id and **never send
  it** - **C# `NCHapticPacket2` and InputPlumber `PackedHapticReport` are the only payload refs.**
  Layout (C# framing, kept): `[ea 0d <side> <style> <intensity> <gain:i8> 04 <tsA:i32> <tsB:i32>]`.
  - `side`: 0=L, 1=R, **2=Both** - all HW-verified (`Motor::Both`).
  - `style` = `HapticStyle` off/weak/strong (0/1/2) - **== InputPlumber's `cmd_type` off/tick/click.**
  - `intensity` = `HapticIntensity` 0..4 (InputPlumber names: Default/Short/Medium/Long/Insane) - the
    byte C# hard-codes to 0. **HW: 0..2 identical, 3 stronger, 4 stronger/different character; values
    outside 0..4 do nothing.**
  - `gain` i8 dB (C# `-7..5` => ~`-2..+10`).
  - Trailing `04` + two `TickCount` words are unexposed; InputPlumber's alternate fixed tail
    (`5F CC 03 ... 10`) made **no HW difference**, so we keep the C# tail.
- **Triton output-report haptics** ride the interrupt-OUT endpoint (not feature reports): `0x80`
  rumble = SDL `MsgHapticRumble` `[80 <unRumbleType> <intensity:u16> <left{speed:u16,gain:i8}>
  <right{speed:u16,gain:i8}>]` - **same `unRumbleType`-inert / `intensity`-u16 facts as `0xeb`**;
  `0x82` click = `[82 <side> <style> <amplitude:u8>]` (side honors Both). Full Triton detail in 1.9.

- **Neptune keep-alive - DONE (in the engine reader, not a steam-hid thread; HW-verified).** The
  Deck reverts to lizard mode unless lizard-off is re-asserted periodically - **measured on HW:
  revert ~10 s after lizard-off**, so any heartbeat well under 10 s suffices. The engine reader
  thread re-asserts lizard-off **every ~2 s, Neptune-gated** (`ReaderCfg::keepalive`), interleaved
  with its poll/rumble/click writes - **not** a background thread inside steam-hid (`Device` is the
  single writer: Send, not Sync, so the cadence belongs in the owner's read loop). **HW-verified**
  holding a real Deck alive across a multi-minute network-mode session (never dropped to lizard).
  On `CONNECT` (Gordon), previously-set config (lizard/gyro/idle) is restored.

**Input report layouts** (both 64 B, packed). Offsets are byte positions in the report.

`INPUT_DATA` - Gordon:

| Off | Field | Type | Off | Field | Type |
|---|---|---|---|---|---|
| 0x04 | seq | u32 | 0x1C | accel x/y/z | 3xi16 |
| 0x08 | buttons0/1/2 | 3xu8 | 0x22 | gyro x/y/z (pitch/roll/yaw) | 3xi16 |
| 0x0B | left/right trigger | 2xu8 | 0x28 | quaternion q1..q4 | 4xi16 |
| 0x10 | lpad x/y | 2xi16 | 0x32 | wireless "unfiltered" trig/joy/pad | i16... |
| 0x14 | rpad x/y | 2xi16 | 0x3E | battery | u16 |
| 0x18 | wireless trig l/r | 2xi16 | | | |

`DECK_INPUT_DATA` - Neptune:

| Off | Field | Type | Off | Field | Type |
|---|---|---|---|---|---|
| 0x04 | seq | u32 | 0x24 | quaternion q1..q4 | 4xi16 |
| 0x08 | buttons0/1/2/3 | 4xu8 | 0x2C | left/right trigger | 2xi16 |
| 0x0D | buttons5/6 | 2xu8 | 0x30 | left stick x/y | 2xi16 |
| 0x10 | lpad x/y | 2xi16 | 0x34 | right stick x/y | 2xi16 |
| 0x14 | rpad x/y | 2xi16 | 0x38 | lpad pressure | i16 |
| 0x18 | accel x/y/z | 3xi16 | 0x3A | rpad pressure | i16 |
| 0x1E | gyro pitch/yaw/roll | 3xi16 | 0x3C | l/r stick force | 2xi16 |

**Stick capacitive force (`0x3C`/`0x3E`, Neptune-only, InputPlumber-sourced).** SDL's
`SteamDeckStatePacket_t` and the kernel stop at pad pressure (`0x3A`); only InputPlumber reads
`0x3C`/`0x3E` (it names them thumbstick "force"). **HW-verified** they carry a raw capacitive
magnitude - idles ~`-5..0`, ~`380..450` pressed, per-stick (InputPlumber's `STICK_FORCE_MAX = 112`
is **wrong**), most likely the analog the firmware thresholds into the binary stick-**touch** bit.
Parsed into `NeptuneReport.{left,right}_stick_force` **for reference only - deliberately NOT exposed
as a bindable input** (a stick-pressure axis is meaningless; even pad pressure isn't surfaced).
**Triton has no equivalent** - SDL's `TritonMTUNoQuat_t`/`Full`/Ibex bodies carry pad pressure but
no stick-force field. Probe: `cargo run -p steam-hid --example stickforce`.

Button bitfields (per-device wire bits) map to a unified button set in the parser.
Superset of inputs across both devices: A/B/X/Y, Dpad, LB/RB, LT/RT, LGRIP/RGRIP,
LGRIP2/RGRIP2, View/Menu/Steam/QuickAccess, left/right pad press+touch, left/right stick
press+touch.

**Layouts kept as-is; button naming unified.** The kernel confirmed the Deck (`0x09`)
layout byte-for-byte and added nothing new; for Gordon it parses *less* (no IMU), whereas
the C# Gordon IMU offsets (accel/gyro/quaternion) are in daily use and known-good - so we
keep the current field layouts. **Button names are unified across all layers** onto one
scheme (was C#'s `L1/L2/L4/L5`): `LB/RB` bumpers, `LT/RT` trigger full-pulls, `LGRIP/RGRIP`
(+ `LGRIP2/RGRIP2` on the Deck) back buttons - deliberate abbreviations of the descriptive
`config::InputSource` names (`LeftBumper`/`LeftTriggerFull`/`LeftGrip`/`LeftGrip2`), and
matching the **kernel's** `GRIPL/GRIPL2` for the grips and **sc-controller's** `LB/RB, LT/RT`
for shoulders/triggers (bumpers/triggers pick sc-controller/Xbox over the kernel's cryptic
`TL/TR, TL2/TR2` - UX over provenance, deliberately). The two small top buttons are Valve's
**`View`** ([copy], left/select, kernel `BTN_SELECT`) and **`Menu`** ([menu], right/start, `BTN_START`)
- NOT C#'s misleading `Menu`/`Options` - unified across `steam-hid`/`config`/`engine`; the
output layer (`vocab`, an Xbox pad) correctly uses `Back`/`Start` there instead. **Gyro
naming:** HW-verified (1.9) the three Gordon gyro `i16`s at `0x22/0x24/0x26` are angular
velocity about `X/Y/Z` = **pitch/roll/yaw**, not the C# `gpitch/gyaw/groll` (yaw/roll
transposed); offsets correct, names dropped (see the IMU-frame note under `ControllerState`).

**Scale constants** (from the kernel; useful for normalization / IMU -> physical units).
The C# *app* likely has equivalents outside the HID lib we read; treat these as a
starting point and verify empirically:
`STEAM_DECK_ACCEL_RES_PER_G = 16384` (raw/g), `STEAM_DECK_GYRO_RES_PER_DPS = 16`
(raw/deg*s^-1), `STEAM_DECK_TRIGGER_RESOLUTION = 5461`, `STEAM_DECK_JOYSTICK_RESOLUTION
= 6553`, `STEAM_PAD_RESOLUTION = 1638`, `STEAM_TRIGGER_RESOLUTION = STEAM_JOYSTICK_RESOLUTION = 51`.

**Protocol caveats to carry into implementation:**
- Offset comments in the C# `GCInput` struct contain typos (e.g. `rpad_x` labelled
  `0x13`, actually `0x14`); the **sequential packed layout is authoritative**.
- Gordon (USB) multiplexes left-pad vs left-stick - **both the `lpad_x/y` coordinates and
  the shared left-click bit** - keyed on `LPAD_TOUCH` (+ `LPAD_AND_JOY`). Resolved entirely
  in `parse_gordon`; full behaviour incl. simultaneous pad+stick use in 1.9. (Not a
  wired-vs-wireless divide - both USB transports do it; BLE has no multiplex at all.)
- Haptics packets (`0x8f` / `0xea` / `0xeb`) are all under test - see the Haptics block
  above; none is trusted yet.
- **Feature reports - confirmed against the Linux `hid-steam` driver (v6.18.30):** all
  commands and reads use HID **feature reports** with **report ID 0**.
  - **Setters** (lizard, gyro, rumble, power off, register writes) need only
    `SET_REPORT` (`send_feature_report`) to take effect - no read-back required. (The
    C# `RequestFeatureReport` always did a GET too, which merely hid this.)
  - **Queries** (serial, read register, attribs) are `SET_REPORT` then `GET_REPORT`
    (`get_feature_report`); the GET fetches the reply.
  - Register writes (`0x87`) may be followed by a GET purely to **drain a lingering
    reply** the device queues, so it can't surface later and corrupt the next query's
    read - this is hygiene, not needed for the write itself. Add the drain only if we
    observe stale reads.
  - **Report-ID-0 framing:** prepend a `0x00` report-ID byte and pad the `SET_REPORT`
    buffer to >=64 bytes (the kernel sends `max(size, 64) + 1`).
  - **Wireless `EPIPE`:** the dongle intermittently fails `SET_REPORT` with `EPIPE`;
    the kernel retries up to 50x with 20 ms sleeps. Our dongle path (primary dev
    target) should replicate retry-on-`EPIPE`.

**Bluetooth (BLE) transport - Gordon `0x1106` (HW-verified, 1.9).** The kernel
`hid-steam` driver has **no Bluetooth entry** (its device table is USB-only), so BLE is
reverse-engineered from **SDL** (`../SDL/src/joystick/hidapi/SDL_hidapi_steam.c` +
`steam/controller_{constants,structs}.h`) and **sc-controller**
(`../sc-controller/scc/drivers/sc_by_bt.{py,c}`). sc-controller's input parser is
*approximate*; SDL's `UpdateBLESteamControllerState` is authoritative. **The command
bytes are identical to USB** - only the transport framing and the *input* format differ.

- **Everything rides Report ID 3 on 20-byte HID reports** (1 report id + 1 header + 18
  payload - the BLE ATT_MTU cap). Both feature writes **and** input longer than 18 bytes
  are **segmented**. Segment header byte = `0x80 (DATA) | segnum(0..7) | 0x40 (LAST)`; a
  single-segment packet's header is `0xC0`. (SDL: `MAX_REPORT_SEGMENT_PAYLOAD_SIZE=18`,
  `REPORT_SEGMENT_DATA_FLAG=0x80`, `_LAST_FLAG=0x40`, `BLE_REPORT_NUMBER=0x03`.)
- **Feature writes:** split the logical `[cmd_id, len, payload]` into <=18-byte segments,
  each sent as `[0x03][header][chunk, zero-padded to 20]`. Same command IDs as USB, so
  lizard-off / settings / gyro / `0x8f` haptics / LED / idle reuse the existing helpers.
- **Input is a compact DELTA stream** (not the fixed 64-byte USB report). Reassembled
  payload: `byte0` low nibble = **report type** (`4`=State, `5`=Status), `(byte0 & 0xF0)
  | (byte1 << 8)` = **chunk mask**; only present chunks follow, in ascending bit order:

  | Bit | Chunk | Bytes | | Bit | Chunk | Bytes |
  |---|---|---|---|---|---|---|
  | 0x0010 | Button1 (low 3 button bytes) | 3 | | 0x0200 | RightTrackpad x/y | 4 |
  | 0x0020 | Triggers L/R | 2 | | 0x0400 | Accel x/y/z | 6 |
  | 0x0040 | Button3 (high bytes; unused on SC) | 3 | | 0x0800 | Gyro x/y/z | 6 |
  | 0x0080 | LeftStick x/y | 4 | | 0x1000 | Quat w/x/y/z | 8 |
  | 0x0100 | LeftTrackpad x/y | 4 | | | | |

  `Device` reassembles segments and **accumulates** chunk updates into a `GordonReport` (the
  *same* type USB produces - see convergence below), emitting a full snapshot per input packet
  (`seq` synthesized - the wire carries none).
- **Button bit layout is identical to USB `GordonButtons`** - same positions *and* names, so
  **USB and BLE share one `GordonButtons` / `GordonReport` / fold** (the separate `GordonBle*`
  types were removed at convergence); only the two *parsers* differ (`parse_gordon` for the
  64-byte USB frame, `apply_gordon_ble` for the reassembled BLE delta). **Dpad works over BT**
  (bits 8..11, synthesized from left-pad directional clicks - HW-verified). Crucially BLE has
  **no left multiplex** - stick and pad are separate chunks and the pad-click / stick-press /
  touch bits are independent - so `apply_gordon_ble` produces already-clean data and **never**
  runs `parse_gordon`'s de-multiplex (hardware-accurate: BT is not forced through it).
- **IMU raw == USB Gordon raw** (all axes/signs HW-verified equal), so BLE flows through the
  shared `from_gordon` and gets the **same `gordon_gyro` y-negation**. The
  **orientation quaternion** (`0x1000`) is only sent if `SEND_ORIENTATION` (`0x04`) is set;
  we enable only `0x18` (raw accel+gyro), so `orientation` stays default (unused downstream
  - USB sends it every frame regardless; enabling it on BT costs an extra 20-byte segment).
- **udev:** BLE is created via `/dev/uhid` (modern BlueZ), landing under
  `/devices/virtual/misc/uhid/...` with **no `id/vendor` attribute** in the sysfs chain - so
  `ATTRS{idVendor}`/`ATTRS{id/vendor}` can't match. The rule matches the **parent HID
  kernel name** instead: `KERNEL=="hidraw*", KERNELS=="0005:28DE:*"` (bus `0005`=BT,
  VID/PID uppercase). Shipped in `crates/steam-hid/udev/69-steam-hid.rules`.
- **enumerate:** hidapi lists the single BLE hidraw node **once per top-level collection**
  (mouse `0x01`, keyboard `0x01`, vendor `0xFF00` - all same path). The standard
  `usage_page >= 0xFF00` gamepad-interface filter keeps exactly the vendor entry - no
  BLE-special-casing needed. `interface_number` is `-1` (BT has none) and the serial is the
  MAC, so `DeviceId = gordon:bt:-1:<mac>`; `-i bt` is the transport selector token.
- **Keep-alive: not needed.** Unlike Neptune (reverts to lizard ~10 s after lizard-off), BLE
  Gordon stayed alive well past that window during gameplay, so it's excluded from keepalive
  (which remains Neptune-gated - no code change needed).

**Triton (new Steam Controller, 2026) - HW-verified (1.9).** The 1.3 "new Steam Controller",
**not in the C#/kernel refs**. Reverse-engineered from **SDL** (`SDL_hidapi_steam_triton.c` +
`steam/controller_structs.h` - Valve's own struct names; the **primary** source, and the one that
handles newer report ids) and **sc-controller** (`scc/drivers/sc2.py` +
`docs/steam-controller-v2-protocol.md` - the better RE narrative + `usbmon` command captures).

- **Topology / PIDs** (all `0x28DE`): `0x1302` wired (single HID iface 0), `0x1303` Bluetooth LE,
  `0x1304` "puck" wireless dongle (SDL "Proteus"). The puck enumerates **per-slot HID interfaces
  2..=6** on the observed unit (SDL documents 2..5 as the 4 controller slots - "4 devices per puck";
  real HW exposes one more, likely a dongle-management iface) and reports a **real serial** (unlike
  the Gordon dongle). Auto-open treats it as a dongle (poll-probe each slot for a streaming frame -
  it streams by default). SDL's 2nd variant "Nereid" `0x1305` is not added until seen.

- **Framing is NOT the `0x01`-event `ValveInReport_t`.** Triton input/haptic reports carry the
  **report id in byte 0** - dispatched in `report::parse_triton` (kind-aware, routed from
  `Device::next_frame_triton`), never the `parse`/`buf[2]` event path. Report ids: `0x42` state
  (with on-controller quaternion), `0x45` state "NoQuat", `0x47` state "Ibex" (16-bit timestamps),
  `0x43` battery, `0x46`/`0x79` wireless status (`TritonWirelessStatus{state}`: 2=connect,
  1=disconnect). **HW: puck + wired stream `0x42`, Bluetooth streams `0x45`** - the same NoQuat body
  (the `0x42` quaternion is just trailing bytes we skip; orientation is unused downstream). BT
  dropping the quaternion mirrors Gordon BLE limiting orientation over the air.
  - **`0x47` deferred.** SDL parses it (`TritonMTUNoQuat32TS_t`, "Ibex" - a trackpad timestamp + a
    16-bit IMU timestamp); sc-controller does not. We **skip** it (`parse_triton` -> `None` -> keep
    reading). **It is NOT simply a newer-firmware default:** on 2026-08-26 Steam force-updated this
    unit's firmware on connect (so, latest) and it **still streams `0x42`** - so whatever selects
    `0x47` is *something else* (a different SKU/variant or mode), currently unknown. If a unit ever
    streams it, add a parser: its extra `u16` trackpad-timestamp shifts the **pad/pressure** fields
    **+2**, but the IMU timestamp shrinks `u32->u16` (-2), so **accel/gyro stay at the same offsets**.

- **Report `0x42`/`0x45` NoQuat body** (offsets into the raw read, byte 0 = report id; little-endian;
  matches SDL `TritonMTUNoQuat_t` and sc-controller's doc):

  | Offset | Type | Field | | Offset | Type | Field |
  |---|---|---|---|---|---|---|
  | 0 | u8 | report id | | 22-23 | u16 | left pad pressure |
  | 1 | u8 | seq_num | | 24-27 | i16x2 | right pad x,y |
  | 2-5 | u32 | buttons | | 28-29 | u16 | right pad pressure |
  | 6-7 | i16 | left trigger (0..~32767) | | 30-33 | u32 | IMU timestamp |
  | 8-9 | i16 | right trigger | | 34-39 | i16x3 | accel x,y,z |
  | 10-13 | i16x2 | left stick x,y | | 40-45 | i16x3 | gyro x,y,z |
  | 14-17 | i16x2 | right stick x,y | | (46-53) | i16x4 | quaternion w,x,y,z (`0x42` only; skipped) |
  | 18-21 | i16x2 | left pad x,y | | | | |

- **Button bits** - `u32` = `byte2 | byte3<<8 | byte4<<16 | byte5<<24` (SDL `TritonButtons` ==
  sc-controller `SC2Button`; `steam-hid::TritonButtons` uses the unified names):

  | byte | `0x01` | `0x02` | `0x04` | `0x08` | `0x10` | `0x20` | `0x40` | `0x80` |
  |---|---|---|---|---|---|---|---|---|
  | **2** | A | B | X | Y | QuickAccess | R3 | Menu | R4->RGrip |
  | **3** | R5->RGrip2 | RB | Dpad-Down | Dpad-Right | Dpad-Left | Dpad-Up | View | L3 |
  | **4** | Steam | L4->LGrip | L5->LGrip2 | LB | RStickTouch | RPadTouch | RPadClick | RT full-pull |
  | **5** | LStickTouch | LPadTouch | LPadClick | LT full-pull | **RGripTouch** | **LGripTouch** | ? | ? |

  Back paddles follow the Deck convention (upper R4/L4 -> grip, lower R5/L5 -> grip2). **New over the
  Deck: capacitive grip-touch** (byte5 `0x10`/`0x20`) - reads on whenever the handles are held,
  *including resting on a table* (byte5 idles `0x30`; hands fully off -> `0x00`). Added as
  `L/RGripTouch` in `vocab-hid` (bindable + usable as gaters). Two byte5 bits (`0x40`/`0x80`) are
  unidentified on the test units.

- **Commands: feature report `0x01`, the whole HID report EXACTLY 64 bytes** (report id + 63
  payload). Gordon/Neptune use report `0x00` with 64 *data* bytes = a **65-byte** buffer (SDL's
  documented *"firmware quirk: Set/Get Feature always require a 65-byte buffer"*; report 0 is
  unnumbered so nothing extra rides the wire). A **65-byte Triton write stalls**
  (`ioctl SFEATURE: Broken pipe`) and lizard-off / gyro / LED / idle silently fail - but input still
  streams (`0x42`/`0x45` flow even in lizard mode), so the symptom is "everything works but the
  config writes fail." `Device::frame(cmd, report_id, buf_len)` sizes it: Triton `(0x01, 64)`,
  Gordon/Neptune `(0x00, 65)`. The command *body* (`[cmd_id, len, payload]`) and setting indices are
  otherwise identical to Gordon/Neptune, so lizard-off (`CLEAR_DIGITAL_MAPPINGS` + trackpad
  `NONE`), gyro-enable, LED, idle **reuse the existing helpers**. The puck streams by default, so no
  `DONGLE_GET_WIRELESS_STATE (0xB4)` prompt is sent (Gordon-dongle-only).

- **IMU passes through RAW** - like Neptune, no correction (HW-verified 1.9): accel flat->`+Z`,
  right-side->`+X`, nose-up->`+Y`; gyro pitch-up->`+X`, roll-right->`+Y`, yaw-left->`+Z` (sc-controller's
  `-ry` is its own DS4 convention, not ours). Gyro full-scale is 2000 dps (res ~`16.384` LSB/dps) vs
  the canonical `GYRO_RES_PER_DPS = 16` (2048 dps) - the ~2.3 % delta is **accepted un-rescaled**
  (below sensitivity granularity). **IMU is default-on:** removing the `set_gyro(true)` call and
  full-power-cycling the controller (no persisted state) still streams accel/gyro - so no explicit
  enable is needed (verified on dongle/wired). We keep the reader's `set_gyro(true)` anyway (harmless
  re-assert, and load-bearing for Gordon/Neptune). Over BT the body is `0x45` (NoQuat) so the
  quaternion is absent there; unused downstream regardless.

- **Haptics ride OUTPUT reports `0x80`-`0x85`** (via hidapi `write` -> interrupt-OUT endpoint, exactly
  as SDL's `SDL_hid_write`; sc-controller's "stall" caveat is only about the *feature/control* path,
  which we don't use for haptics). Wired up: **`0x80` rumble** (`{type, intensity, left{speed,gain},
  right{speed,gain}}`, 10 B) and **`0x82` click** (`{side, style, amplitude}`, 4 B). Also wired +
  HW-explored (the beep/haptic session): **`0x81` pulse** (`pulse_triton` - rumble train + single-pulse
  click), **`0x83` LFO tone** + **`0x84` log sweep** (`lfo_tone_triton`/`logsweep_triton` - the audio
  paths). Recorded but unused: `0x85` script. Full findings in 1.10.
  - **Rumble** levers == the Deck's `0xeb`: `left`/`right` = per-motor **rate** (coarse strength),
    `*_gain` (dB) the strength trim, `intensity` = a finer **inverted** lever (`0` = no change). side
    `0`=left/`1`=right (verified). `Device::rumble_triton` param order mirrors `rumble_cmd`. Re-fire
    **400 ms** - the firmware sustains each command well past that (no gap until >500 ms), far laxer
    than the Deck's 500 ms burst; reader `TRITON_REFIRE_MS`.
  - **Click** `style` is a `HapticStyle` (`1` Weak / `2` Strong) and the **only working lever**; the
    amplitude byte (sc-controller `0`=medium...`255`=strong; SDL's struct mis-types it as signed
    `gain_db`, but SDL never sends `0x82`) is **HW-confirmed inert** - and **Steam's own 0-12 dB
    haptic setting changes nothing either**, so it's a firmware/hardware limit, not our packet. Even
    `Weak` is a fairly firm click => only **two** distinct strengths. Reader `triton_click`:
    Low=`(Weak,0)` Med=`(Weak,255)` High=`(Strong,0)` (Med's 255 is only a gradient for a possible
    future firmware). Explore with `haptic --triton`.

- **Transports need no per-transport input code** - everything keys on `DeviceKind::Triton`, not the
  transport (this is why wired and BT each cost **zero** core changes).
  - **Wired** (`0x1302`): identical to the puck; battery pinned **100 %** placeholder (like Gordon
    wired - a USB device that reports no real charge).
  - **Bluetooth** (`0x1303`): **not segmented like Gordon BLE.** The OS HID-over-GATT stack
    reassembles GATT notifications into plain numbered reports and prepends the report id, so BT
    reports arrive exactly like USB (state id `0x45`) and route through `next_frame_triton` - the
    Gordon `BleState` segmenter is `!is_triton()`-gated, so Triton never touches it. (SDL's manual
    report-id reconstruction is Android/iOS-only, where the app does raw GATT; Linux HOG - and
    Windows `hidclass` - do it in the OS.) `interface_number = -1`, serial = MAC
    (`triton:bt:-1:<mac>`); the udev rule `KERNELS=="0005:28DE:*"` already covers it.
  - **Keep-alive:** Triton reverts to lizard **~3 s** after lizard-off on **all** transports (a
    firmware watchdog - unlike BLE Gordon, which needs none), so `DeviceKind::needs_keepalive` is
    kind-based (Neptune + Triton) and the reader re-asserts on a single **~3 s** cadence.

### 1.5 Public API design (Rust)

Idiomatic, embeddable, **blocking + sync by default, no forced async runtime**.

**Frame model - the device speaks full snapshots, and the read stream is heterogeneous.**
Every input frame is the **complete current state** (buttons as a current-*held* bitfield,
sticks/pads/triggers as absolute values, IMU) pushed ~250 Hz-1 kHz; the controller never
sends deltas. But the *same* read stream also carries non-input frames - connect/disconnect
(`0x03`) and battery (`0x04`, wireless only). So **one physical read yields exactly one
frame of *some* type**, and that is modelled as an enum, never a bare `ControllerState`.
(A bare-state return can't represent a disconnect frame and would then block forever once
the controller powers off but the dongle stays connected - see disconnect semantics below.)

**Two layers.** Layer 1 = **`RawReport`**, the decoded wire frame (per-device input fields
in wire types, *plus* the lifecycle frames). Layer 2 = **`Report`**, wrapping a unified,
normalized **`ControllerState`** (produced by a pure `From` conversion), or the same
lifecycle signals. The mapper consumes `Report::State(ControllerState)`; the `dump` example
can read raw. Illustrative sketch (subject to refinement during implementation):

```rust
// ---- discovery / lifecycle ----
pub struct Manager { /* owns hid backend context */ }
impl Manager {
    pub fn new() -> Result<Self>;
    pub fn enumerate(&self) -> Result<Vec<DeviceInfo>>;   // gamepad interfaces only, see 1.6
    pub fn open(&self, info: &DeviceInfo) -> Result<Device>;
    pub fn open_first(&self) -> Result<Device>;           // convenience
}

pub struct DeviceInfo {
    pub kind: DeviceKind,        // Gordon | Neptune | (future) ...  (codenames, unified w/ RawReport)
    pub transport: Transport,    // UsbWired | UsbDongle | Bluetooth (Gordon BLE, 1.4)
    pub serial: Option<String>,
    pub vid: u16, pub pid: u16,
    pub interface: i32,          // HID interface No. - identifies the dongle slot / wired gamepad iface
    // + opaque OS path used to open
}

// ---- a Device is a transport ENDPOINT (wired gamepad iface / dongle slot), NOT a live
//      controller: it outlives connect/disconnect and stays usable across reconnects ----
pub struct Device { /* owns its hid handle; is Send; no keep-alive thread - the owning consumer drives it (see 1.6) */ }
impl Device {
    pub fn info(&self) -> &DeviceInfo;
    pub fn is_connected(&self) -> bool;        // cached from 0x03 frames; convenience, not load-bearing
    pub fn battery(&self) -> Option<Battery>;  // cached last-known (wireless only); see below

    // ---- input: one physical read == one frame of some type ----
    // low-level: the decoded wire frame (per-device input fields, or a lifecycle frame)
    pub fn read_raw(&mut self) -> Result<RawReport>;                    // blocks until next frame
    pub fn poll_raw(&mut self, timeout: Duration) -> Result<Option<RawReport>>;   // None = timed out

    // high-level: unified normalized snapshot, or the same lifecycle signal
    pub fn read(&mut self) -> Result<Report>;                          // blocks until next frame
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Report>>;          // None = timed out

    // event view: the SAME stream expressed as deltas (+ lifecycle passthrough).
    // Holds an internal previous-state; derived from read() + ControllerState::diff() (see below).
    pub fn events(&mut self) -> Events<'_>;

    // ---- commands ----
    pub fn set_lizard_mode(&mut self, on: bool) -> Result<()>;
    pub fn set_imu_mode(&mut self, mode: ImuMode) -> Result<()>;       // bitflags (1.4 IMU_MODE)
    pub fn set_gyro(&mut self, on: bool) -> Result<()>;                // convenience = raw accel|gyro
    pub fn set_led_intensity(&mut self, percent: u8) -> Result<()>;    // 0..=100
    pub fn set_idle_timeout(&mut self, secs: u16) -> Result<()>;
    pub fn rumble(&mut self, motor: Motor, params: Rumble) -> Result<()>;  // wraps the verified packet
    pub fn power_off(&mut self) -> Result<()>;

    // escape hatch: takes the LOGICAL command `[cmd_id, len, payload...]`; the report-ID-0
    // prepend + pad-to->=64 framing (1.4) is applied internally, not by the caller.
    pub fn send_feature_report(&mut self, cmd: &[u8]) -> Result<()>;
    pub fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize>;
}

// ---- LAYER 1: raw decoded wire frame (input variants + lifecycle variants) ----
pub enum RawReport {
    Gordon(GordonReport),      // 0x01 INPUT_DATA
    Neptune(NeptuneReport),    // 0x09 DECK_INPUT_DATA
    Connected,                 // 0x03, payload byte 0x02
    Disconnected,              // 0x03, payload byte 0x01
    Battery(BatteryRaw),       // 0x04 CONTROLLER_STATUS (wireless only)
}

pub struct GordonReport {              // original Steam Controller - input fields only
    pub seq: u32,
    pub buttons: GordonButtons,        // bitflags of Gordon's buttons only
    pub left_trigger: u8,  pub right_trigger: u8,    // wire: u8 0..=255
    pub left_stick: Vec2i, pub left_pad: Vec2i,      // multiplex resolved via touch flags
    pub right_pad: Vec2i,                            // (no right stick on Gordon)
    pub accel: Vec3i, pub gyro: Vec3i, pub orientation: Quati,   // raw i16
    // NOTE: battery is NOT here - it arrives out-of-band as the 0x04 frame (RawReport::Battery),
    // wireless only. The inline GCInput field @0x3E is an unverified alternative source (1.9).
}

pub struct NeptuneReport {             // Steam Deck - input fields only
    pub seq: u32,
    pub buttons: NeptuneButtons,       // Deck buttons only (+L5/R5, QuickAccess, stick touch...)
    pub left_trigger: i16, pub right_trigger: i16,   // wire: i16
    pub left_stick: Vec2i, pub right_stick: Vec2i,
    pub left_pad: Vec2i,   pub right_pad: Vec2i,
    pub left_pad_pressure: i16, pub right_pad_pressure: i16,
    pub accel: Vec3i, pub gyro: Vec3i, pub orientation: Quati,   // raw i16
    // (Neptune-as-controller has no battery; the Deck's device battery is a separate concern)
}

// ---- LAYER 2: unified snapshot, or the same lifecycle signal ----
pub enum Report {
    State(ControllerState),    // an input frame, converted
    Connected,
    Disconnected,
    Battery(Battery),
}
impl From<&RawReport> for Report { /* input variants -> State(convert); lifecycle -> passthrough */ }

pub struct ControllerState {
    pub seq: u32,
    pub timestamp: Timestamp,         // monotonic, captured at read; recorded in traces &
                                      // drives the engine's injected clock on replay (1.8, 4)
    pub buttons: Buttons,             // unified superset bitflags; absent buttons never set
    pub left_trigger: f32,  pub right_trigger: f32,  // normalized 0.0..=1.0
    pub left_stick: Vec2,   pub right_stick: Vec2,    // normalized -1.0..=1.0 (zeroed if absent)
    pub left_pad: TrackPad, pub right_pad: TrackPad,  // pos -1..1, pressure 0..1, touched
    pub accel: Vec3i, pub gyro: Vec3i, pub orientation: Quati,  // RAW i16 + documented scale
    // NOTE: battery is NOT a field here - it is device-level state (is_connected()/battery()),
    // surfaced as Report::Battery / Event::Battery, not carried in every input snapshot.
}
impl ControllerState {
    // STATELESS diff primitive: caller holds both snapshots (e.g. the engine keeps prev).
    pub fn diff<'a>(&'a self, prev: &'a Self) -> impl Iterator<Item = Event> + 'a;
}

pub enum Event {
    ButtonPressed(Button), ButtonReleased(Button),
    AxisChanged(Axis, f32),          // normalized axes
    Connected, Disconnected, Battery(Battery),
}

// small value types
pub struct Vec2  { pub x: f32, pub y: f32 }                 // normalized
pub struct Vec2i { pub x: i16, pub y: i16 }                 // raw wire
pub struct Vec3i { pub x: i16, pub y: i16, pub z: i16 }
pub struct Quati { pub x: i16, pub y: i16, pub z: i16, pub w: i16 }
pub struct TrackPad { pub pos: Vec2, pub pressure: f32, pub touched: bool }
```

**Snapshots vs. events - how the four input methods relate.** They are not four parallel
things; they are two layers x two views over the *one* frame stream:
- **`read_raw`/`read`/`poll_*` = snapshots.** The native truth: each read returns one full
  frame. `read_raw` gives wire fields; `read` gives the normalized `ControllerState` (or a
  lifecycle `Report`). **The engine uses these** - mapping needs the whole current state
  each tick (a *held* button is a state, not a one-shot event), not deltas.
- **`ControllerState::diff(prev, cur)` = stateless diff primitive.** Give it two snapshots,
  get the changes. No hidden state; the caller owns `prev`.
- **`events()` = the stateful streaming view**, built from the two above: it keeps an
  internal `prev`, and per frame yields *changes* for input frames + *passthrough* for
  lifecycle frames. Roughly:
  ```
  loop { match read()? {
      Report::State(cur)   => { yield cur.diff(&prev); prev = cur; }  // diffed
      Report::Disconnected => yield Event::Disconnected,              // passthrough
      Report::Connected    => yield Event::Connected,
      Report::Battery(b)   => yield Event::Battery(b),
  } }
  ```
  Use it for change-driven consumers (the `dump` example; a UI showing "you pressed A").
- **One mode per Device.** `read`/`poll` and `events()` both *consume* the single frame
  stream, so a given `Device` is driven in snapshot mode **or** event mode, not both at
  once.
- **Event semantics (threshold + scope).** The device **streams full snapshots
  continuously at a fixed cadence** (free-running per-frame `seq`; it does *not* send
  on-change), so consecutive frames usually differ only by analog/IMU **noise**. So
  `diff()` applies a small **per-axis deadband** before emitting `AxisChanged`, and
  `events()` covers **buttons + normalized axes + lifecycle only** - **IMU is snapshot-only**
  (raw, always varying; meaningless as discrete "changed" events). Without the deadband a
  change-log would spew at the full stream rate on jitter alone.
  - **The deadband is analog-only, cosmetic, and off the mapping path.** Digital buttons
    are exact bit flips (no threshold). And **the engine does not use `events()`/`diff()`**
    at all - it consumes raw snapshots and applies its *own configurable* deadzones/curves
    (3/4). Where the engine needs input edges (activators: long-press, double-tap,
    on-release, turbo) it computes exact button transitions from its **own** retained `prev`
    (`cur.buttons & !prev.buttons`), not from this path. So this deadband **can never reach
    mapping.** `diff()`/`events()` exist for change-log consumers (`dump`; future UI
    binding-capture); `tui` just renders the current snapshot and uses neither.

**Disconnect semantics - two distinct flavors, neither of which hangs `read`.**
- **Transport gone** (wired Gordon unplugged, or the dongle removed): the USB device
  disappears, the HID handle goes invalid, the underlying read fails -> `Err(Error::...)`.
  The `Device` (interface) is dead. This *is* an error.
- **Controller gone, transport alive** (dongle stays; pad powers off / out of range): a
  `0x03` frame arrives, then the dongle goes quiet. This is **not** an error - it is
  returned as `Ok(RawReport::Disconnected)` / `Ok(Report::Disconnected)` and surfaced as
  `Event::Disconnected`. Because disconnect is a *returned value*, `read` never blocks on a
  vanished controller: a frame woke it. A subsequent blocking `read` then legitimately
  waits for the next frame (a reconnect, or a transport error). `is_connected()`/`battery()`
  are just cached conveniences updated from these frames - not required for correctness.

**Conversion rules (`RawReport` -> `ControllerState`), hybrid normalization:**
- **Normalized to `f32`:** triggers -> `0.0..=1.0` (Gordon `u8/255`, Neptune
  `i16/32767`); sticks & pads -> `-1.0..=1.0` (`i16/32768`). The u8-vs-i16 trigger
  divergence disappears here - the unified struct is device-agnostic. **Divisors are
  provisional** - the 1.4 resolution constants hint some axes may not span full i16
  range; verify actual full-scale per axis/device on hardware (1.9).
- **Kept raw `i16`:** accel, gyro, quaternion - normalizing to `-1..1` would destroy
  their physical scale, so they pass through raw with documented scale factors. **One
  sign fix:** Gordon's raw gyro `y` (roll) channel is mounted inverted, so the convert
  negates it to land in the unified right-handed IMU frame (`X=right, Y=forward, Z=up`;
  `x`=pitch, `y`=roll, `z`=yaw rate) - HW-verified, 1.9. Accel passes through unchanged
  (already right-handed). Neptune's IMU sign/axis map is a separate, still-unverified
  question (different sensor layout).
- **Buttons:** per-device `GordonButtons`/`NeptuneButtons` fold into the unified
  `Buttons` superset; buttons a device lacks are simply never set.
- **Absent inputs zeroed:** e.g. Gordon has no right stick -> `right_stick = (0, 0)`.

**Error type.** A `steam_hid::Error` (`thiserror`) is part of the public surface: at least
HID/IO failure, transport-gone disconnect, unsupported device, and a framing/short-read
variant. Timeout is *not* an error - it is `Ok(None)` from `poll_*`. Controller disconnect
is *not* an error - it is a `Report::Disconnected` value.

**Traits, derives & `serde`.** Public value / state / event types derive
`Debug + Clone + PartialEq` - `Clone` (never `Copy`; clone explicitly where a copy is
wanted) and `PartialEq`/`Debug` for golden-test assertions and for holding a `prev`. An
optional **`serde` feature** derives `Serialize`/`Deserialize` on `ControllerState`,
`Report`, `Buttons`, and the value types - needed by the `dump` trace format (1.8) and the
6 network path; **off by default** so non-serializing consumers don't pay for it.

**Button / axis taxonomy.** `Button` (enum, one variant per bit) and `Buttons` (bitflags)
are two views of the same unified superset (listed in 1.4); `Axis` enumerates the
normalized analog channels - both triggers, both sticks x/y, both pads x/y + pressure. Both
are fully enumerable now (no hardware needed).

### 1.6 Backend & platform notes

- **HID backend:** `hidapi` (input via `read_timeout`, commands via
  `send_feature_report`/`get_feature_report`). We target **USB only for now** (wired +
  wireless dongle); Bluetooth is deferred (see 1.9), but `hidapi` keeps the door open
  with no API change. The backend is a concrete internal wrapper (`HidapiDevice`), **not**
  exposed in the public API; a raw `hidraw`/`nusb` Linux backend, if ever needed, would be a
  compile-time swap - no runtime abstraction (trait/`dyn`) is carried speculatively for it.
- **Interface topology & `enumerate` filtering (important).** Each Steam device exposes
  **several HID interfaces**, only one (or a few) of which is the real gamepad; the others
  are the lizard-mode emulated mouse/keyboard. From `hid-steam.c`:
  - **Wired Gordon (`0x1102`): 3 interfaces** - `0` emulated mouse, `1` emulated keyboard,
    **`2` the real gamepad**.
  - **Wireless dongle (`0x1142`): 5 interfaces** - `0` emulated keyboard, **`1-4` the up-to-4
    gamepad slots**.

  So `enumerate()` **must filter to the gamepad interface(s)** and drop the emulated ones,
  otherwise `open_first()` grabs the mouse interface and reads nothing useful. The kernel's
  discriminator is "the interface that has a feature report"; via `hidapi` the practical
  filter is the Valve **vendor usage page (`0xFF00`-range)** / interface number, both
  exposed on `hidapi`'s device info. `DeviceInfo.interface` carries the number.
- **Dongle slots are separate interfaces, not a multiplexed stream.** Each wireless slot
  (`1-4`) is its own HID interface -> its own `hidapi` path -> its own `Device`, with its own
  connect/disconnect frames and input stream. "Which slot" == "which interface you opened";
  there is **no phantom-slot bookkeeping** - an idle slot simply never emits a connect.
  (Multi-slot use is still deferred per 1.9; this just records how it actually works.)
- **Linux `hid-steam`:** the in-kernel driver claims the device. Getting raw access may
  require a udev rule and/or unbinding the kernel driver. **Spike this first.** Ship an
  example `udev/69-steam-hid.rules` granting `uaccess` for the VID/PIDs above.
- **`Device` ownership & `Send`.** A `Device` is **self-contained** - it owns (an `Arc` to)
  the HID context rather than borrowing `Manager`, and is **`Send`** so the 4 manager can
  move one onto its own worker thread (thread-per-device). It is not `Sync` (single-owner
  per thread), which is all the thread-per-device model needs.
- **Device lifecycle - `open` and `Drop`.**
  - **On `open()` of a wireless Device, request connection status** (`ID_DONGLE_GET_WIRELESS_STATE`
    `0xB4`) - the kernel does exactly this (*"useful if this driver is loaded when the
    controller is already connected"*). Without it, a controller that was **already
    connected** before we opened sends no fresh `0x03` connect frame, so `is_connected()`
    would sit false and we'd wait forever for a connect that already happened. Opening
    prompts the current state.
  - **On `Drop`, restore lizard mode (best-effort).** Otherwise, when the mapper exits, the
    controller is left with mappings cleared -> dead (does nothing) until replugged. Default is
    **restore-on-exit**, not leave-raw. (No keep-alive thread to join - see 1.6.)
- **Reads are internally timeout-based (cooperative shutdown).** Even the "blocking"
  `read()` loops over short-timeout reads checking a shutdown flag - a thread parked in a
  truly infinite `hidapi` read can't be cancelled, so `Drop` / the 4 manager tearing down
  a worker would hang. This holds regardless of which keep-alive sync option (below) wins.
- **Concurrency - the keep-alive vs. reader question (RESOLVED: option C, at the engine layer).**
  Blocking reads run on the owning thread. The Neptune lizard-off keep-alive (1.4) must send a
  feature report ~1 Hz *while* a read is in flight, and both touch the same handle - this was the
  one real synchronization question, and it is **Deck-only**: Gordon has no keep-alive, so it is
  **lock-free and thread-free** (direct blocking/timeout reads + direct writes). The three options
  weighed were: **A.** single handle + `Mutex` + timeout reads; **B.** a dedicated write fd (needs a
  Windows-exclusive-open spike); **C.** a single IO thread owning the handle, interleaving keep-alive
  with reads/writes. **Outcome:** C won, but realized **one layer up** - steam-hid stays thread-free
  and stays the single-writer `Device`; the **engine reader thread** (4) is the IO thread that owns
  the handle and interleaves the ~2 s keep-alive with its poll/rumble/click writes. So steam-hid
  needs no lock, no second fd, and no internal thread; the keep-alive cadence and its Neptune gate
  live in the engine (`ReaderCfg::keepalive`), documented on `set_lizard_mode`.

### 1.7 Dependencies (all well-maintained, widely-used)

`hidapi` (HID access), `bitflags` (button set), `thiserror` (error types). Optional:
`serde` behind an off-by-default **`serde` feature** (for trace + network serialization,
1.5). Examples only (`[dev-dependencies]`): `ratatui` + `crossterm` for the TUI.

### 1.8 Test binaries

Two, both living **inside** the `steam-hid` crate initially as `examples/` (their deps
come from `[dev-dependencies]` and do **not** leak into the library's public dependency
graph - unlike `src/bin/`). Run via `cargo run --example dump` / `--example tui`. Split
the TUI into its own workspace crate later only if its dependency footprint grows.

1. **`dump`** - prints controller state changes to the terminal as they happen; can
   also send a few commands (toggle lizard/gyro, rumble, power off) for manual testing,
   **including firing each candidate haptic packet** (`0x8f`/`0xea`/`0xeb`) so we can
   compare them during verification (1.9). Optionally **records input traces** to a file,
   reused as golden fixtures for `engine` tests (4). Traces **must record each frame's
   arrival timestamp** (the `ControllerState.timestamp`, stored as elapsed-from-start so
   it's serializable) - `seq` alone gives ordering, not the wall-clock spacing that
   timing-based activators (long-press, double-tap) need to replay faithfully.
2. **`tui`** - a live `ratatui` dashboard of the full controller state (buttons, pads,
   sticks, triggers, gyro), same command controls.

### 1.9 Scope, development order & open questions

**Device scope & extensibility.**
- **New Steam Controller (post-2023):** out of scope - no hardware, no reference, will
  not implement. But keep the design *ready* for it: `DeviceKind`, `RawReport`, and the
  button/axis mapping must be extensible (add a variant + a reader) without reworking
  the public API. Mark the extensible public enums `#[non_exhaustive]` - `DeviceKind`,
  `RawReport`, `Report`, `Event`, `Button`, `Axis`, and `Error`.
- **`0x12f0` ("SteamOS handheld"):** identity unconfirmed - ignore for now.

**Transport scope & development order.**
- Implementation is written for all supported devices in parallel, but **development &
  verification proceed in order**: primary dev target is **Gordon + wireless dongle**
  (`0x1142`). Once that works, connect and test **wired Gordon** (`0x1102`), then
  **Steam Deck** (`0x1205`).
- **Bluetooth: deferred.** The dev device has no BT firmware and it isn't needed now
  (maybe later). USB only for the foreseeable work.

**steam-hid API scope (decided).**
- **Multiple devices:** the `Manager` API is designed for multiple controllers from the
  start (enumerate -> list); we only *test* single-device for now. Multiple wireless slots
  per dongle (up to 4) are physically separate HID interfaces (1.6), each its own
  `Device` - so multi-controller *support* is nearly free; only its *testing/UX* is
  deferred. `ID_DONGLE_GET_CONNECTED_SLOTS` (querying slot occupancy) is deferred.
- **Hotplug:** handle the controller's own `CONNECT`/`DISCONNECT` frames (the dongle
  sends them; 1.5 disconnect semantics) + reconnect on the same `Device`/interface.
  **OS-level USB hotplug monitoring** (a whole *interface* appearing/vanishing, e.g. dongle
  plugged in) **is deferred** - decide when doing the wired setup (fiddly cross-platform).

**Haptics - Gordon RESOLVED; Deck HW-explored (this session).**
- **Gordon (`0x8f` `TRIGGER_HAPTIC_PULSE`): VERIFIED** via the `haptic` example (escape
  hatch -> raw packets). The original SC has **only** this one haptic path - it drives the
  two **trackpad actuators** (no rumble motors). Findings: **wire pad 0 = RIGHT, 1 = LEFT**
  (the kernel's "legacy swap" `pad ^= 1`; our `Motor::Right->0/Left->1` matches). Params are
  `duration`/`interval` (us on/off -> tone) and `count` (pulses -> length). The kernel's
  8-byte variant adds a trailing **`gain`** byte (dB, -24..+6) - **ignored on Gordon** (no
  audible difference across the range), so the C# 7-byte and kernel 8-byte forms are
  functionally identical here. `Device::haptic_pulse()` (renamed from `rumble()` this session) sends
  the kernel 8-byte form (gain honored on Deck, inert on Gordon). **`pad=2` (BOTH) is NOT a valid
  wire value on Gordon - it no-ops; "both pads" must be two pulses (wire 0 + wire 1).**
- **Gordon haptic parameter map (verified via `haptic` sweep).** The pulse is a square wave:
  **freq ~ `1e6/(duration+interval)` Hz**, on/off ratio = duty cycle, `count`xperiod = length.
  - **Frequency = rumble<->tone:** <= ~130 Hz feels like **rumble**; ~350 Hz is transitional
    (more sound than rumble); 600-1000 Hz is an audible **tone** (still faintly felt). So
    game rumble -> Gordon is a low-frequency drive (~30-130 Hz).
  - **Duty cycle = crude amplitude** (since `gain` is inert): more on-time = stronger, but
    **non-linear and saturating** - a clear jump 10%->50%, much less 50%->90%.
  - **Effective strength - why per-profile `strength` can exceed 100 % (engine mapping, HW-found).**
    Two compounding attenuations make raw game rumble feel weak on Gordon: **(1)** the pad
    **saturates at ~25 % duty**, so the engine maps full drive onto only `RUMBLE_MAX_DUTY = 0.25`
    of the period (the honest ceiling - above it feels identical); and **(2) games under-drive
    their FF** - HW logging caught the test game peaking at only ~25 % of the `u16` FF range (raw
    ~16381/65535), so its "full" rumble reached ~ 0.25 x 0.25 ~ **6 % duty**. So per-profile
    **`strength` is a `u8` gain that may exceed 100 %** (up to 255): ~**200 %** multiplies such a
    game's drive back up so it can **saturate up to ~0.50 of the range** (~ 12 % duty - bumping its
    weak low forces), while the drive still **clamps at `u16::MAX`** so it can only reach, never
    overshoot, the 0.25-duty ceiling. Attenuation is now per-device (Gordon's `duty` lever, replacing
    the dropped global `master_rumble`). This lives in the **engine** mapping (`runtime/reader.rs`
    `RUMBLE_MAX_DUTY` + `mapping.rs` scaling), not steam-hid - the *why*, since it's easy to misread as
    a bug.
  - **Future tuning (deferred):** the Xbox360-strength -> Gordon-drive mapping should be
    **non-linear**, and **left/right actuators differ in hardware characteristics** (one may
    be more linear), so calibrate the two pads separately. Fine-tuning for the engine/mapping
    stage, not steam-hid.
- **`0xeb` `TRIGGER_RUMBLE_CMD`: does nothing on Gordon** (confirmed) - the kernel gates
  `FF_RUMBLE` on `STEAM_QUIRK_DECK`, and the original SC has no rumble motors. Deck-only.
- **Deck haptics - HW-EXPLORED (this session; `0xeb` wired into the engine, tuning ongoing).**
  Three packets, characterised on a real Deck via the `haptic` example:
  - **`0x8f` (trackpad pulse) works well on BOTH controllers.** On the Deck its **`gain` byte IS
    honored** (ignored on Gordon), so there amplitude = gain (Gordon amplitude = duty). Feel:
    **150 Hz** is nice across most gains (**right pad weaker at low gains**, comparable higher);
    **60-90 Hz feel weaker at low gains** (need more gain to register). Useful gain ~ -16...+6 dB
    (~0-2 dB already strong, +6 shakes the whole unit).
  - **The re-fired short train feels slightly jerky on BOTH controllers** - each re-fire
    (`RUMBLE_REFIRE_MS` = 220 ms) restarts the actuator -> a ~4.5 Hz amplitude beat (previously
    mistaken for Gordon's character; it's the re-fire).
  - **"Longrumble" = ONE very long train (huge `count`, no re-fire) is smooth on both** (rings up
    once, then holds). **Stopped with `count = 0`** (HW-confirmed on Gordon AND Deck; `count = 1`
    also works - a single imperceptible tick that ends). A viable **future alternative** to the
    re-fire model; if adopted, **`RUMBLE_MAX_DUTY` needs re-tuning** (a full-ring-up train is much
    stronger, so the 0.25 / 200 % values - which partly compensated for the under-ringing short
    train - would drop), **possibly per-controller/`Shape`**. A first engine cut was reverted
    (multi-rumble stop/interleave bugs) and parked on a branch.
  - **`0xeb` `TRIGGER_RUMBLE_CMD` = the Deck's real dual motors** (`Device::rumble_cmd`; the
    kernel `FF_RUMBLE` path; Deck-only, no-ops on Gordon). Character: **inherently pulsating**
    (matches Steam's own in-game feel). Field semantics (HW-found): **`left`/`right` "speed" = pulse
    RATE, not amplitude** - higher value = faster pulsing (a *pulse* frequency, NOT a rumble
    frequency); **`gain` = amplitude/strength** (clean, monotonic, felt symmetrically on both motors
    -> we drive both at **+2 dB**, dropping the kernel's asymmetric +2/0). Each command is a **fixed
    ~0.5 s burst with no length field** (measured consistent from ~10 % to ~100 % strength over
    several seconds), so a sustained rumble must be **re-issued** - the engine re-fires every
    `NEPTUNE_REFIRE_MS` (500 ms, HW-tunable) while non-zero + on change; `(0,0)` stops it. The
    leading **`intensity` u16 is a SECOND amplitude lever** (HW-found) - a **finer** control than the
    coarse dB `gain` (`gain`'s -16...+6 dB ~ ~20 steps; `intensity` is 16-bit), changing **only**
    amplitude (not pulse rate / length / character). It is **inverted**: **`0` = strongest**, larger
    = weaker (gets weaker ~256, usable to ~16k, ~unfelt near `u16::MAX` - a faint tingle). Now a
    param on `Device::rumble_cmd` but **passed 0 (strongest) everywhere** - plumbed, not yet a
    mapping lever (a future controller-strength -> rumble map could use it for fine resolution).
    `haptic --eint` sweeps it.
  - **`0xea` `SET_HAPTIC2` = a short, finely-tuned trackpad "click"** (`Device::haptic_cmd`,
    `HapticStyle` Disabled/Weak/Strong + `gain` i8; Deck-only, HW-tested via `haptic --ea`).
    Earlier called "known wrong", but the C# *app* uses it exclusively on the Deck and it works
    great: **`style` x `gain` (dB, C#'s -7..5 => -2..+10 dB) give a wide range of click strengths**
    (Weak < Strong at the same gain; `Disabled` = off), and the **strongest beats a full `0x8f`
    command click** - so on the Deck this **IS** the command-haptic click now (see below). **HW-found:
    its two motors are the REVERSE of the `0x8f` wire pads**, so `haptic_cmd` maps `Motor::Left -> 0`,
    `Motor::Right -> 1`. The `gain` param is C#'s `NCHapticPacket2.intensity` field, renamed for
    consistency with the other haptic gains; the packet's other bytes (C#'s `unsure2`/`unsure3`, two
    timestamp words) stay **unverified/unused** - filled with C#'s fixed values + a current-ms tick,
    not exposed.
  - **Command-haptic clicks map per-device from a strength level.** The mapping loop sends the
    `Low/Med/High` level to the reader (not a resolved duration - dropped the master-rumble scaling of
    clicks; a discrete tick, and game rumble still scales), and the reader resolves it: **Gordon** =
    `0x8f` pulse whose **duration** encodes strength (500/1000/2000 us; gain inert -> 0); **Deck** =
    the `0xea` `haptic_cmd` (`Strong` style) whose **gain** encodes strength, **per side** (the two
    motors differ - left -4/0/4, right -2/2/6 dB). Replaces the old flat per-controller gain (0/+6).

**Verification status (Gordon).** The full Gordon **input** parse is **hardware-verified**
on **both the wireless dongle (`0x1142`) and wired (`0x1102`)**: buttons (all, correct bit
map), triggers (`u8/255` reaches 0..1), pads & **left stick** (`i16/32768` reaches +/-1,
correct directions), the **left multiplex** (the pad and analog stick share `lpad_x/y`
`0x10`, disambiguated by `LPAD_TOUCH` - pad when touched, stick when not - the same on wired
and wireless; `0x36` is *not* the stick (0 on wireless, ~500 noise on wired); the left
**click** and **touch** buttons are also multiplexed and are de-muxed against *engaged* =
`LPAD_TOUCH || LPAD_AND_JOY` - see the dedicated multiplex bullet below),
and **IMU** (accel reads 1g gravity at rest -> `ACCEL_RES_PER_G=16384` confirmed; gyro rests
~0 and scales sensibly -> `GYRO_RES_PER_DPS=16` confirmed). Enumerate + interface filtering
(wired iface 2; dongle slots 1-4, both drop the emulated ifaces) and the connect/disconnect/
battery lifecycle frames are confirmed. Wired sends **no battery** (`0x04`) frame - expected
(USB-powered; the kernel registers battery only for wireless).
- **IMU axis/sign conventions: RESOLVED (Gordon).** Verified by the 6-pose gravity method
  (accel) + single-axis rotations (gyro). The device frame is **right-handed `X=right,
  Y=forward (toward the nose), Z=up (out of the face)`**. Accel reads `+1g` on the up-axis
  and passes through raw (already right-handed: face-up `+Z`, nose-up `+Y`, right-side-up
  `+X`). Gyro channels are **axis-aligned** with accel - raw `x`=pitch (about X), `y`=roll
  (about Y), `z`=yaw (about Z) - **not** the C# `pitch/yaw/roll` field order (C# transposed
  yaw/roll; offsets are right, names lie). Right-hand-rule signs: pitch-up `+x`, yaw-left
  `+z` are already correct, but roll-right reads **`-y`** (the raw gyro `y` is inverted), so
  the convert **negates `y`** to make `(wx, wy, wz)` a proper right-handed vector. **Neptune IMU
  axis/sign: RESOLVED (2026-08-06).** Compared the `imu` example's converted accel + gyro on a real
  Deck against Gordon - all 3 accel axes/signs and all 3 gyro axes/signs match. So Neptune's raw IMU
  already sits in the unified right-handed frame Gordon reaches *after* its `y`-negation: `from_neptune`
  passes IMU through **raw** with **no** `gordon_gyro`-style correction, confirmed correct (Neptune's
  gyro `y` is mounted the proper way round, unlike Gordon's).

**Other open questions (need hardware verification).**
- **Battery source: RESOLVED.** The out-of-band `0x04` frame is authoritative on the
  wireless dongle - **voltage at offset `0x0C`, charge % at `0x0E`** (kernel offsets;
  verified: `2743 mV, 92%`). The inline `GCInput @0x3E` field the C# reads is **vestigial**
  (always `0` on the dongle), so it is not decoded. (Note: offset `0x04` of the `0x04` frame
  is the seq/timestamp, not voltage - an early mis-decode read it and saw a climbing counter.)
- **Normalization divisors:** `i16/32768` (sticks/pads) and `u8/255` (Gordon triggers)
  **confirmed** on Gordon (full +/-1 / 0..1 reached). **Neptune triggers `i16/32767` and pad
  pressure `i16/32767` also confirmed** on a real Deck (`norm_i16`, full 0..1 reached).
- **Gordon wired vs wireless: RESOLVED.** They share the **same input layout**, including
  the left pad/stick multiplex on `0x10` (the earlier guess that wired had a separate `0x36`
  stick was wrong - `0x36` is noise on wired). `parse` is **not** transport-aware. Only
  difference confirmed: wired sends no `0x04` battery frame. (The C# reference carries a
  `// TODO: this logic is for wireless, wired should report those directly` next to the
  multiplex - an unverified guess that our hardware test disproved; the multiplex is not a
  transport quirk.)
- **Multiplex is Gordon-specific - Neptune does NOT multiplex (from C# ref).** Neptune
  (`NeptuneControllerInputState`) has **dedicated separate axis fields** - `lthumb_x/y`
  (left stick) *and* `lpad_x/y` (left pad), plus `rthumb_x/y`/`rpad_x/y` and pad pressure -
  all populated unconditionally, no touch-gating. The touch-gated left pad/stick share on
  `0x10` is unique to Gordon (which has only a left stick + two pads; no right stick). The
  Neptune parse (`parse_neptune`/`from_neptune`) **has since landed and is HW-verified on a real
  Deck** (wired `0x1205`): it reads the separate stick/pad fields directly (no `LPAD_TOUCH`
  multiplex), plus pad pressure and the firmware-synthesized L2/R2 full-pull bits.
- **Dpad note (not a bug):** the left pad reports **dpad-quadrant bits in every raw report**
  (firmware-derived from click position), regardless of lizard mode, *alongside* the raw pad
  x/y. The parser exposes both; choosing dpad vs. analog is the mapper's job (4). **Confirmed
  identical over Bluetooth** (dpad bits fire from left-pad directional clicks alongside the raw
  pad coords).
- **Left click/touch multiplex: RESOLVED (HW-verified, dongle - via a raw-bit probe).** The left
  pad and stick share not only `lpad_x/y` but a **click bit**. Probed raw: a **stick click** raises
  `LPAD_PRESS`(bit17)+`LSTICK_PRESS`(bit22) with no touch; a **pad click** raises
  `LPAD_PRESS`+`LPAD_TOUCH`(bit19); **both together** hold `LPAD_AND_JOY`(bit23) set and *flicker*
  `LPAD_TOUCH` frame-to-frame (the per-frame axis tag - sc-controller's `STICKTILT`). So
  **`LPAD_AND_JOY` is NOT unused** (earlier assumption corrected) - it's the both-engaged flag.
  `parse_gordon` de-muxes the **buttons** against *engaged* = `LPAD_TOUCH || LPAD_AND_JOY`:
  `left_pad_touch = engaged`, `left_pad_click = LPAD_PRESS && engaged`, `left_stick_click =
  LSTICK_PRESS` (ungated) - matching the kernel's `BTN_THUMB = lpad_touched || lpad_and_joy`.
  **Kernel and C# disagreed** (kernel reads only bit22 as the left click and ignores bit17; C#
  gates bit17 on touch) - the probe settled it in C#'s favour, now HW-verified, not inherited.
  **Buttons are fully clean on both transports; only X/Y stays time-multiplexed** (single coord
  field - not separable per-frame without retention; accepted, and BLE has no multiplex at all).
- **Bluetooth (BLE) Gordon `0x1106`: HW-verified (RESOLVED).** Same commands as USB, segmented
  Report-ID-3 transport + compact delta-input format (see "BLE transport", 1.4). Verified:
  input, buttons, dpad (left-pad clicks), IMU axis/sign, `0x8f` haptics, and the full daemon
  path (`-i bt`) including a quick in-game test. **IMU raw == USB Gordon raw** (same
  `gordon_gyro` y-negation via the Gordon path). **No left multiplex** (separate stick/pad
  chunks). Button bit layout **identical** to USB `GordonButtons`. Orientation quaternion is
  available only with `SEND_ORIENTATION`; we run `0x18` (accel+gyro), leaving it off/unused.
  **Keep-alive not needed** (stayed alive well past Neptune's ~10 s revert during play; BLE is
  excluded from keepalive, which stays Neptune-gated).
- **Gordon USB/BLE path convergence - DONE** (landed with the button naming unification). USB and
  BLE Gordon now share one `GordonButtons` / `GordonReport` / `from_gordon` / `map_gordon_buttons`
  and emit `RawReport::Gordon`; the separate `GordonBle*` types were deleted. Parsers stay separate
  (the wire genuinely differs: 64-byte USB frame vs segmented BLE delta) but the **conversion
  converged** - the USB left multiplex (coords **and** click **and** touch) is absorbed entirely in
  `parse_gordon`, so the shared fold is a plain 1:1 map and BLE (no multiplex) never runs it.
- **Auxiliary output commands (Gordon): mostly RESOLVED** (via the `aux` example).
  `set_led_intensity` **verified** (Steam-button LED: 0=dark, 50=dim, 100=default full;
  reg `LED_USER_BRIGHTNESS=45`). `power_off` **verified** - the controller powers off and
  the dongle delivers a `Disconnected` **value** (not a read `Err`, since the dongle
  transport stays alive; the two-flavor disconnect model, 1.5, confirmed on hardware).
  `set_idle_timeout` (reg `SLEEP_INACTIVITY_TIMEOUT=50`, seconds) writes **byte-identically
  to the verified C# `WriteRegister` path** (`0x87, 0x03, 0x32, lo, hi`) - but physical
  auto-sleep was **not reproducible** at 60/120/300 s under a continuously-reading host (the
  controller streamed a `0x04` battery frame every ~1 s and never slept; no reconnect
  occurred). Command bytes are correct; sleep appears gated by host activity/firmware
  conditions. Note the C# reference re-asserts `SetIdleTimeout` (+ lizard + gyro) inside its
  **CONNECT** handler - the controller likely resets these on each wireless (re)connect, so
  the engine's connect-handling should re-apply config on `Connected`; revisit idle behavior
  then. `set_lizard_mode`/`set_gyro` are exercised and known-good (raw pads + IMU data work).

**Deferred implementation decision (see 1.6).**
- **Keep-alive vs. reader handle synchronization** (Deck-only): choose A (mutex + timeout),
  B (dedicated write fd), or C (single IO thread) once the Deck is in the loop. Gordon
  ships lock-free/thread-free until then; the choice stays contained in the internal
  backend/device layer.

_(Resolved: feature-report set/get behavior, report-ID-0 framing, and wireless EPIPE
retry are confirmed against the `hid-steam` kernel driver - see 1.4.)_

### 1.10 Advanced steam-hid features (UNIMPLEMENTED - reference map)

_(Recorded here as a future-work reference. Steam exposes far more than we do;
captured for later. All 3 controllers unless noted. We currently only **write** feature reports -
every "get/check" below needs a `get_feature_report` round-trip helper. Command/setting IDs live in SDL
`controller_constants.h` + kernel `hid-steam.c` enums; a subset is mirrored in `protocol.rs`. **NOTE:
GET round-trips do NOT work on Triton** - its feature channel is effectively write-only, replies come
over the interrupt-IN stream with un-RE'd framing; see `Device::get_feature_report`.)_

- **Audio feedback - RESOLVED 2026-08-30 (`PLAY_AUDIO 0xB6` DROPPED); the audible-tone HAPTIC paths
  were then HW-characterized in depth 2026-09-01/02 (see the beep block below). Two separate things:**
  - **Rich audio (`PLAY_AUDIO 0xB6`) = ABANDONED, no reference.** `0xB6` is *defined* in every
    reference (SDL/kernel/C#/sc-controller) but **SENT by none** and has **no payload struct at all**.
    Its slots (0 startup..4 identify..6 normal, max 15) stay **empty until Steam uploads a blob** over
    `AUDIO_UPDATE_START/DATA/COMPLETE 0xB7-0xB9` + `SET_AUDIO_MAPPING 0xC1` - **HW-confirmed**: `0xB6`
    is silent until Steam pings the pad, then slots 4/5/6 play *rich* sounds (uploaded, RAM-resident,
    gone on restart). Blob format undocumented -> would need a USB capture. **Not worth it; dropped.**
  - **Simple audible feedback (beeps/tones on layer/mode change) = the HAPTIC actuator, not audio -
    HW-EXPLORED IN DEPTH (this session). Three per-device command paths, one parallel `examples/beep*`
    suite each; all three back onto `Device` methods.** (Gordon has no speaker; the trackpad
    voice-coils/LRAs are the "speaker". Kernel `steam_do_deck_input` plays the mode-switch notes on
    `0x8f` - a right-pad ack tick then a left-pad note ~1502 Hz on / ~1000 Hz off.)
    - **Gordon - `0x8f` `TRIGGER_HAPTIC_PULSE` (`haptic_pulse`), example `beep`.** Broadband voice
      coils reproduce arbitrary tones faithfully. A tone is `duration == interval` (50%-duty square
      wave, `f = 1e6/(dur+interval)`, `count` cycles). **Pitch is a clean continuous lever up to a high
      ceiling (~4-5 kHz)**; amplitude = **DUTY CYCLE** (`gain` inert on Gordon). Same primitive as
      Gordon's (low-freq) rumble. Gordon has no other haptic path.
    - **Deck - `0xEA` `SET_HAPTIC2` (`MsgTriggerHaptic`), example `beep-neptune`.** The Deck's LRAs are
      resonant, so the `0x8f` route collapses off-resonance (only ~5-6 usable pitches, coarse plateaus).
      `0xEA` is a firmware-SYNTHESIZED engine (SDL `haptic_type_t`) and IS the real Deck beep path:
      - **`cmd=Tone` (`haptic_tone(side,freq,dur_ms,gain,lfo_freq,lfo_depth)`) - WORKS.** Tracks pitch
        cleanly from ~200 Hz to a **~2 kHz ceiling** (higher = silent); no resonance collapse.
      - **`cmd=LogSweep` (`haptic_logsweep(side,start,end,dur_ms,gain)`) - WORKS**, glides start->end;
        the nicest-feeling cue.
      - **Levers:** `dbgain` (i8) is the amplitude lever (~8 dB good, `0` too quiet); **`ui_intensity`
        is INERT for a Tone** (a click-only lever, HW-confirmed).
      - **`cmd=Noise` - tested, DROPPED.** Fires but is rumble-only (~`Click`/Insane), moved ONLY by
        `dbgain`; `noise_intensity`/`rand_tone_gain` inert. Not a sound.
      - **`lfo_freq`/`lfo_depth` (LFO = low-freq oscillator, tremolo/texture) - WORKS but marginal.**
        Audibly makes a difference on a Tone, subtly; NOT yet a predictable/consistent lever. `lfo_freq`
        is a `u16` whose character keeps shifting well above ~64.
      - **`cmd=Rumble`/`Script` - UNTESTED** (`0xEB` covers rumble; script needs a firmware preset id).
      - Clicks (Tick/Click x `ui_intensity`, `haptic_cmd`) remain the reference-proven click path.
    - **Triton - output reports `0x80`-`0x85` (interrupt-OUT, not feature reports), example
      `beep-triton`.** Near-1:1 with the Deck's `0xEA`, but each primitive is its own report id:
      - **`0x83 LfoTone` (`lfo_tone_triton(side,freq,dur_ms,gain,lfo_freq,lfo_depth)`) - WORKS**, the
        Triton tone path. Pitch tracks to a **~1.9 kHz ceiling; 2 kHz+ is silent or repeats lower
        pitches**. `dbgain` amplitude (~8 default, a guess); LFO audible but subtle/not a consistent
        lever (as Deck). The one primitive that carries the LFO as a first-class report.
      - **`0x84 LogSweep` (`logsweep_triton(side,start,end,dur_ms,gain)`) - WORKS**, glides like the
        Deck's.
      - **`0x81 Pulse` (`pulse_triton`, `MsgHapticPulse`, structural Gordon-`0x8f` analog) - USABLE
        both ways** (first read as "erratic" while chasing beeps, but that was the wrong lens - it's a
        rumble, not a tone). As a **train** (`repeat_count>1`, `beep`-style freq sweep) it's a
        pulse/rumble usable across the range, with a narrow ~600-700 Hz band that oscillates oddly (the
        one sign that spooked the first look) - the rest is clean; NOT a clean tone path, but good as a
        rumble. As a **single** pulse (`repeat_count=1`) it's a discrete click, width (`on_us`) =
        strength, finer than `0x82`. **LEFT/RIGHT are physically SWAPPED** (like Gordon's `0x8f`; the
        motor/`0x80` and `0x82` sides are not); BOTH works. Probed in `haptic-triton` (`pulse` train /
        `clicks-pulse` single).
      - **`0x82 Command` (click, `haptic_command_triton`, `HapticStyle` off/weak/strong) - proven** (the
        reader's shipped click; only 2 distinct strengths, `amplitude` inert - the `0x81` single-pulse
        click above is the finer-grained alternative). **`0x85 Script` - untested/ignored.**
    - **Tone pitch ceilings (HW):** Gordon ~4-5 kHz * Neptune ~2 kHz * Triton ~1.9 kHz.
    - **Shared example command set (beep-style positional modes):** common - `kernel` / `pattern` /
      `feedback` / `note <hz> [ms]` / `sweep` / `fine` (500..3000, 100 Hz steps) / `gain` / `melody` /
      `vader`; device tails - `beep`: `duty` `ringdown`; `beep-neptune`: `chirp` `lfo` `dur` `clicks`;
      `beep-triton`: `chirp` `lfo` `clicks`. `melody`/`vader` note tables are shared in
      `examples/common`.
  - **Design direction (NOT built - deferred):** a device-independent "feedback sound" = a small
    **pattern** of `(pitch, duration, gap)` steps rendered per device (Gordon rich/continuous; Deck &
    Triton on the firmware Tone up to their ceilings; portable lever = **rhythm**, pitch a bonus).
    LogSweep is a good enter/exit-cue primitive on Deck+Triton. Wiring into `config`/`engine` deferred.
- **Reference-backed (implementable directly):**
  - **Gordon-dongle pairing:** SDL `SDL_hidapi_steam.c` - `ENABLE_PAIRING(0xAD)` = `[0xAD, 2,
    enable(0/1), duration_s]`, then `DONGLE_COMMIT_DEVICE(0xB3)` = `[0xB3, 0]`. Flow: enable ->
    controller announces (wireless status) -> commit to accept.
  - **Any device setting (set+get):** `SET_SETTINGS_VALUES(0x87)` (used) + `GET_SETTINGS_VALUES(0x89)`,
    shape `(id:u8, val:u16)`. ~80 settings (trackpad sens/noise, touch thresholds, click pressures,
    stabilizer, LED, idle, momentum...). `GET_SETTINGS_MAXS/DEFAULTS(0x8B/0x8C)` = valid ranges (basis
    for a Steam-like settings UI).
  - **Read-only queries:** `GET_DEVICE_INFO(0xA1)`, `GET_ATTRIBUTES_VALUES(0x83)`, `GET_CHIPID(0xBA)`,
    `GET_STRING_ATTRIBUTE(0xAE)` = the real serial getter (Gordon dongle reports empty -> `None`; this
    could fill that gap). All implemented for Gordon/Neptune (`examples/getters.rs`); Triton stalls
    (write-only feature channel).
- **Probeable without a dump (command known, payload trivial/none):**
  - **Trackpad calibrate:** `CALIBRATE_TRACKPADS(0xA7)` - likely a no-payload trigger.
  - **Gyro calibrate:** `CALIBRATE_GYRO(0xB5)` / `RESET_IMU(0xCE)` - likely no-payload "calibrate at
    rest" triggers.
  - **Gyro drift CHECK/CALIBRATE (host-side):** read gyro at rest, average -> bias; subtract in the
    mapper (we already have a small drift deadzone; this would make it a proper offset).
- **Needs a USB packet dump (unknown multi-byte payload / risky):**
  - **Custom audio/music upload:** `AUDIO_UPDATE_START/DATA/COMPLETE(0xB7-0xB9)` + `SET_AUDIO_MAPPING
    (0xC1)` - blob-upload protocol, unknown framing.
  - **Remove/list pairings:** `DONGLE_DISCONNECT_DEVICE(0xB2)`, `DONGLE_GET_CONNECTED_SLOTS(0xC4)` (the
    *read* may be probeable = slot bitmask).
  - **Trackpad calibration tables:** `GET_TRACKPAD_CALIBRATION/_FACTORY/_RAW(0xAA/0xAB/0xAC)`.
  - **(!) `RADIO_ERASE/WRITE_RECORD(0xAF/0xB0)` ~ FIRMWARE UPDATE - DO NOT TOUCH.** Also avoid
    `FACTORY_RESET(0x86)`, `SET_SERIAL_NUMBER(0xA9)`.
- **Triton caveat (the weak spot):** Triton uses a **different settings/config framing** (`sc2.py`:
  `0x87` + `configType` bytes, not the flat `SETTING_*` indices) -> settings surface known-good for
  Gordon/Neptune, **probe to confirm on Triton**. **Triton puck pairing** = a different v2 protocol
  (sc-controller captured but did NOT decode: `user/wireless_transport`, `esb/bond`, opcodes
  `ED/AD/DC/E2`) -> dump. **Grip-touch sensitivity/flicker** is Triton-new HW (no `SETTING_*` in the v1
  enum) -> probe or dump.

## 2. VIRT-OUT

**Purpose.** The output HAL - the symmetric peer to `steam-hid`. Emits virtual input to
the OS: mouse (move/buttons/scroll), keyboard, and gamepad. `engine` produces abstract
output events; `virt-out` implements the **sink** that realizes them per platform.
Platform code is isolated behind a compile-time-selected `Sink` (a per-OS concrete type,
cfg-picked - no trait), exactly like `steam-hid`'s concrete HID backend - `engine`,
`config`, etc. stay platform-agnostic.

**Discussion (why a separate crate).** The mapper has two device-facing layers, not one.
Output emission (uinput/ViGEm/CoreGraphics) is as large and as platform-specific as HID
input, and must not live inside `engine` - otherwise the mapping core becomes
platform-locked and hard to test. Extracting it keeps `engine` a pure function of
(input + config) -> output events, testable without an OS. **Decided: add `virt-out`.**

**Backends.**
- **Linux (primary):** `evdev` / uinput. Crate: **`evdev`** (has `uinput` virtual-device
  support; most widely-used/maintained Rust evdev binding; user has used it). Create
  virtual keyboard/mouse/gamepad via `/dev/uinput` - needs a udev rule / group, the same
  permissions story as `steam-hid` (track together).
- **Windows - DONE & in-game validated.** `SendInput` for mouse/keyboard; the virtual
  gamepad via a **compile-time-selected controller backend** (see 2.1): **ViGEm**
  (`vigem-client` + ViGEmBus driver, proven) or **VIIPER** (Virtual Input over IP
  Emulator - USB/IP, experimental), or neither (kb/mouse-only build). Both driver stacks
  are the user's.
- **macOS:** plan only - keep the path open (CoreGraphics/IOKit), no implementation yet.

### 2.1 Design (decided) & Linux vertical slice

**Crate shape.** `virt-out` exposes a platform-agnostic **sink** the engine drives, plus an
owned output **vocabulary** (`Key` / `MouseButton` / `GamepadButton` / `GamepadAxis` enums -
the set of things that can be emitted). Platform code sits behind a per-OS **`Sink`** selected
at compile time by `cfg(target_os)` (a concrete type, not a trait; Linux uinput; Windows
SendInput + a controller backend), mirroring `steam-hid`'s concrete HID backend. On Windows the
*controller* (virtual-gamepad) backend is
a further compile-time, mutually-exclusive choice (ViGEm / VIIPER / none - see the Windows
block below); kb/mouse is backend-independent. **Sync, no async** (emit is a syscall on the engine's
mapping-loop thread). **Update (3, DONE):** the output vocabulary was **extracted into a new
bottom `vocab` crate** (now built) shared by `virt-out` + `config` (so `config` need not
depend on `virt-out`); the input vocabulary is `config`'s own, hardware-independent.

**Output model - imperative event stream, not desired-state.** The engine is level-driven and
already reconciles desired-vs-applied output (4), so `virt-out` stays a thin realizer: it
takes a batch of `OutputEvent`s and flushes (SYN). **Levels:** `Key(k, down)`, `MouseButton`,
`GamepadButton`, `GamepadAxis(axis, value)`. **Deltas:** `MouseMove{dx,dy}`, `Scroll{dx,dy}`.
The engine holds "applied" state and sends only changes -> `virt-out` is dumb, the engine stays
golden-testable on the emitted stream.

**Linux backend - 3 separate uinput devices** (decided): keyboard (EV_KEY), mouse (EV_KEY
buttons + EV_REL X/Y/WHEEL/HWHEEL), gamepad (EV_KEY + EV_ABS). Cleaner identity than one
combined device.

**Gamepad = Xbox 360** (decided). A uinput device *is* an X360 pad via (1) **`input_id`**:
bus USB, vendor `0x045e`, product `0x028e`; and (2) the **xpad code set**: buttons
`BTN_A/B/X/Y`, `BTN_TL/TR`, `BTN_SELECT/START/MODE`, `BTN_THUMBL/THUMBR`; axes `ABS_X/Y` +
`ABS_RX/RY` (sticks, i16), `ABS_Z/RZ` (triggers 0..255), `ABS_HAT0X/Y` (dpad -1..1), each with
an `AbsInfo`. SDL2 derives a GUID from bus+vendor+product and applies its built-in X360
mapping -> games see a standard pad; uinput injects at the same layer `xpad` would (no driver).

**Force feedback - advertised now** (decided). The gamepad advertises `FF_RUMBLE`; we read
`UI_FF_UPLOAD`/`UI_FF_ERASE` off the uinput fd and translate effect magnitude -> Gordon
trackpad haptics (the verified low-freq `0x8f` drive, 1.9). **Risk:** uinput FF is a
request/response ioctl protocol, not plain event emission - **verify `evdev` 0.13 exposes it
before committing; else a small `libc` ioctl shim.** Spike this early in Phase A.

**Vertical slice (both phases):**
- **Phase A - DONE, hardware-verified.** The crate + Linux backend + 3 devices + FF, with a
  `selftest` example. Confirmed: `/proc/bus/input/devices` shows the pad as
  `045e:028e "Microsoft X-Box 360 pad"` with `js`/event nodes, 8 axes (`ABS=3003f`) and
  `FF_RUMBLE` (`EV` has EV_FF, `FF=10000`); kb/mouse events seen via `libinput` (it ignores
  gamepads - use `/proc` or `jstest`/`evtest` for the pad); `jstest` shows buttons/axes;
  `fftest` uploads **only** rumble effects (periodic/constant/spring/damper correctly
  rejected by the kernel) and `poll_rumble` reads magnitudes with the **correct** mapping
  (Strong/heavy->`strong`, Weak/light->`weak` - verified by effect-id tracing). Gotchas fixed:
  evdev's sync `VirtualDevice` fd is blocking, so `fetch_events` hangs on no FF - set the
  gamepad fd `O_NONBLOCK` and treat `WouldBlock` as "no rumble". FF play tracking is
  single-slot (last effect wins) - combine simultaneous effects later.
  (`evdev` 0.13.2 handles the whole uinput FF ioctl protocol; only extra dep is `libc` for
  the one `fcntl`.)
- **Phase B** - a `bridge` example (dev-dep on `steam-hid`) with a hardcoded mapping: real
  Gordon -> virtual mouse/keyboard/Xbox pad, **and the FF loop back**: `fftest`/a game rumbles
  the virtual pad -> we drive the real Gordon's trackpad haptics. Proves the whole
  ports-and-adapters path (incl. the 6-style haptic back-channel) before config/engine/
  Windows/Neptune.

**Permissions:** `/dev/uinput` needs a udev rule / group - same story as `steam-hid`'s hidraw;
enumerate the exact requirement during Phase A (user handles the Gentoo side).

**Windows backend - DONE & in-game validated.** Kb/mouse via `SendInput` with **scancode
injection** (games read scancodes); the **media/volume/browser keys** fall back to virtual-key
injection (they have only E0-extended scancodes, which would otherwise alias to letters);
**discrete and hi-res smooth scroll** both work. The virtual gamepad is a **compile-time,
mutually-exclusive controller backend**, opt-in - no default, so a plain build is kb/mouse-only
and drops controller output with a warning:
- **`vigem`** - virtual Xbox 360 pad via the ViGEmBus driver (`vigem-client`), with the FF
  (rumble) back-channel. The proven backend; validated in a real game.
- **`viiper`** - experimental, **runtime-verified** (2026-07-31): pad + rumble in-game, clean
  Ctrl-C shutdown, server survives + reconnects. VIIPER is **not** a local driver like ViGEm; it is a
  **USB/IP** system with three parts:
  (1) `usbip-win2`, a separately-installed USB/IP vhci **kernel driver**; (2) `viiper.exe`, a
  **server process** hosting a USB/IP endpoint + a TCP management/streaming API that
  auto-attaches created devices to the local driver; (3) `viiper-client`, a **pure-Rust TCP
  client** (`build=false`, no linking/DLL/FFI). We create an Xbox 360 device over the API and
  stream full input snapshots; rumble returns over the same stream. Heavier to deploy than ViGEm
  (usbip-win2 driver + a running `viiper.exe server`), though **no password is needed on localhost**
  (we connect plain). Two gotchas found in bring-up: the server's native-IOCTL auto-attach fails on
  the current driver build and falls back to `usbip.exe`, which **must be on PATH**; and the crate's
  *encrypted* path deadlocks on shutdown, so we **default to a plain (unauthenticated) connection**
  (both reported upstream). **Its USB/IP transport is an
  internal implementation detail of this one backend, NOT deckhand's 6 networking** - we run
  the VIIPER server on **localhost** and treat `viiper` as a purely local virtual pad, exactly
  like `vigem`. deckhand's own 6 network seam is separate, works for **every** backend
  (including ViGEm), and carries raw `ControllerState` with the Mapper on the sink; VIIPER's
  wire protocol is never used for it.

A `compile_error!` guards against enabling both backends. `virt-out` keeps no default feature; the
**engine (as the app) defaults to `vigem`** and forwards the choice: plain `cargo run -p engine` =
ViGEm pad, `--no-default-features` = kb/mouse-only, `--no-default-features --features viiper` = VIIPER.

**Still open / deferred:** macOS (CoreGraphics/IOKit, plan only); VIIPER runtime verification
(needs the `usbip-win2` driver + a running server). Dependency policy bites hardest here (niche
virtual-pad crates, driver installs) - Windows bring-up confirmed this (ViGEmBus / usbip-win2).

## 3. CONFIG

**Purpose.** The mapping configuration as a first-class, feature-rich concern: a
declarative **data model** of all bindings, its (de)serialization, validation, and
**compilation** into a fast runtime form. Feature target: not full Steam Input, but not
far off - action sets, layers (which subsume mode-shift), activators, gyro, trackpad modes, macros.

**Status - BUILT.** The **`vocab`** crate (output leaf enums, optional `serde`) and the
**`config`** crate (data model + serde/RON + `validate()` collecting all diagnostics, model
rounds A-E) are implemented and in use. **`compile()` -> `Program` moved to `engine`** (4.1
decision A: the IR is the runtime's contract, so it lives with its consumer - `config` stays pure
authoring data). Everything below is the design record; it is implemented unless noted otherwise.

**Discussion / decision (own crate from the start).** The UI needs the config *types*
(to render/edit bindings) but must NOT need the `engine` runtime. A separate `config`
crate means `ui` depends on `config` only - not on timers/output-sinks/`steam-hid`.
**Decided: build `config` as its own crate from the start** (rather than extracting it
later); we'd reach that point anyway, so keep it clean from day one.

**Crate graph & vocabulary ownership (decided).** `config` is *pure data* - it must not
drag platform backends (hidapi, evdev, the `windows` crate) into everything that touches
it, and it must not speak any controller's wire layout. Resolution (the "tiny types crate"
2.1 anticipated):
- A new bottom crate **`vocab`** (no deps) holds the hardware-independent **output** target
  enums (`Key` / `MouseButton` / `GamepadButton` / `GamepadAxis`), extracted from `virt-out`.
  Shared by `virt-out` (realizes them) and `config` (names them in actions).
- `config` owns its **own hardware-independent input vocabulary** (logical controller
  inputs - pads, sticks, grips, gyro...), NOT `steam-hid`'s wire-level `Buttons`. The
  `ControllerState -> logical-input` resolution is the **engine's** boundary job (fixed per
  device for now, not user-configurable).
- So `config` deps = `serde` + `ron` + `thiserror` + `vocab`; it links no hidapi/evdev/
  windows. `ui -> config` (+ `vocab`) stays free of those too (in the *embedded* build the UI
  still links them via `engine`; the win is for config-only consumers, the daemon-mode UI,
  and clean layering - plus config genuinely must not speak Gordon's vocabulary).

Layering (bottom-up): `vocab` -> { `steam-hid`(hidapi), `virt-out`(evdev/windows, `vocab`) }
-> `config`(serde/ron/`vocab`) -> `engine`(steam-hid/virt-out/config) -> `ui`(config [+engine
when embedded]).

**Data-vs-behavior + compile step (design I proposed, agreed).** Config is *data*; the
engine is *behavior*. Three layers, like source -> bytecode -> VM:
- `ConfigDoc` - on-disk, user-facing, forgiving format (serde); what the UI edits.
- **compile / lower** - validate references, resolve names->indices, expand sugar,
  precompute tables.
- `Program` - flattened, index-based, cheap to execute; **lives in `config`** (it is the
  output of config's own compile step; `engine` depends on `config` to consume it). Carries
  a **small metadata header (name/id)** so the engine's status API can report *what is
  loaded* - but **no file/profile linkage** (profiles are purely UI/filesystem).

This keeps a rich, permissive, editable format out of the ~4 ms hot loop and localizes all
validity checks in one testable place, so the runtime can assume a well-formed `Program`.
`compile()` is **per-profile (one `ConfigDoc` -> one `Program`)** and **collects all
diagnostics** (every dangling reference / unknown action) rather than failing on the first,
so the UI can surface every problem at once.

**Model sketch (feature-rich target).**
```
ConfigDoc { version, name, action_sets: Vec<ActionSet>, rumble, ... }  // ONE profile (device toggles -> DeviceConfig)
ActionSet   // full-controller mode; exactly ONE active at a time; `ChangeActionSet` swaps it
Layer       // partial overlay; STACKABLE; applied Hold / Toggle / Add / Remove
// one uniform node per top-level control (type name TBD: `InputSource` | `SourceBinding`):
InputSource ::= Button                    // standalone -> DEGENERATE: no mode, just bindings
              | ButtonGroup(mode)         // A/B/X/Y & D-Pad clusters; mode: ButtonPad(v1) | DirectionalPad/Joystick(defer)
              | Pad(mode) | Stick(mode) | Trigger | Gyro(activation)
//   a rich source = mode + Settings + directional bindings + click/touch sub-bindings
Commands    // every button-like node -> Vec<Command> (>=1; several activators/commands per node)
Command     // { activator, actions: Vec<Action>, settings }
            //   actions = a combo (1st = command, rest = subcommands); all fire together while the
            //   command is active - emitted as one level set per tick, NOT sequenced (see Round C)
Activator   ::= Regular{interruptible} | Long{ms} | Double{ms} | Start | Release   (Chord deferred -> entry gater)
            //   settings: Toggle, Turbo{rate}, Haptics{activation,strength}  (interruptible is on the Regular variant)
Action      ::= Key | MouseButton (incl. Scroll Up/Down/Left/Right pseudo-buttons) | GamepadButton
              | ChangeActionSet(id) | HoldLayer(id) | Add/RemoveLayer(id) | None
              // Macro deferred; MouseMove / continuous-scroll / stick+trigger axes = behavior outputs
Settings    // InputProcess: deadzones, curves, sensitivity, invert, gyro activation, per-mode params
```

**Model - Steam-aligned skeleton (part 2, decided; contents pending rounds A-E).** Mirror Steam
Input's paradigm and vocabulary faithfully; implement a **subset** initially; keep every enum
`#[non_exhaustive]` so deferred modes/activators/actions are **additive, not structural**. Locked
structure:
- **Two-level modes.** **Action Sets** (full-controller swap, exactly one active, `ChangeActionSet`)
  **and** **Action Set Layers** (partial overlays, **stackable**, Hold/Toggle/Add/Remove) - both from
  the start. The runtime resolution ("active set as base, apply active layer stack") is one path
  regardless, so Action Sets add little over Layers and retrofitting later would be worse.
- **Mode Shift - DROPPED (layers cover it).** Steam's per-input "mode shift" (an alternate config of
  one source, active while a button is held) is a **single-input hold-layer** - anything it expresses,
  a `Layer` does, and more (stackable, multi-input, toggle/add/remove). Adding it would mean a **second
  `SourceBinding` per input + a trigger field** - real format complexity for **no new capability**. So
  it is **not in the model**: author the effect as an explicit `Layer` + `HoldLayer` (the `config`
  `bridge_profile` example does exactly this). Revisit only if the authoring *convenience* is ever
  wanted - it would be a compile-time expansion into a 1-input hold-layer (the "expand sugar" step),
  changing no runtime.
- **Uniform node; buttons degenerate.** One node type per top-level control; a standalone **Button**
  has **no mode** (just Activator->Action), while rich sources carry **mode + Settings + directional
  bindings** (clicks/touches are separate `Button` inputs - see Round A). The UI surface stays degenerate for plain buttons (no mode
  picker), exactly like Steam. **`ButtonGroup`** (the A/B/X/Y and D-Pad clusters) is a group whose
  mode is `ButtonPad` (v1 - Steam's term for the cluster-of-buttons behavior) or
  `DirectionalPad`/`Joystick` (deferred). ("Button Group" is the logical InputSource; `ButtonPad` is
  Steam's *mode* name - a pad/group behaving as a set of buttons.)
- **Multi-activator everywhere.** Every button-like node - a standalone `Button` (incl. clicks/touches/
  trigger full-pulls), a `ButtonPad` member, or a **mode-synthesized virtual button** (a Trigger
  soft-pull, a `DirectionalPad` direction) - can hold **>=1 `Activator -> [Action]` entries**: separate
  activators (regular/long/double...) and/or `Chord`-gated modifier entries.
- **Clicks/touches/full-pulls - resolved (Round A):** every real **hardware bit** is a standalone
  `Button` (uniform digital model), *not* a sub-binding - clicks, touches, and **trigger full-pulls**
  included. What a source's **mode synthesizes** (a Trigger soft-pull, `DirectionalPad` directions, ...)
  is a **virtual** button/axis under the source - bindable, but not a hardware bit; **no exceptions**.
- **Gordon: `DPad` and `LeftTrackpad` are SEPARATE InputSources.** Hardware reports the dpad quadrant
  classifiers *and* raw pad X/Y independently every frame, so we expose both: `DPad` (a `ButtonGroup`,
  fed by classifiers on Gordon / the physical dpad on Neptune) and `LeftTrackpad` (a `Pad`). Keeps
  profiles portable Gordon<->Neptune (dpad-out->dpad-in uses hardware classifiers on both); binding both
  = double output, the user's choice - "go with what hardware reports."
- **Shapes + device-independent profiles (Round A).** A **`Shape`** = a device's input-layout/
  capability descriptor; **shapes live in `config`** so the daemon-mode UI authors offline, while
  **profile data stays device-independent**. The engine **ignores bindings for absent inputs**. Detail
  in Round A below.

**Drill-down rounds:** **A (done)** input vocabulary + shapes; **B (done)** behavior InputSources + settings;
**C (done)** activators (commands) + settings; **D (done)** actions; **E (done)** Tier-B above-profile config
(`DeviceConfig` + `Chords`) + rumble (all below). **Config model design COMPLETE (A-E).**

**Round A - logical input vocabulary + shapes (decided).** Device-agnostic **superset** of
InputSources - defined in full now (cheap enum variants); only Gordon is wired in V1, Neptune entries
are defined-but-stubbed until hardware:

| InputSource | Type | Gordon | Neptune |
|---|---|---|---|
| Face Buttons (A B X Y) | ButtonGroup | y | y |
| D-Pad | ButtonGroup | y (left-pad quadrant classifiers) | y (physical) |
| Left / Right Trackpad | Pad | y | y (+pressure) |
| Left Stick | Stick | y | y |
| Right Stick | Stick | - | y |
| Left / Right Trigger | Trigger | y | y |
| Left / Right Bumper (L1/R1) | Button | y | y |
| Left / Right Full Pull (L2/R2 click) | Button | y | y |
| Left / Right Grip (L4/R4) | Button | y | y |
| Left / Right Grip 2 (L5/R5) | Button | - | y |
| View (Back) - Gordon `<` / Deck [copy] | Button | y | y |
| Menu (Start) - Gordon `>` / Deck [menu] | Button | y | y |
| Steam (Guide) | Button | y | y |
| Quick Access - Deck `...` | Button | - | y |
| Left / Right Stick Click | Button | y (left only) | y |
| Left / Right Pad Click | Button | y | y |
| Left / Right Pad Touch | Button | y | y |
| Gyro | Gyro | y | y |

- **Uniform digital model (resolves the click/touch TBD).** Every **hardware digital bit** is a plain
  `Button` - face buttons, `ButtonPad` members, bumpers, grips, system buttons, **and** stick/pad
  **clicks**, pad **touches**, and **trigger full-pulls**. Every `Button` both carries `Activator ->
  Action` bindings **and/or** is referenceable as a **gate/activation** (a behavior mode, or gyro) -
  one mechanism, no special-cased "touch is only a gate" (touch is usually unmapped, as in Steam, but
  not special). A rich source's raw clicks/touches/full-pulls are thus separate `Button`s; the source
  itself contributes its **mode + settings + any virtual buttons/axes the mode synthesizes** (next
  bullet). Principle: *no special cases when we don't have to.*
- **Mode-synthesized virtual buttons/axes (no exceptions).** What a source's mode produces from its
  analog/positional/bit input is a **virtual** output living under the source, shaped by its settings:
  a `Trigger`'s **soft-pull** (a **configurable threshold** on the analog value), a `Pad`/`Stick`/
  `ButtonGroup` in `DirectionalPad`/`Joystick` mode (its directions/axis), a `Pad` in `Mouse` mode
  (motion). The soft-pull is **not special** - it's simply the Trigger's virtual button, the same
  category as a `DirectionalPad` direction. (Real hardware bits - incl. **trigger full-pulls** - are
  standalone `Button`s instead; full synthesis detail in Round B.)
- **Shapes + device-independent profiles.** A **`Shape`** = a device's input-layout/capability
  descriptor (present InputSources + flags: `pad_pressure`, `dpad: physical|pad-derived`,
  `has_right_stick`, ...). **Shapes live in `config`** (a `Shape` enum + capability tables) so the
  **daemon-mode UI authors offline** - making the crate *shape-aware* while **profile data stays
  device-independent** (a `ConfigDoc` references logical inputs only). The engine maps
  `steam_hid::DeviceKind -> Shape` and **ignores bindings for inputs absent** on the live device; a
  profile authored for one shape silently drops what the device lacks - **the UI may warn**, but
  config/engine just drop. Keeps profiles portable Gordon<->Neptune.

**Round B - behavior InputSources + settings (decided).** Each Round A rich source takes a
**source-specific behavior** (Steam's names) that produces **virtual buttons** (bound via
activators->actions like real `Button`s, Round C) and/or drives an **output target chosen in settings**,
plus a settings palette. A behavior is picked from the set valid for its physical source:

| Behavior | on | virtual buttons (-> activators->actions) | output target (a setting) | v1 settings |
|---|---|---|---|---|
| **Joystick** | Pad, Stick | `OuterRing` | gamepad stick (L/R) | **axis**, inner deadzone, anti-deadzone, outer-ring radius, curve, invert X/Y, rotation, activation |
| **DirectionalPad** | Pad, Stick | U/D/L/R + `OuterRing` (5) | - | register deadzone, 4-way/8-way, outer-ring radius, rotation, activation |
| **AsMouse** | Pad | - | cursor / scroll | **axis**, sensitivity, **acceleration**, invert X/Y, rotation, 1-Euro filter, activation *(no deadzone; velocity behavior -> accel not curve)* |
| **JoystickMouse** | Stick | - | cursor / scroll | **axis**, sensitivity, inner deadzone, invert X/Y, **curve**, rotation, activation *(deflection behavior -> curve not accel; no 1-Euro - a stick is already smooth)* |
| **GyroToMouse** | Gyro | - | cursor / scroll | **axis**, per-axis sensitivity, invert X/Y, radial deadzone, acceleration, rotation, 1-Euro filter, `space`, activation |
| **Trigger** | Trigger | soft-pull | gamepad trigger (L/R) | soft-pull threshold, output curve, input deadzone |
| **ButtonPad** | ButtonGroup | 4 buttons | - | - |

- **Virtual buttons vs settings (the split).** A behavior's **virtual buttons** (`Joystick`/`Dpad`
  `OuterRing`, `Dpad` directions, `Trigger` soft-pull, `ButtonPad`'s 4) are bound on the main page via
  activators->actions, exactly like hardware `Button`s. Everything else - output target, deadzones,
  curves, sensitivities - is chosen in the source's **settings**, not bound.
- **Nullifying (two kinds, added post-Round-E).** (1) `SourceBinding::None` - an explicit **unbind**,
  valid on ANY source. In a **layer** it *overrides* a base binding ("this input does nothing here" -
  e.g. suppress a pad's mouse while a mode-shift layer is held); in a base set it equals omission. It is
  the general nullifier for non-button sources (which have no top-level activator to leave empty).
  (2) **Output target `None`** on `TriggerOutput`/`StickOutput` - drop the primary gamepad axis but
  **keep the behavior's virtual buttons** (e.g. a trigger bound only to a mouse click, no phantom LT/RT
  axis). Added **only** where a behavior has both an axis and virtual buttons; `MouseOutput` deliberately
  has **no** `None` - mouse behaviors have no virtual buttons, so "no output" already *is*
  `SourceBinding::None` (one clear way, no redundant second path). *(Rust variant name `None` => RON
  serializes `r#None`; plain `None` also parses.)*
- **Shared vs source-specific.** `Joystick` and `DirectionalPad` are **one shared behavior each**, valid
  on Pad *and* Stick with the same settings - **parametrized, not split** (the only pad-vs-stick
  difference, "requires click," is a gater - see activation). The three mouse behaviors are **distinct**
  (`AsMouse` = pad delta, `JoystickMouse` = stick deflection->rate, `GyroToMouse` = angular velocity):
  different signals, different math + settings. Their output target = **cursor or scroll** (scroll = our
  extension over Steam; on `GyroToMouse` scroll may be nonsensical but kept for consistency).
- **Activation - general (provisional).** Promoted from GyroToMouse-only to an **optional per-behavior
  setting**, default *always*: `mode: HoldToEnable | HoldToDisable` + `gaters: [Button]` (plain
  hardware-bit buttons incl. trigger full-pull; **OR-combined; no thresholds** - deferred).
  HoldToEnable + no gater = **never**; HoldToDisable + no gater = **always**. One mechanism expresses
  **both** gyro gating (`gaters=[FullPull]`) **and** pad-`DirectionalPad` "requires click"
  (`HoldToEnable, gaters=[LeftPadClick]`), so `Joystick`/`DirectionalPad` stay single shared types.
  *(User not fully convinced - provisional; revisit if it complicates.)*
- **Gyro `space`** = an extensible enum: **`Yaw` / `Roll` / `YawRoll`** (local presets - horizontal
  from yaw / roll / yaw+roll; `YawRoll` default) and **`PlayerSpace`** (yaw+roll projected onto the
  gravity axis, normalized; vertical stays local pitch). **IMPLEMENTED + HW-verified.** World-space /
  laser-pointer, and fusion-based gravity (player space uses a crude accel low-pass), deferred.
  **Rotation, the 1-Euro (One-Euro) filter, and player-space are V1** - not deferred polish (the earlier
  "engine polish" note meant "don't polish the bridge now," not "exclude these from v1").
- **Axis limit (`Axis: Both | Horizontal | Vertical`; added 2026-08-21).** On the four **2D** output
  behaviors (`Joystick`, `AsMouse`, `JoystickMouse`, `GyroToMouse`; **placed just below `output`**),
  constrains the behavior to a single **screen** axis - `Horizontal` keeps x and forces y=0, `Vertical`
  keeps y and forces x=0, `Both` (default) is no limit. Semantics = **make it a true 1-D control**, so
  the mapper applies the mask **early** (see 4.2), not as a naive output null: on the deflection
  behaviors before the radial deadzone/curve (so the kept axis gates + rescales on its *own* magnitude),
  on the velocity behaviors before the 1-Euro filter + acceleration (so accel reads the kept axis' speed
  only). Config carries it as plain data; all zeroing is mapper-side. *(Not on `DirectionalPad`/`Trigger`
  - a dpad is already directional and a trigger is 1-D.)*
- **Shared setting types** (defined once, reused): `Deadzone(inner, radial)`, `AntiDeadzone`,
  `Curve(Linear | Power)`, `Sensitivity`, `Acceleration(simple)`, `Rotation`, `OneEuroFilter`,
  `OuterRingRadius`.
- **Dropped / deferred.** Pad `ButtonPad` **dropped** (`DirectionalPad` supersedes it). Deferred
  behaviors: pad `ScrollWheel` (circular-edge), `MouseRegion`, `MouseJoystick`, Radial/Touch menus;
  stick `JoystickCamera`; gyro `GyroToJoystick`; ButtonGroup `Directional`/`Joystick`. Deferred
  settings: outer deadzone/saturation, named/custom curves, dpad overlap/cross-gate.

**Round C - activators (commands) + settings (decided).** Every button-like node (hardware `Button`s
*and* the virtual buttons from Round B - OuterRing, dpad directions, soft-pull, ButtonPad's four) holds
`commands: Vec<Command>` - several activators per node.

```
Command { activator, actions: Vec<Action>, settings }
```
- **Command structure.** A **command** *is* an activator: an **activator type** + its **settings** +
  an **action list**. The 1st action is the "command," the rest are **subcommands** (action-only - no
  own type/settings) for **key combos** (e.g. `Ctrl` + `C`). All actions of a firing command are
  **held together** while it fires - they are **not sequenced**: every action becomes a desired output
  *level* and the level reconciler emits them as one set per tick (4.2). **Declaration order is NOT
  honored** - see the ordering note below. Settings apply to the whole combo. (Replaces the sketch's
  `entries: Vec<(Activator, Vec<Action>)>`.)
  - **Combo ordering (the truth, corrected).** The intent was "modifiers press before the key, release
    after" (`Ctrl-down C-down / C-up Ctrl-up`). The **level reconciler doesn't sequence within a tick** (it's a
    `BTreeSet<Key>` diffed and emitted **sorted by the `Key` enum's ordinal**, releases before presses
    - deliberately, so no key can strand across a layer swap). So the *declared* order of subcommands
    (or of separate commands on the node) never affects output. **Modifier combos still come out
    correct** because the `Key` enum lists **all modifiers first** (lowest ordinals), so a modifier's
    key-down is always emitted before a letter's within the same tick's event batch, and all land in
    one frame. This means `[Ctrl, C]` and `[C, Ctrl]` produce **identical** output; a combo whose
    intended sequence *contradicts* enum-ordinal order (two letters, or a key-before-modifier) would
    not be honored. Making emission order-preserving is a scoped reconciler change, **deliberately not
    done** - the only observable effect today is terminal autorepeat cosmetics, and the level model is
    load-bearing for stuck-key safety.
- **Activator types (v1):** `Regular{interruptible} | Long{hold_time} | Double{window} | Start | Release`.
  (Cycle, Double-and-hold deferred.)
- **Per-command settings (v1):**
  - **Interruptible** - *must-have*. **Lives on the activator, not in `CommandSettings`:**
    `Activator::Regular { interruptible: bool }` (it's `Regular`-only, so a field of the variant makes
    the invalid combos unrepresentable and drops the engine's runtime `matches!(Regular)` guard). Its
    behaviour is the full activator model below, not a simple suppress.
  - **Haptics** - `activation: Off | OnPress | OnRelease | Both`, `strength: Low | Medium | High`.
    Fires **on the action's edges, not the raw button** (so `Long` pulses after its timeout; `Turbo`
    repeats it). A **singular click** at one of 3 strengths - the reader maps the level per device
    (Gordon `0x8f` pulse duration / Deck `0xea` gain, 1.9). **Actuator side = the triggering input's
    side** (below). **The click is paired to the effect that LANDS** - a level command clicks on its
    output edge, but an OpSet's click is deferred and gated on the op actually landing (dedup, no-op,
    engage/disengage); see the 4 "Command-haptic pairing" addendum (2026-08-30).
  - **Toggle** (latch on/off per activation), **Turbo** (`{rate}`, repeat while held), timing
    (`hold_time`, `window`, `turbo_rate`). Deferred: fire-delay.
- **Haptic actuator side.** Inputs on the **left** buzz the **left** actuator, **right**->right (as in
  the user's prior software). `Side(InputSource) -> Left | Right` is a **device-independent** match in
  `config` - the logical vocab already encodes left/right and the cluster/center assignments are
  consistent across Gordon+Neptune (`ABXY`=right, `DPad`=left), so **no `Shape` table is needed**.
  Central buttons: **View->Left, Steam->Left, Menu->Right, QuickAccess->Right**; gyro is moot (no
  haptic-firing virtual buttons). (Movable into `Shape` later if a mirrored device ever appears.)
- **Chord - deferred** (hold-layers cover it). When added it's an optional `gaters: [Button]`
  on a `Command` (reusing Round B's gater), not a new activator type.
- **Engine resolution - the activator model (not config; config just declares, engine resolves).**
  A node's commands are evaluated together each tick (`engine/src/mapper/command.rs`, two passes:
  classify hold-takers -> resolve every command's output). Five activators, two roles:
  - **`Start` / `Release`** - independent one-shot taps (`TAP_MS`~40 ms) on the press / release edge.
    Never interrupt, never interrupted, don't participate in anything below.
  - **`Long` / `Double`** - the **interrupters** and **hold-takers**. Fire when their condition holds
    (`Long`: held past `hold_ms`; `Double`: a **second press within `window_ms` of the *first press***
    - press-to-press, so holding the first press past the window forecloses a double) and then **hold
    from that point until release**. They **never contest each other** - several can be active at once
    and they all stay held (`double(200),long(300),long(500)` held long -> all three down until release;
    verified against Steam with `wev`, the terminal's autorepeat had earlier made it *look* like only
    the last held). While any is active, an interruptible `Regular` is killed.
  - **`Regular`** - the only interruptible command; the whole behaviour is **one rule:**
    > *A `Regular` presses-and-holds the moment it becomes **safe** from interruption* - "safe" = no
    > `Long`/`Double` on the node can still fire from this interaction. Safe at press (no interrupters
    > present) -> holds from press. Becomes safe later **while still held** -> holds from that moment.
    > Safe only after release (or button already up) -> a `TAP_MS` tap. A `Long`/`Double` fires before
    > safe -> interrupted, no output.

    Consequences (all fall out of the one rule):
    - No interrupters on the node -> holds from press (`interruptible` is a no-op - the neutral default).
    - `Long` present: short press (released before it fires) -> tap on release; hold past threshold ->
      interrupted. A `Long` never permits "safe while held" (it either fires or keeps threatening), so
      a committed **real hold** only happens with `Double`(s) and **no** `Long`.
    - `Double` present: safe only when the window closes (from first press). Quick press -> **tap after
      the window**; **press-and-hold** past the window -> a **real, delayed press-and-hold**; a genuine
      second press -> `Double` fires, `Regular` interrupted. (The window latency is unavoidable - you
      can't know it's *not* a double until the window elapses.)
  - `toggle`/`turbo`/haptics are post-processing on the resolved level; meaningful on holds, inert on
    the one-shot taps (the editor hides the ones that don't apply). Full worked examples + golden
    cases live in `command.rs`'s module docs and tests.

**Round D - actions (decided).** An `Action` is what a command/subcommand fires. Two families:
- **Output** (from `vocab`, realized by `virt-out`): `Key(Key)`, `MouseButton(MouseButton)`,
  `GamepadButton(GamepadButton)`. **Discrete scroll is folded into `MouseButton`** as pseudo-buttons
  (`ScrollUp/Down/Left/Right`) - a scroll tick is a button-like impulse (fires per activation, `Turbo`
  repeats), a la X11 buttons 4-7 / the Windows wheel; the backend realizes them as wheel ticks, not
  `BTN_*`. (`MouseButton` set: Left/Right/Middle/Side/Extra + the four scroll pseudo-buttons.)
- **Config/engine** (no OS output): `ChangeActionSet(id)`, `HoldLayer(id)` / `AddLayer(id)` /
  `RemoveLayer(id)`, and `None` (explicit no-op / block).
- **`Key`** = adopt the sibling `uinput-simulation`'s grouped `Key` enum (through `KbdIllumUp`; the
  obscure codes below are additive later) into **`vocab`** as **bare names** - no embedded evdev value;
  the backends map (`virt-out` Linux -> `evdev::KeyCode`, Windows -> scancode). Adjustments applied on
  import: media pairs **backward-first** (`PreviousSong` before `NextSong`, `Rewind` before
  `FastForward`) to match the Down/decrement-first convention of Volume/Brightness/KbdIllum; `KpDot`
  moved beside `Kp0`; `Menu` + `Compose` split into their own small **"Special"** group.
- **Not actions** - continuous outputs (`MouseMove`, **axis/continuous scroll**, gamepad **stick /
  trigger axes**) come from a *behavior*'s output-target/settings (Round B), never a button-fired
  action.
- **`Macro` deferred** - a timed **sequence** (press A, wait, release A, press B...), distinct from the
  Round C **simultaneous** combo (all actions held together, e.g. `Ctrl`+`C` - no sequencing). Slots in later as
  `Action::Macro(Vec<MacroStep>)` (`{ action, down/up, delay }`). Steam/system actions also deferred.

**Round E - above-profile config (Tier B) + rumble (decided). Completes the model.**

> **SPLIT (2026-08-19):** the original single `GlobalConfig` blob was split into **two independent,
> separately-stored, uncompiled config "beings"** - `config::DeviceConfig` (device-local reader-side
> settings) and `config::Chords` (the top-level switch/command chords, a third engine slot). The text
> below is the **post-split** shape; `GlobalConfig`/`GlobalChord`/`GlobalAction` no longer exist.

*DeviceConfig* (`config/src/device.rs`; profile-independent, **device-local**; **uncompiled**;
handed to the engine and applied **reader-side** on connect, `ReaderCfg::for_device`; **never crosses
the 6 wire** - each machine keeps its own; on disk `devcfg.ron`):
```
DeviceConfig {                  // container #[serde(default)] -> old files migrate (unknown fields ignored)
  led_brightness: Option<u8>,   // 0..=100 %  (None = leave default)
  idle_timeout:   Option<u16>,  // seconds    (None = leave default; caveat below)
  gordon:         GordonTuning, // Gordon pulse-train shaping (duty lever + freq)
  neptune:        RumbleTuning, // Deck motor shaping (speed + gain levers)
  triton:         RumbleTuning, // Triton motor shaping (separate - different motors)
}                               // NO master_rumble (dropped 2026-08-27); NO top-level rumble_hz (-> gordon.hz)
GordonTuning { duty: Lever<u8> /*% of drive*/, hz: u16 /*pulse rate, default 60*/ }
RumbleTuning { speed: Lever<u8> /*% of drive*/, gain: Lever<i8> /*dB, -8..16*/ }
Lever<T> ::= Fixed(T)                     // constant, strength-independent
           | Scaled { min: T, max: T }    // per-motor strength lerped into [min,max]
// drive(strength)->u16 (renamed from speed()): strength 0 -> 0 (a zero motor is ALWAYS silent); min is
//   a floor for NONZERO strength only. Scaled{0,100} = raw identity. Defaults: duty/speed Scaled{0,100},
//   gain Fixed(2) (Neptune) / Fixed(0) (Triton) - reproduce the removed NEPTUNE/TRITON_L/R_GAIN consts.
```
*Chords* (`config/src/chords.rs`; the third engine slot, an `Option<Chords>` beside the two program
roles; **uncompiled**; live hot-swappable; on disk `chords.ron`):
```
Chords { chords: Vec<Chord> }   // named struct (not a bare Vec) so it can grow chord-level settings
Chord  { buttons: Vec<Button> /*physical bits, AND*/, action: ChordAction }
ChordAction ::= SwitchProfile { mode: SwitchMode }       // main<->fallback
              | CommandExecute { command, args }         // run a headless tool
SwitchMode  ::= HoldFallback   // force Fallback while held; base on release
              | Toggle         // flip base (Main<->Fallback) on each engage
              | SetMain        // latch base to Main on engage (idempotent)
              | SetFallback    // latch base to Fallback on engage (idempotent)
```
- **No global rumble knob** - `master_rumble` was **dropped (2026-08-27)** as redundant with the
  per-device levers (each device's `duty`/`speed`/`gain` already scale its own strength), and
  `reader::scale_master` went with it. **Rumble shaping is applied reader-side**, not in the mapper:
  the mapper's `RumbleCmd` is profile-scaled only; the reader applies the **bound device's** tuning at
  *emit* - Gordon's `GordonTuning` (duty lever + pulse `hz`) via `apply_gordon`, or Neptune/Triton's
  `RumbleTuning` (speed + gain) via `apply_rumble{,_triton}`. The level is kept **raw** and the levers
  read from `ReaderCfg.rumble` (a `Gordon|Neptune|Triton` enum holding only the bound kind's tuning),
  so a live tuning change lands on the next re-fire.
- **Device toggles are device-local** - `led_brightness` (verified working) and `idle_timeout`
  (writes correctly, but sleep is gated by host read-activity -> mostly moot on a running engine, kept
  for completeness). **Gyro/IMU enable is auto-derived** (on iff the program uses a `GyroToMouse`);
  **lizard is always off** while running (raw input) - neither is a setting.
- **`chords`** = **physical** hardware-bit buttons **AND**-combined; the engine **evaluates them first
  and consumes** their buttons (4), so they work UI-down/daemon-only. **Each `ChordAction` carries
  its own params:** `SwitchProfile` holds a `SwitchMode` (`HoldFallback`/`Toggle` are relative to the
  current role; `SetMain`/`SetFallback` latch a **specific** role on engage - same persistent outcome
  as `Toggle` but unconditional); `CommandExecute` **spawns a
  subprocess** - the escape hatch for system actions (on-screen keyboard, switch audio device,
  brightness...) that links **no X/Wayland/DE/audio deps**, keeping the engine headless/system-dep-free
  on Linux (4). No `reload` action (the engine has no on-disk config - it's pushed from the UI).

*rumble feel* (per-profile, in `ConfigDoc`): `rumble { strength /*default 100 %*/,
curve /*default Linear*/ }` - game rumble (strong/weak) -> Gordon trackpad haptics; the profile tunes
texture, the per-device rumble levers (`DeviceConfig`) shape it. **`hz` is NO LONGER per-profile** -
it now lives in `DeviceConfig::gordon.hz` (device-level, applied reader-side; was the top-level
`rumble_hz` between the 2026-08-19 split and the 2026-08-27 per-device rework).
`curve` = strength->drive response (the non-linear Xbox-strength map is the future tweak, 1.9).
Per-profile `strength` **may exceed 100 %** to boost games that under-drive their FF (1.9).

**Complete `ConfigDoc`:** `{ version, name, action_sets: Vec<ActionSet> (+default/active), rumble }`
- **no `device_settings`** (LED/idle live in `DeviceConfig`). **Config model design COMPLETE (rounds
A-E) and BUILT** - the `vocab` + `config` crates are implemented (see the 3 status note above).

**Two tiers (decided).** Configuration splits into two profile-independent levels:
- **Tier A - Profile:** `ConfigDoc -> Program`, above. One per file, hot-swappable; holds
  everything *inside* a mapping (action-sets, layers, bindings, per-profile gyro/rumble
  texture/deadzones).
- **Tier B - above-profile (engine-level):** since the 2026-08-19 split, **two independent uncompiled
  beings**, each authored by the UI, handed to the engine directly (not per profile-swap):
  - **`DeviceConfig`** - device-local settings applied **reader-side**: per-device rumble shaping
    (`gordon`/`neptune`/`triton`), LED, idle. Staged (applies at next `start()`); **never crosses the
    6 wire**.
  - **`Chords`** - the **top-level chords** (each `Chord` fires a `ChordAction` carrying its own params
    - `SwitchProfile{HoldFallback|Toggle|SetMain|SetFallback}` for main<->fallback, or
    `CommandExecute{command,args}` for system actions via headless tools, keeping the engine
    system-dep-free). A third live engine slot; hot-swappable. Full detail in **Round E** above.

The engine therefore holds `{ device_config, chords: Option<Chords>, program[main],
program[fallback] }` - two live profile Programs, the device settings, and the chords slot. `config`
compiles one profile at a time and never itself knows about "two"; the UI orchestrates which Programs
are loaded and in which role. (Engine mechanics - the chord switch, apply-API role tagging -
are the engine talk, 4.)

**Versioning (decision - deferred).** During initial development the on-disk format WILL
break as it evolves - that's fine, test setups getting broken is acceptable. Config
**versioning + migration** is added only at the **stable release**, after which the
format is a non-breaking user contract. This is the explicit exception to the "break the
API freely" rule (0), and it applies only post-stable.

**Profiles & switching (decision).** **One `ConfigDoc` = one profile** (its own action-
sets/layers); the always-reachable **desktop / fallback profile** is just another
`ConfigDoc`. **Switching between profiles is manual for now.** Automatic per-application
switching (detect the foreground game -> activate its profile) is **explicitly out of
scope** initially - it needs per-OS foreground detection that is painful on Wayland and
ugly on Windows. Kept as a possible future capability, not planned.

**Where switching lives (boundary).** Manual switching, and automatic switching *if it is
ever built*, belong in the **`ui`/tray app** (5) - not in the engine. The
`engine`/daemon is **headless** and has **no concept of what applications are running**:
it holds the compiled program(s) (main + fallback) + device-config + chords and maps whatever is
active, but never inspects the desktop. Consequently the engine must **not link any
X/Wayland/desktop-environment code on Linux**; only the `ui` may. (On Windows the engine
may still need some `winapi` linkage for its own output/device work, but not for
app/foreground awareness.) The UI decides "which profile is active" and tells the engine.

**Config ownership (decision).** The **UI owns the config lifecycle** - authoring,
persistence to disk, managing the set of profiles, and selecting the active one. It hands
a (compiled) config to the engine via the narrow control API (4); the engine keeps no
notion of profiles and no ownership of the on-disk store - it just runs what it's given.
A daemon variant, if ever built, is fed the same way (UI or a small CLI config-sender).

**Serialization format (decided: RON).** The model is nested and **enum-heavy** (input
sources, activators, actions are all sum types); RON represents Rust enums/nesting cleanly,
supports comments, and is more editable than JSON - the right fit for a UI-authored,
occasionally-hand-inspected format. (TOML fights deep nesting/enums; JSON has no comments.)

**Skeleton (decided).** Edition 2024, dual MIT/Apache, `publish = false`, **no `Copy`**
(project convention), `serde` a **hard dep** (the crate's purpose, not an optional feature).
Deps: `serde`, `ron`, `thiserror`, `vocab`. Compile errors: **collect-all** diagnostics.

## 4. ENGINE

**Purpose.** The **headless runtime / manager** - the central place the whole mapper
runs, with no UI. It owns and drives everything below it: opens and manages devices via
`steam-hid` (lifecycle, the Neptune keep-alive, hotplug/reconnect), routes each device to
its mapping, runs a compiled `config` `Program` over the input, and emits the resulting
output through `virt-out`. It holds the live runtime state - active action set/layer,
activator timers, turbo counters, gyro integration, trackpad momentum. `engine` depends
on **both** `steam-hid` and `virt-out` directly and drives them; it is the thing a daemon
or CLI binary is a thin wrapper around.

**Status - CORE COMPLETE & HW-validated.** The `engine` crate is built: `Program` IR +
`compile()`, the pure golden-tested `Mapper`, and the threaded runtime/manager (reader-thread-
per-device -> central mapping loop -> `virt-out`), driven by the `deckhand-run` example. See 4.2
for the step-by-step record. Remaining work is the deferred list at the end of 4.2 (daemon/
socket + 6 networking, multi-device->multi-Mapper, USB hotplug, and a few polish items).

> This crate is deliberately **not** a side-effect-free "pure core." It manages real
> devices and real OS output. Purity is kept *inside*, at the mapping step (below), where
> it actually pays off for testing - not by pushing I/O out of the crate.
>
> But it **is** strictly headless: it has **no concept of running applications or the
> desktop** and must **not link any X/Wayland/DE code on Linux**. It holds the config and
> maps the active profile; it never decides *which* profile to activate from desktop
> state. Profile switching (manual now, automatic if ever) lives in `ui`/tray (3, 5),
> which tells the engine. (Windows may need some `winapi` linkage for output/device work,
> never for app/foreground awareness.)

**Design notes (mine, agreed).**
- **Manager shell + pure inner step.** Factor a pure mapping step -
  `step(state, config, clock) -> output_events` - that the manager shell drives each tick.
  The shell does the I/O (read `steam-hid`, write `virt-out`, timers, threads); the step
  is a pure function of (input + config + injected clock). **Inject the clock** - activators
  are timing-based; never read "now" deep in the step. This keeps the mapping logic
  **golden-testable without hardware**: feed recorded input traces (from the `steam-hid`
  `dump`, 1.8) + a config into `step` -> assert output events. "Pure & testable" applies
  to the step, not the whole crate.
- Consumes `steam-hid`'s `ControllerState` directly (don't over-abstract the input side);
  emits abstract output events into `virt-out`.
- **Level-driven mapping - the diff is on the *output*, not the input.** Each tick the step
  (a) takes the current input **snapshot** (level, exact - never `steam-hid`'s `diff()`),
  (b) computes the **desired output state** as a function of current input + config +
  runtime (active layer, timers), then (c) **reconciles desired-output vs. applied-output**
  and emits only the difference to `virt-out`. So "hold the virtual button while the
  physical is held, release it when the physical releases" falls out of *output*
  reconciliation - no input-edge diffing - and it composes cleanly with layers
  (a held button is recomputed fresh each tick, so a layer change can't strand a stuck
  output). Where activators genuinely need input **edges** (long-press, double-tap,
  on-release, turbo), the step derives exact **digital** transitions from its *own*
  retained previous snapshot (`cur.buttons & !prev.buttons`) - digital, so no deadband is
  ever involved, and independent of the `steam-hid` `events()`/`diff()` convenience (which
  the engine does not use; 1.5). Analog deadzones/curves/gyro sensitivity are the
  *configurable* `InputProcess` (3), applied to raw snapshot values - never a fixed
  library filter.
  - **Why output-side reconciliation especially pays off with filters.** When a smoothing
    filter sits in the path - e.g. the **1-Euro (One Euro) low-pass filter** on gyro or a pad,
    or a response curve - it removes jitter, so a steady hold yields a steady (often
    zero-delta) filtered output. Reconciling on the output then emits a virtual event only
    when the **filtered result** actually changes, silently absorbing the raw input jitter
    the filter smoothed away. Input-edge diffing can't do this - it never sees that the
    filter flattened those frames.
- **Layer holds latch to the triggering node's held-state (avoids self-shadowing oscillation).** A
  `HoldLayer` hold is registered when the resolved binding fires it and **keyed to the held-state of the
  node that fired it**: it persists while that node is held - *even if* the layer it activates then
  **overlays that same node** so its new binding no longer names `HoldLayer`. Without this latch - i.e.
  re-deriving the active-layer set from the *currently-commanded* `HoldLayer`s each tick - a
  self-shadowing button **oscillates** (layer on -> node rebound -> the command keeping it alive vanishes
  -> layer off -> node reverts -> ... strobing every tick). Output reconciliation (above) still prevents
  *stuck* outputs either way; the latch is what makes it *stable*.
  - **The latch is robust only for *frame-local* triggers.** "Physical" isn't the real criterion -
    *"always derivable from the raw frame, so it can't vanish when re-bound"* is. **Physical hardware
    bits** qualify (always present), and so do the **trigger soft-pull / full-pull** (a frame-local
    threshold on the always-present analog). A **fully pipeline-derived** virtual (a `DirectionalPad`
    direction from a pad's position+mode, an outer-ring) - *or even a soft-pull whose layer overlay uses
    a different threshold* - can effectively **vanish** under the active layer (e.g. base fires at
    threshold 0.5, the layer's overlay needs 0.9, the analog sits at 0.6), so its latch can't hold ->
    **flicker**. This is never a *stuck* key (reconciliation), only strobing.
  - **Decision: `HoldLayer` / mode-switch actions are permitted on *any* button-like node, virtual
    included** - *unlike* **activation gaters** (gyro/behavior), which are physical-only. The reason
    gaters are physical is **evaluation order, not the `InputSource` typing** (the typing merely
    *encodes* the rule): a gate answers "does this behavior run this tick?", so it must be resolvable
    **before** the behavior runs - i.e. from the **raw frame**, up front. A virtual button is a behavior
    *output*, so gating on it inverts the dependency (behavior A's gate needs a virtual from behavior B,
    which may be gated by a virtual from A -> a **cyclic** behavior-dependency graph needing per-tick
    topological sort + cycle checks). Physical/raw-frame gates keep the tick a single fixed **acyclic**
    pass (read frame -> resolve all gates -> run behaviors in any order -> reconcile). `HoldLayer` doesn't
    hit this: it's a **runtime-state hold** evaluated as the tick evolves - if its trigger vanishes the
    tick still completes (worst case flicker), no ordering problem. So gating *enforces* frame-local;
    `HoldLayer` *tolerates* non-frame-local. The doubly-pathological self-shadowing of a virtual/
    threshold-fuzzy trigger is **user error** - only flickers, never sticks. (A "freeze the fire-time
    condition" latch could make even that stable, but it's extra state + odd semantics; skipped.)
  - **Corollary** (ordinary overlay semantics, no special rule): a layer overrides only the inputs it
    **names**, so an input the active layer is silent about keeps resolving to the base binding for the
    whole hold.
  - **Layer/mode changes take effect *next* tick.** Layer state is fixed at the **start** of a tick;
    all bindings resolve against it, and `HoldLayer`/`Add`/`RemoveLayer`/`ChangeActionSet` collected
    this tick update the state for the **next**. This is what keeps the pass **acyclic** - a binding
    this tick depends only on *last* tick's layer state, never on this tick's behavior outputs. It also
    means a layer can **emulate "gate on a virtual"** (which a gater can't): e.g. base gyro
    always-disabled, a layer gyro always-enabled, `HoldLayer(that layer)` on a **soft-pull** => "gyro on
    while soft-pull held", stable, at a **one-tick** on/off latency (~4 ms @ 250 Hz - imperceptible).
    So gater vs layer+`HoldLayer` isn't a capability gap, just a trade: gater = same-tick but
    frame-local only; layer = any trigger, one tick deferred (the tick is the price of breaking the
    cycle).
  - **Golden tests (pure `step()`):** (i) base binding = out-key **+** `HoldLayer(L)` where `L` re-binds
    that same button => stable, no stuck output (a brief base-key blip, then the layer's key held); (ii)
    same but `L` does **not** name the button => the base out-key persists for the whole hold.
- **Persistent stack/set ops dedup per node (fix self-toggle strobing).** `AddLayer`/`RemoveLayer`/
  `ChangeActionSet` are **persistent** state mutations, and a self-referential toggle re-creates the
  oscillation the `HoldLayer` latch defeats - one tick worse: base `AddLayer(L)` + the layer's own
  same-button `RemoveLayer(L)` (or a one-button action-set cycle set0->set1->set2->...). While the button is
  held, a `Regular` re-applies its op **every tick**, the op flips the node's **own winning binding**
  (base<->layer / set<->set), and the binding change **resets the activator** (the no-long-press-bleed rule)
  into a *phantom press* - so the two opposing ops alternate every tick and the landing state is the
  **parity of held-ticks** ("works intermittently / needs 2-5 presses"). This bites **every** activator,
  not just `Regular`: even `Start` re-detects a press edge after the reset. Only `Release` escapes,
  because its edge is button-**up**, when no phantom press can form (so a `Release`-driven toggle already
  worked pre-fix).
  - **Fix - fire once per node engagement.** A persistent op applies only if its **trigger node isn't
    already armed** this press; then the node arms until it releases. This is the **same node-latch as
    `HoldLayer`** - `armed_nodes`, a `Map<NodeKey, NodeHeld>` rebuilt each tick by "keep while
    `node.held(frame)`" - so it survives the activator reset (it lives on the mapper, keyed by the
    **physical node**, *not* in the per-source activator slots) and disarms exactly when the node
    releases. Node-keyed, so it **survives a `ChangeActionSet`** (a set swap doesn't change the node),
    which is what makes the set-cycle advance **once per press** rather than free-run.
  - **Same machinery, opposite lifetime, as `HoldLayer` - one place, no fire-time special case.** Both
    are node-latches anchored to the **raw frame**, not to whether a command re-fired (the property that
    kills the oscillation). They differ only in what a live entry *means*: a `HoldLayer` entry = "this
    layer is held" (**sustains**, drops on release); an `armed_nodes` entry = "this node already made its
    one persistent change this press" (**suppresses**, also drops on release). So `LayerOps` carries the
    trigger node on **every** op (`holds`/`adds`/`removes`/`set_change`), `eval_commands`/`apply_action`
    stay pure "what fired" reporters that just queue the raw ops (the persistent ones only on the
    command's **rising edge** - see the edge-fire bullet below), and **all** node-latch logic
    (both latches + the dedup + the set swap) lives together in **`reconcile_layer_ops`** (renamed from
    `reconcile_layers`, since it now folds layers, the set change, and the dedup). *(Rejected: doing the
    dedup at fire-time inside `eval_commands`, which needed a fourth `Sinks` member - asymmetric with
    `HoldLayer` and put state logic on the hot per-command path.)*
  - **Rule: one persistent layer/set change per button press**, whatever the activator. Ops fired
    **together in one tick** - one command's `AddLayer(X)+RemoveLayer(Y)`, or two commands firing the
    same tick - all apply, because the armed check reads the **pre-tick** snapshot (a single fire lands
    **atomically**); but a **second** persistent op *later in the same press* (e.g. `long(300)->AddLayer`
    then `long(500)->ChangeActionSet` on one button) is **suppressed**. `Regular{interruptible}`+`Long` is
    unaffected - tap vs hold are mutually exclusive, so only one ever fires per press. This one-per-press
    ceiling is the deliberate cost of **per-node** keying; finer keying would reintroduce the strobe.
    (`set_change` additionally **wins** over a co-fired `Add`/`Remove` in the same command - the full
    swap clears the per-set stacks, so the layer add is dropped.)
  - **Persistent ops are edge-fired, not level-fired (2026-08-24 - the fix that makes `armed_nodes`
    actually correct).** `armed_nodes` disarms the tick a node releases, but an interruptible-`Regular`
    **tap holds its output for `TAP_MS` (~40 ms) *past* release**. So queuing the op *every tick a
    command is high* (level) re-applied it during that post-release tail - **after** the node had
    disarmed - re-adding a just-removed layer: exactly the "needs 2-5 presses" feel, now via the tap
    rather than the strobe (the self-removing-layer / cp2077 shape: base `Regular`->`AddLayer` +
    `Long`->`HoldLayer`, layer `Regular`->`RemoveLayer`). Fix: `apply_action` fires the three persistent
    ops **only on the command's rising edge** (`CmdState::prev_out`, unified with the command-haptic
    edge), so each lands **once**, at the commit tick, where the node is still armed -> suppressed -> no
    re-apply. `HoldLayer` stays level-fired (node-latched via `held_layers`, not `armed_nodes`, so
    re-queuing is idempotent). *(The original `armed_nodes` commit's message said "edge events" but the
    code level-fired + deduped; this is the fix that makes the description true. Verified against the
    self-removing-layer regression test, which fails under level-firing.)*
  - **Same frame-local robustness caveat as the latch (above).** The dedup's `held`-re-derivation is
    robust for **frame-local** trigger nodes - the criterion is *"re-derivable from the raw frame,"* **not
    "physical vs virtual"** - so **physical buttons, group members, and trigger soft-pull / full-pull**
    dedup exactly, while a **fully pipeline-derived virtual** (outer-ring, `DirectionalPad` direction) has
    `NodeHeld::held()==false` and so **can't stay armed** -> a self-toggle off such a node can **oscillate**
    (never *sticks* - reconciliation), the identical user-error corner as a virtual-triggered `HoldLayer`.
    A soft-pull re-bound at a **different threshold** across layers has the same fuzziness (base 0.5 /
    layer 0.9 / analog 0.6 -> the captured threshold disagrees with the active condition).
  - **Golden tests (pure `step()` / `tick()`):** same-button `AddLayer`/`RemoveLayer` toggle stable at
    hold-lengths 1/50/37 ticks (not parity-dependent); one-button 3-set cycle advances once per press
    (0->1->2->0); `Regular{interruptible}`+`Long` taps one op / holds the other, each once; a single
    command's `Add(X)+Remove(Y)` lands atomically; the dedup is per-node (two buttons toggle
    independently, held together); two hold-takers on one node = one-per-press; and the layer<->set mix
    (tap adds a layer / hold changes set; `set_change` overrides a co-fired `Add`; a set-changing node
    doesn't also fire the new set's op until released).
  - **Command-haptic pairing + `HoldLayer` folded into the uniform dedup (2026-08-30).** Two changes,
    both moving OpSet effects to be decided in `reconcile_layer_ops`, where the real layer state lives:
    - **The command click is now paired to the effect that LANDS, not the raw output level.**
      `command_effect(cmd)` (`mapper/command.rs`) classifies a command's effect into three mutually-
      exclusive routes, and the click follows the effect's own landing edge: **(a) Level** - any output
      leaf / bare `None` / a mix -> clicks **inline** on the command's output edges (unchanged). **(b)
      PersistentOpSet** - *only* `Add`/`Remove`/`ChangeActionSet` -> click **deferred** into
      `LayerOps::pending_haptics`, fired iff the op **changed state** (its node lands in an `applied`
      set: past dedup, `set_change`-wins **and** a real `insert`/`remove` returning `true`), so a
      deduped-away, superseded, or no-op op is silent; a persistent op has **no release edge**
      (`falling` forced off, so `OnPress`/`Both` click once on landing, `OnRelease` never). **(c) Hold**
      - *only* `HoldLayer` -> click rides the layer's **`held_layers` lifecycle**: engage when the layer
      enters `held_layers` (`OnPress`/`Both`), disengage when it leaves (`OnRelease`/`Both`), the haptic
      stored on the `HeldLayer` entry so the **disengage click survives the self-shadow** that hides the
      command on the release tick. *Fixes a no-op `RemoveLayer` buzzing on a held layer, and a `Both`
      hold never clicking on release.*
    - **`HoldLayer` is now a uniform OpSet for dedup - this SUPERSEDES the "`HoldLayer` stays
      level-fired, not `armed_nodes`" / "same machinery, opposite lifetime" framing in the bullets
      above.** Engaging a hold **arms the node** (it joins the requesting arm set beside
      `Add`/`Remove`/`set_change`), and a **new engagement is armed-gated** (`NodeHeld::may_fire`)
      exactly like the other OpSets - a hold can't engage on a node that already fired an OpSet this
      press. What is *not* a dedup decision is an already-engaged hold **continuing** to hold: that stays
      the `held_layers` node-latch (keep-while-`node.held`), the definition of a hold, and it's how an
      engaged hold rides out its own self-shadowed no-op `RemoveLayer` and stays immune to another
      button's ops. So the **"one persistent change per press" rule now covers `HoldLayer` too**. *Fixes
      a hold-to-remove re-adding the layer via a re-firing base `Long` - a real re-hold, not just a stray
      click (the cp2077 RG2 double-click).* The held-layers rebuild + its lifecycle clicks now live in
      their own `reconcile_held_layers`.
    - **Golden tests added:** persistent-op click fires once on landing / ignores release; self-toggle
      clicks once per real change; a no-op removal / a deduped op / a `set_change`-superseded add stays
      silent; a `Both` hold clicks on engage **and** release through the self-shadow; a hold-after-remove
      doesn't re-engage; engaging a hold arms the node (a shadowed op is deduped); a held layer isn't
      torn down by another button's op; the hold lifecycle click follows its edge config; a
      `ChangeActionSet` clicks once on the swap.
- **Layers stack; precedence is *declared order*, not activation order.** More than one layer can be
  active at once - `AddLayer`/`RemoveLayer` manage **persistent** stack entries, and multiple held
  `HoldLayer`s add **held** entries; both feed one active-layer stack over the base action set.
  **Overlap resolution:** for each input, the highest-precedence *active* layer that **names** it wins;
  a layer that doesn't name the input falls through to the next active layer down, then to the base set
  - so non-overlapping layers compose freely (layer A rebinds the stick, layer B the gyro, both live).
  **Precedence = each layer's position in `ActionSet.layers`** (fixed **declared order**), *not* the
  order it was activated: declared order is **deterministic** (the same active *set* always resolves
  the same way) and checkable/configurable, whereas activation order is **path-dependent** ("hold A then
  B" != "B then A" when they overlap). The config's `Vec<Layer>` already carries this order; the UI may
  offer re-ordering.
- **Relative outputs accumulate a sub-pixel remainder (don't reconcile).** Mouse-move and
  scroll are **deltas/rates**, not levels - the output-reconciliation above is for levels
  (keys/buttons/abs axes); relative motion is *accumulated* instead. Converting fractional
  per-tick motion (pad/gyro -> pixels/ticks, after sensitivity/accel/curve) to integer device
  units **must carry the fraction forward** in a float accumulator: emit `trunc()`, keep the
  remainder for next tick. Truncating each tick silently drops sub-pixel motion and makes
  slow/fine movement feel dead - the accumulator restores it (validated in the Phase B bridge;
  a big feel improvement). Applies to every relative output (mouse move, scroll). **Note (the
  accumulator is not anti-drift):** it faithfully *integrates*; it does not filter. Gordon's
  gyro has a **small DC bias** (stationary raw isn't zero-mean, 1.9) that integrates into a
  **slow but real cursor drift** (confirmed by holding the controller still with gyro active
  for an extended time - trigger-gating hides this in normal short-burst play but is *not* the
  fix). The fix is a **small radial deadzone** on the per-frame gyro delta: **~0.1 px kills the
  drift completely** (the bias is ~0.003 px/frame, far below it) while sitting well under real
  aiming motion, so fine/slow movement is untouched. So gyro **wants a small, configurable
  deadzone** (radial, on the delta magnitude - cleaner than per-axis) and/or **bias
  calibration** (measure the resting offset, subtract it): a needed input-process knob (3),
  just a very small one.
- **Gyro processing - validated wants for the engine (from real play).** The Phase B bridge's
  gyro is deliberately crude (**local space**: raw yaw->horizontal, pitch->vertical, linear
  sens, trigger-gated) and already "100% playable" in Cyberpunk. To reach parity with a good
  daily config, the engine's gyro/`InputProcess` (3) should add: (a) **player-space gyro** -
  use the accel gravity vector to combine yaw+roll into a stable "turn = horizontal" mapping
  regardless of how the controller is tilted/held (vs. the bridge's raw local space), the
  biggest feel gap; (b) a **light 1-euro (One Euro) smoothing filter** on gyro/pad (the 4
  filter note - confirmed subtle-but-desirable in practice); (c) **configurable response
  curves** on gyro & pad. None are blockers (crude version is playable) - they're the top
  polish items.
- **Haptic / rumble back-channel (engine-owned, must be configurable).** Rumble flows the
  other way: a game rumbles the virtual gamepad -> `virt-out` surfaces it (two `u16`
  magnitudes, heavy/low-freq + light/high-freq - the only thing evdev FF / XInput give; **no
  frequency or waveform**) -> the engine routes it to the real device's haptics via
  `steam-hid`. On **Gordon** the "motors" are the two **trackpad actuators** (1.9), so the
  engine *synthesizes* the buzz from the bare intensity: it chooses the **frequency**, maps
  intensity -> **duty-cycle amplitude** (non-linear/saturating - wants a **response curve**),
  and applies an overall **strength**. These must be **user-configurable** (validated in the
  Phase B bridge as fixed consts - ~60 Hz, 50% strength, linear): expose **Hz, response
  curve, and strength** in the config model (3). **Scope (leaning, undecided): per-profile**
  haptic settings **plus a single global master strength %** - so profiles tune texture while
  one knob scales everything. (Device-dependent: the Deck has real rumble motors + trackpad
  haptics, so its mapping differs; keep the config expressive enough for both.)

**Input/output seams = the network role-swap points.** The step's input and output are
seams, not hard-wired to the local adapters. The manager feeds the step from `steam-hid`
**or** a network receiver, and routes the step's output to `virt-out` **or** a network
sender. That is exactly what lets a *single* engine implementation run either 6 role -
source (local device -> net) or sink (net -> local output) - with the role chosen at
runtime, not by a separate build. (This is also why even the sink side links `steam-hid`:
same artifact, it may drive a local device a moment later - see 6.)

**Control API (transport-agnostic, deliberately narrow).** The engine exposes **one
in-process Rust control API**. *(Concrete method surface + lifecycle are refined and made
authoritative in 4.1 below - `set_input`/`set_output`, `apply`/`set_chords`/`set_device_config`,
`start`/`stop`/`shutdown`; `open()`/`stop(self)` here are the earlier sketch.)* That same API is what a daemon would wrap behind a socket
(with an optional small CLI config-sender speaking that socket), and what the UI calls
directly when embedded. **Embedded vs daemon = the same command set, different transport**
- this is precisely what keeps that deployment choice deferrable without a rewrite. The
API has two families plus lifecycle, and **no concept of profiles**:
- **Config:** `apply/swap config` - hand the engine a (compiled) config to run. Config
  authoring, persistence, profiles, and *which* config is active all live in the **UI**
  (3); "switch profile" is just the UI calling `apply config` with a different one, so
  **hot-reload rides the same path** (an atomic swap on the running engine - define what
  resets vs. persists across it).
  **Refinement (3 part 1, mechanics pending this section):** the engine can hold **two**
  live profile Programs (**active + fallback**) plus the two above-profile beings - a
  `DeviceConfig` (per-device rumble shaping, LED/idle; **reader-side**) and an
  `Option<Chords>` slot (top-level chords) - and switch main<->fallback **itself** via a
  profile-independent top-level **chord** - so not every switch is a UI re-apply. Apply-API
  therefore tags each Program with a **role** (main/fallback) and takes device-config + chords
  separately; it may expose loaded-program **name/metadata** back (for UI restart/status). Still
  **no profile/file concept** in the engine - just program(s) + device-config + chords.
  *(Post-2026-08-19-split naming: the earlier single `GlobalConfig` blob is gone.)*
  **Chord evaluation (decided):** global chords are evaluated **first**, before any profile
  binding, and an activated chord **consumes** its trigger buttons (they are *not* forwarded
  to the active Program). So chords may **shadow** profile bindings, **never the reverse** -
  fine in practice, since chords sit on keys profiles don't use (Steam button, grips). Switch
  semantics are **hold-vs-toggle, configurable per chord** (3 Tier B; toggle the safe
  default). The whole point of holding this in the engine: the escape hatch works **UI-down /
  daemon-only**, instantly, with no IPC.
- **Devices:** enumerate/report available devices, initialize/open, manage lifecycle, and
  **inform the UI what is available** (so the UI can bind configs to real devices and show
  status). This is the engine surfacing `steam-hid`'s device world upward.
- **Lifecycle/observability:** `start/stop`, `status`, and a small live **monitor**
  channel (low-rate, for the UI to show current state) - distinct from the 6 high-rate
  state stream.

**Concurrency model (decided): sync, no async.** Well-defined **thread workers** where
needed - the natural shape is **thread-per-device (blocking `steam-hid` reads) feeding a
central mapping loop over channels**. The Neptune keep-alive rides *inside* each reader thread
(steam-hid owns no threads of its own; 1.6). No async runtime is imposed; this is **not** a distributed system, so an executor
would be overhead. (If 6 networking ever makes multiplexing painful, revisit *then* - the
local engine does not need it.)

**Engine is a library (decided).** `engine` is always a **library** crate. What's
undecided is *where that library runs* - the process/deployment model (decide after
`steam-hid`, `virt-out`, and initial `config` are done):
- **Embedded:** the engine library is linked into the UI process (UI = config + engine),
  optionally with a thin CLI runner.
- **Daemon:** the engine library is hosted in a separate daemon process; the UI and/or a
  CLI drive it over a **control-plane** socket (UI = config only). Note this daemon
  control seam (UI -> daemon: push config, read state, a small live monitor - low-rate,
  reliable, request/response) is a **different** channel from the 6 network seam (a
  high-rate, unreliable, latest-wins `ControllerState` stream). Do not conflate them.

Both put the *same* library to work - only the process boundary differs. The crate
**name** (`engine`, `core`, `mapper`, ...) and the final **dependency graph** are deferred
to that decision.

### 4.1 Concrete engine design (agreed 2026-07-28 - BUILT, see 4.2)

Locked in a pre-implementation design pass, and since **implemented** (4.2 checklist S0-S10,
HW-validated). **This subsection is authoritative for the concrete surface**; the notes in 4
above are the reasoning behind it. Where the built code refined a detail, the change is noted
inline.

**Crate & IR placement (decision A).** `crates/engine` (edition 2024, dual-license,
`publish=false`). Deps: `steam-hid`, `virt-out`, `config`, `vocab`, `crossbeam-channel`,
`log`; the test binary adds `ctrlc` + `env_logger`. **`Program` (the runtime IR) and
`compile(&ConfigDoc) -> Result<Program, Vec<Diagnostic>>` live in `engine`, not `config`**
- the IR is the engine's *runtime contract* (resolved indices, runtime-ready settings),
co-designed with the Mapper; `config` stays pure authoring data. (Supersedes the earlier
3 "Program lives in config": it read as config's third pipeline layer when it was still
"flattened config"; once it became the runtime's contract it moved to its consumer.)

**Control API - one owned `Engine` handle** (sync; owns the worker threads). This is the
exact API a daemon later wraps behind a socket, and the UI calls directly when embedded.

```
Engine::new(opts) -> Engine        // IDLE: control channel + in-memory config only; acquires NO hardware
  // staging (idle) - set_input/set_output are staged-only; apply/set_chords also work live (running):
  .set_input(Input)                // Input  = Local(DeviceSelect) | Network(bind_addr);   applied at next start()
  .set_output(Output)              // Output = Local | Network(connect_addr); default Local; applied at next start()
  .apply(program, role)            // Role   = Main | Fallback; callable idle (staged) OR running (live hot-swap)
  .set_chords(Option<Chords>)      // top-level chords; callable idle (staged) OR running (live hot-swap)
  .set_device_config(DeviceConfig) // reader-side device settings; live hot-swap when running (dedicated device_config_tx, machine-local - NOT control_tx), else staged
  .start() -> Result<()>           // acquire HW: open device (+reader thread), create Sink, run the mapping loop
  .stop()  -> Result<()>           // halt loop, release HW (device->lizard restored, virtual pad unplugged); config RETAINED; start() resumes
  .devices() -> Vec<DeviceStatus>  // enumerate; works ANY time (idle or running) - just steam-hid::Manager
  .status() -> StatusInfo          // ONE atomic snapshot { state, input, output, bound: Option<DeviceId>, controller: Option<bool>, main/fallback: Option<String>, active: Option<Role>, device_config: DeviceConfig, chords: Option<Chords> }
  .monitor() -> Receiver<MonitorEvent>   // low-rate live state for a UI (distinct from the 6 high-rate stream)
  .shutdown(self)                  // ensure stopped, full teardown, consume the handle
```

- **Lifecycle rationale (daemon-friendly):** `new()` never touches hardware -> construction
  always succeeds and HW errors surface at `start()`. `stop()` releases HW but keeps config
  in memory -> a daemon pauses/resumes without losing configuration and the process outlives
  `stop()`. `shutdown()` is the explicit full teardown.
- **Mutability rules:** `apply`/`set_chords`/`set_device_config` = **live hot-swap** while running
  (also stageable while idle) - `set_device_config` pushes on a dedicated reader channel
  (`Runtime::device_config_tx`), machine-local, never `control_tx`/the 6 wire; a no-op on the server
  role (no local reader). `set_input`/`set_output` = HW bindings, **staged only**, take effect
  at the **next `start()`**; between a `start`/`stop` pair the I/O set is **fixed** (no mid-run
  device/transport swap). `devices()` always available.
- **No profile/file concept in the engine** - just program(s) + device-config + chords, each
  program tagged Main/Fallback. **Chords evaluated first**, **consume** their trigger buttons (masked
  out of the frame before the Mapper), switch Main<->Fallback per-chord hold|toggle. First cut
  ships **SwitchFallback**; **CommandExecute chords now IMPLEMENTED** (was decision B) - run a headless
  external program on the engage edge, spawned on a detached thread, output logged.

**Internal architecture (sync, no async - the 4 model made concrete).**
- **Reader thread, one per open device.** Owns the `steam-hid::Device` (Send, not Sync) -> **all
  device writes happen here**: the blocking `read()` loop, the Neptune keep-alive, re-applying
  device config on `Connected`, and **haptics** (both game-rumble and command-activation). Pushes
  each `Report` frame onto a channel to the central loop; receives `HapticReq`s back over a
  per-device channel.
- **Central mapping loop.** Owns the `virt-out::Sink` and the runtime (a `Mapper` per device,
  the Main/Fallback `Program`s, chords). `crossbeam-channel::select!` over {frame channel,
  control channel, fallback tick}. Gordon/Neptune stream ~250 Hz continuously while connected, so
  it's **frame-driven**; the tick is insurance for rumble/turbo if a stream stalls. Each iteration
  also `Sink::poll_rumble()` -> routes game rumble to the reader thread.
- **Clock injection.** The loop stamps each frame with a monotonic `Tick` (Duration since start)
  at dequeue and passes it into the Mapper. **Golden tests feed synthetic `(input, Tick)` pairs**
  -> no threads, no hardware.

**The Mapper (pure, golden-testable core) - retained state:** `active_set`; the **layer stack**
(persistent Add/Remove entries + held HoldLayer entries); the **prev logical frame** (raw + virtual
levels -> digital edges); the **activator table** (per resolved-binding timing/toggle/turbo/double
state); **applied levels** (last-emitted keys/buttons/abs-axes, for reconciliation); **relative
accumulators** (mouse + scroll sub-pixel remainders); **gyro state** (bias/deadzone/integration).

**Per-tick algorithm - one fixed ACYCLIC pass** (this is *how* the 4 rules hold):
1. Layer state is **frozen** from last tick (`active_set`, stack) - every binding this tick resolves
   against it. (This is what makes the pass acyclic; layer/set changes apply NEXT tick.)
2. Resolve **activation gaters from the raw frame**, up front (physical-only -> all readable before
   any behavior runs).
3. Resolve each input's **winning binding**: walk active layers **highest-declared-order first**,
   first layer that *names* the input wins, else fall through to base.
4. Run **behaviors** (pad/stick/trigger/gyro) in any order -> abs axes, relative nudges, and **virtual
   button levels** (soft-pull, dpad dirs, outer-ring).
5. Run **all commands** on button-like nodes - physical **and** the virtual buttons from step 4 -
   evaluating activators against digital edges (`cur & !prev`) + `now` + retained timers -> desired
   key/button levels, toggle latches, turbo fires, queued layer/set actions, haptic requests.
6. Collect layer/set changes fired this tick -> update stack/active_set for the **next** tick; a
   HoldLayer entry is **latched to its firing node's held-state** (survives that layer re-binding the
   node).
7. **Reconcile** desired levels vs `applied` -> emit only diffs (no stuck outputs across layer changes).
   **Accumulate** relative outputs with remainder -> emit `trunc()`, keep the fraction.
8. `prev = input`.

**Activator state keying:** by the **resolved binding's stable id** `(SetId, source, LayerId-or-base,
command-idx)`; when an input's winning binding changes between ticks (a layer overlays it), its
activator/toggle state **resets** - a half-completed long-press can't bleed into a different binding.

**`ControllerState` -> logical input** is the **engine's** job: a fixed table per `DeviceKind`
(the Gordon multiplex etc. is already resolved in `steam-hid`; here it's just which bit/axis is which
`InputSource`). `LogicalFrame` = per-`InputSource` resolved values; behaviors read raw values,
commands read digital levels/edges. **Compile stays device-independent** (config is); the device table
is applied at runtime and absent inputs are ignored.

**Program IR + `compile()`.**
```
Program { meta, sets: Vec<CompiledSet>, default_set: SetId }
CompiledSet   { name, base: SourceMap<CompiledBinding>, layers: Vec<CompiledLayer> }
CompiledLayer { name, bindings: SourceMap<CompiledBinding> }   // Vec index == declared-order precedence
```
`compile()`: run `validate()` (bail on any `Error`) -> resolve `ActionSetRef`/`LayerRef` names to
`SetId`/`LayerId` (**the `Vec<Layer>` index *is* the precedence**, for free) -> flatten base + layer
bindings into per-source lookups -> precompute settings into runtime-ready floats. The
`InputSource -> ControllerState` resolution is **not** in compile (it's the device-specific runtime
table).

**Networking seam (documented; implemented later - all agreed).**
- **Seam locked now, only `Local` built.** `set_input(Local|Network)`, `set_output(Local|Network)`;
  the `Network` variants return "unimplemented" for now. Locking the method *shape* costs nothing and
  matches 6's "seams = role-swap points."
- **Transmission = REPORT-side (raw `ControllerState` on the wire); the Mapper runs on the SINK.**
  Rationale: a `ControllerState` is an idempotent **snapshot** (drop-tolerant, latest-wins, `seq`
  drops stale) -> safe over unreliable UDP and self-heals on the next frame; `OutputEvent`s are
  stateful **diffs** that a dropped packet turns into a stuck output, forcing a reliable channel whose
  retransmit/head-of-line latency hurts twitch input. (This settles the earlier 6 "emit vs report"
  openness -> **report**.)
- **Server/client split (agreed):** the **output/game side (PC) is the SERVER** - started once with a
  bind address, headless, **survives client disconnect/reconnect**, never needs reconfiguring; the
  **controller side (Deck) is the CLIENT** - user-facing, owns/authors config + profiles, **initiates**
  the connection out to the server. **Who-binds-vs-who-connects is independent of who-maps** - which is
  what lets the Mapper sit on the server while all configuration stays on the client you hold.
- **Config stays owned on the client**, shipped to the server over the **reliable control channel** at
  connect; frames stream over the **unreliable UDP data-plane**; **rumble** flows back server->client
  (light reliability). So the Deck's networked role is a **thin forwarder** (device -> net, no local
  Mapper); the PC runs a device-less engine = net-receiver -> Mapper -> virt-out (still links
  `steam-hid`, 6: one artifact, role chosen at runtime).

**Haptics - v1 model (agreed; Gordon path, provisional).**
- **Two sources, both -> the reader thread as `HapticReq`:** (1) game rumble (`Sink::poll_rumble` ->
  strong/weak magnitudes), (2) command-activation `Haptics` (config Round C, on the action's press/
  release edges).
- **From the HID POV (Gordon, 1.9):** each haptic is an independent fire-and-forget `0x8f` pulse to
  **one physical actuator** (left pad / right pad). **Different actuators are independent** (both can
  buzz at once). **Same actuator can't play two waveforms** -> **latest-wins** (a new pulse preempts).
  No firmware-side mixing or priority - any arbitration is ours.
- **Sustained rumble = a pulse train** the reader thread **re-issues at the configured Hz** (per 1.9);
  the reader thread runs this small synthesizer, holding the current level per pad.
- **v1 arbitration = NONE.** Command pulses just fire; on a **shared** actuator a one-shot pulse
  briefly interrupts the rumble train (resumes next period), on the **opposite** actuator fully
  independent. Command haptics land on the input's own side, rumble is usually both -> you feel a short
  "tick" over the rumble. Acceptable; tune later. Deck (real motors `0xea` + pad haptics) = a different,
  unverified path -> deferred.
- **First cut wires only the game->device rumble path** (proven in the Phase B bridge);
  **command-activation haptics deferred** (decision C) but the `HapticReq` seam is in from day one
  (`Mapper::tick` emits `haptics` alongside `out`).

**Gyro/pad polish ordering (decision D).** First pass = **crude local-space** linear gyro + the small
**radial gyro deadzone** (~0.1 px - kills the DC-bias drift, 4/1.9). Next (soon) = **1-Euro (One Euro)
filter** across gyro/pad/stick relative motion (deferring it defers it **everywhere**, incl. all three
mouse behaviors). Later = **player-space** gyro, then **response curves**.

**First-cut scope (test binary, `deckhand-run` or similar).** Parse `--wired/--dongle`, a profile
`.ron` (+ optional `--fallback`, `--chords`, `--devcfg`), `env_logger`. `Engine::new` -> `compile()` the RON(s) ->
`apply()` -> `set_input(Local(device))` -> `start()` -> run until Ctrl-C (clean Drop restores lizard,
unplugs the pad - the bridge pattern), then `shutdown()`. Essentially the Phase B bridge loop, but
driven by a real compiled `Program`. **Build order:** `Program` + `compile()` (reusing `validate()`)
-> `Mapper` + golden tests -> manager shell (threads/channels) + test binary. Manager includes
Main/Fallback + the **SwitchFallback** chord from the start. CommandExecute chords, command-activation
haptics, and gyro spaces are now all IMPLEMENTED (were deferred here).

### 4.2 Implementation steps (checklist - COMPLETE)

Ordered so each box is **one reviewable, committable, green** step; later boxes depend on
earlier ones. Progress marker for future sessions: **check the box when its commit lands.**
**Progress: S0-S10 DONE - the 4.2 checklist is complete (commits `c701f5b` scaffold ->
`f6d3842` IR -> `58273cb` compile -> `1775d90` logical -> `0f48ec6` reconcile -> `a4af743` Mapper
skeleton -> `e35d143` behaviors S6a -> `5d9bd3c` behaviors S6b -> `b816b8e` activators S7a ->
`c9d0569` activators S7b -> `caf7b45` layer/set actions S8 -> `75a5258` chords/switch S9a ->
`3fca718` runtime shell S9b -> `0b6f680` Engine handle + deckhand-run S10). **HW-VALIDATED** on
the Gordon dongle (mapping/behaviors/activators, HoldLayer mode-shift, main<->fallback chord,
`SourceBinding::None`, game->pad rumble - confirmed in a real game + jstest); a few sign fixes
landed during validation (cursor/stick Y +up->+down, gyro yaw->-X, scroll coarser than cursor).
Remaining: extended sensitivity tuning, then the deferred list below.**
Suggested engine module layout: `program.rs` (IR), `compile.rs`, `logical.rs`
(`ControllerState`->`LogicalFrame`), `mapper/` (`mod.rs` state+`tick`, `resolve.rs`,
`behavior.rs`, `command.rs`, `reconcile.rs`), `runtime.rs` (threads/channels), `handle.rs`
(`Engine`), `error.rs`.

- [x] **S0 - Scaffold `crates/engine`.** Cargo.toml (edition 2024, dual-license,
  `publish=false`; deps `config`/`vocab`/`steam-hid`/`virt-out`/`crossbeam-channel`/`log`/
  `thiserror`), add to workspace members, `lib.rs` with module stubs + `Error`/`Result`.
  Builds empty-green.
- [x] **S1 - `Program` IR types.** `SetId`/`LayerId` newtypes, `Program`/`CompiledSet`/
  `CompiledLayer`/`CompiledBinding`/`CompiledCommand`/`CompiledAction` (refs resolved to
  ids), `SourceMap` (per-`InputSource` lookup), `ProgramMeta{name, role}`. Derives
  Clone/Debug/PartialEq; **no Copy**. Types only, no logic.
- [x] **S2 - `compile(&ConfigDoc) -> Result<Program, Vec<Diagnostic>>`.** Reuse
  `ConfigDoc::validate()` (bail on any `Error`); resolve `ActionSetRef`/`LayerRef` ->
  `SetId`/`LayerId` (Vec index = declared-order precedence); flatten base + layer bindings;
  precompute settings to runtime floats. Unit tests over a bridge-like `ConfigDoc`
  (round-trips ids; rejects dangling refs).
- [x] **S3 - `LogicalFrame` + `ControllerState`->logical (per `DeviceKind`).** Fixed table
  mapping each `InputSource` to its `ControllerState` bit/axis (Gordon multiplex already
  resolved upstream); absent inputs ignored. Unit test Gordon + Neptune samples.
- [x] **S4 - Reconciliation + relative accumulators.** `AppliedLevels` (keys/buttons/abs
  axes) -> emit only diffs as `OutputEvent`; `RelAccum` (mouse+scroll) carrying sub-pixel
  remainder -> emit `trunc()`. Pure, unit-tested in isolation (no Mapper yet).
- [x] **S5 - `Mapper` skeleton + binding resolution.** Retained state struct; `tick(&mut
  self, &LogicalFrame, Tick, &Program, &mut Vec<OutputEvent>, &mut Vec<HapticReq>)`; the
  frozen-layer-state read + winning-binding resolution (declared-order stack walk, else
  base). Wire buttons->level outputs through S4. Golden test: simple button map. **Done:**
  `Tick`/`HapticReq` seam types added; only `Button` + `Regular` wired (held-while-held);
  layer-precedence resolution in place (poked in a test) but the stack stays empty until S8.
- [x] **S6 - Behaviors (crude first).** `Trigger` (soft-pull virtual + analog axis),
  `Joystick`/`DirectionalPad` (shared), `AsMouse`/`JoystickMouse` (relative via S4),
  `GyroToMouse` (local-space linear + radial deadzone), `ButtonPad`. Emit abs axes/relative
  nudges/virtual-button levels; gaters resolved from the raw frame up front. Golden tests
  per behavior. **Done in two commits:** *S6a* (`mapper/behavior.rs` + shared
  `mapper/command.rs`) - the non-relative producers (ButtonPad/Joystick/DirectionalPad/Trigger),
  pure functions of the current frame; pad-as-joystick/dpad gated on touch. *S6b* - the relative
  mouse behaviors via a per-tick `behavior::Ctx {cur, prev, dt}` feeding `RelAccum` (`Mapper`
  gained `last_tick` for `dt`, and now reads `prev`). Virtual buttons run through `eval_commands`
  (still `Regular`-only until S7). Applied: rotation/sensitivity/invert + gyro radial deadzone.
  **Acceleration later implemented (proper, not the bridge stopgap):** `out *= 1 + speed*factor` keyed
  off a *proper instantaneous speed* - AsMouse velocity (`|delta|/dt`), JoystickMouse deflection,
  GyroToMouse angular-velocity - so it's poll-rate-independent (the bridge's per-frame-delta version is
  rate-tied). **1-Euro smoothing later implemented** (`mapper/smooth.rs`) for AsMouse (pad velocity) +
  GyroToMouse (angular velocity), on the rate signal (frame-rate-independent); `JoystickMouse` smoothing
  dropped (a stick is already smooth). **Player-space gyro since implemented** (gyro `space` =
  Yaw/Roll/YawRoll/PlayerSpace, HW-verified, with a gravity estimate). **Still deferred** (decision D):
  response curve on the mouse behaviors; fusion-based gravity + World/Laser gyro spaces. Output
  gains are starting points, HW-tuned. **Axis limit added (2026-08-21):** the `Axis` setting (3) is
  masked **in the output frame, early**, so a limited behavior is a genuine 1-D control rather than a
  post-hoc output null. Ordering (learned the hard way): the mask must precede any coupling stage -
  the **radial deadzone/curve** on `Joystick`/`JoystickMouse` (else a large discarded-axis deflection
  carries a *sub-deadzone* kept-axis value through the 2D magnitude gate, under-scaled), and the **1-Euro
  filter + acceleration** on `AsMouse`/`GyroToMouse` (else the discarded axis' speed inflates the kept
  axis' accel; the 1-Euro filter is per-axis so the mask's position relative to it is neutral, but it's
  placed before for a uniform 1-D chain). While here, **`eval_as_mouse` and `eval_gyro_to_mouse` were
  unified to one op order** - rotate -> mask -> 1-Euro smooth -> accelerate -> scale/invert -> (gyro) anti-drift
  gate -> emit - and the single-use `process_relative` helper was **inlined and dropped** (its bundled
  rotate-at-the-end was what had forced AsMouse to filter *before* rotating, out of step with gyro).
  Kept + commented as intentional: AsMouse's base motion stays **positional** (dt-independent - a
  same-millisecond tick still applies the finger displacement), gyro's is **rate*dt**; hence AsMouse's
  `/dt ... xdt` wrap around the filter vs gyro filtering the rate directly.
- [x] **S7 - Activators/commands.** `Regular` first, then `Start`/`Long`/`Double`/`Release`,
  plus `Toggle`/`Turbo`/`Interruptible`; activator table keyed by resolved-binding id, reset
  on winning-binding change. Golden tests incl. interruptible short-vs-long. **Done in two
  commits:** *S7a* - `mapper/activator.rs` (the `Activators` table keyed by source->`BindingKey`
  {Base|Layer}, reset on change; one `SlotState` per button-like node, lazily grown) + the five
  activator types timed off the injected `Tick` (`Start`/`Release` = taps of `TAP_MS`; `Long`
  arms after `hold_ms`; `Double` = second press within `window_ms`); `eval_commands` is now
  stateful and every slot advances even when a behavior is gated off (edges stay correct;
  reconcile releases outputs). *S7b* - `toggle` (latch on the raw rising edge), `turbo` (50%-duty
  square wave, period `interval_ms`), `interruptible` (Regular deferred -> tap on release iff no
  sibling fired the press). Mapper-level reset-on-rebind golden test. **Deferred:** scroll-impulse
  handling (turbo+scroll), command haptics (decision C).
- [x] **S8 - Layer/set actions + next-tick semantics.** `HoldLayer` (latched to firing
  node's held-state), `AddLayer`/`RemoveLayer` (persistent stack), `ChangeActionSet`; changes
  apply next tick. Golden tests: the 4 pathological cases (self-shadowing HoldLayer stable;
  non-naming layer keeps base output; two Hold layers stack). **Done:** `mapper/layers.rs`
  (`LayerOps` collected per tick; `NodeHeld` re-derives a hold's trigger held-state - frame-local
  buttons/group/soft-pull exactly, layer-dependent virtuals read released -> flicker-not-stick);
  `command.rs` `apply_action` routes output vs layer/set actions; `Mapper::reconcile_layers`
  folds ops into persistent+held layers and recomputes `active_layers` (declared-order) for the
  next tick; set change is a full swap clearing the stack. All six 4 cases golden-tested
  (+persistent Add/Remove, +ChangeActionSet-clears-layers). **Deferred:** the "flicker on a fully
  virtual/differently-thresholded trigger" is accepted, never sticks.
- [x] **S9 - Manager shell (threads/channels).** Reader-thread-per-device (read loop +
  keep-alive + re-apply-on-Connected + haptics writer), central `crossbeam select!` loop
  (owns Sink + Mapper(s), stamps Tick, `poll_rumble`->reader), `HapticReq` plumbing (game
  rumble only in first cut). Global chords evaluated first + button masking; Main/Fallback
  + SwitchFallback switch. **Done in two commits:** *S9a* (`chords.rs` pure global-chord eval
  + `LogicalFrame::masked` + `Mapper::switch_program` - hot main<->fallback swap keeping applied
  levels; unit-tested) and *S9b* (`runtime.rs`: reader thread owns Device; central mapping loop
  owns Sink+Mapper, `select!` over {frame,control,8ms tick}, chords->role+mask->tick->emit,
  rumble poll->reader scaled by master; `Runtime::start/stop/control` + `Control` messages).
  Rumble = latest-level pulse train, sent on change. **Deferred:** command haptics (decision C),
  status/Battery surfacing (S10).
- [x] **S10 - `Engine` handle + `deckhand-run` binary.** `new`/`set_input`/`set_output`/
  `apply`/`set_chords`/`set_device_config`/`start`/`stop`/`devices`/`status`/`monitor`/`shutdown` over the
  control channel; the CLI (`--wired/--dongle`, profile/fallback/chords/devcfg `.ron`,
  `env_logger`, ctrlc -> clean shutdown). HW-validate against the bridge behavior. **Done:**
  `handle.rs` (`Engine` - staged input/output Local; `apply`/`set_chords` retained + live
  hot-swap; `start` opens device [examples' wired-direct/dongle-poll selection] + `Sink` +
  spawns `Runtime`; `stop` joins + retains config; `Error::NotReady` for misuse); `monitor`
  folded into `status`+`devices` (Battery/lifecycle surfacing deferred). `examples/deckhand-run.rs`
  loads RON -> `compile` -> drives the stack, ctrlc/SIGTERM clean shutdown; ships
  `examples/test_profile.ron` (bridge-parity) + a test that compiles it. **HW-VALIDATED on the
  Gordon dongle** - full stack (input->Mapper->virtual pad+kbd/mouse, HoldLayer mode-shift, fallback
  chord, rumble) confirmed working in a real game + jstest; a few output **sign conventions** were
  fixed during validation (cursor/stick Y +up->+down, gyro yaw->-X, scroll coarser + own polarity -
  all in `mapper/behavior.rs`).

**Since done (were deferred past the first cut):** CommandExecute chords; command-activation
haptics (decision C); 1-Euro filter; player-space gyro; gyro spaces (Yaw/Roll/YawRoll/PlayerSpace).
**Still deferred:** response curve on the mouse behaviors; fusion-based gravity + World/Laser gyro
spaces; Deck haptics; Battery/status surfacing and true USB hotplug enumeration (**design in 4.3**);
multi-device -> multi-Mapper; the daemon/socket transport and 6 networking.

**Structural refactors.** **Q2 DONE (2026-08-01):** `runtime.rs` (~815 lines) split into a `runtime/`
module - `mod.rs` (Runtime lifecycle + inter-thread types), `reader.rs` (reader thread + device
writes + reacquire), `mapping.rs` (the central mapping loop) - pure move, tests unchanged.
**Q1 DEFERRED (to-decide):** split the pure `mapper/` core (+ `program`/`compile`/`logical`/
`reconcile`/`gyro`/`smooth`) into its **own crate**, leaving `engine` = `handle`/`runtime`/`chords`/
`event`. The mapper *module* is already pure/golden-tested, so the crate mainly buys a **hard wall
enforcing that purity** (the mapper crate physically can't depend on the runtime/threading code) - a
lasting but mostly-preventative gain. Moderate work (decide which crate owns the shared IR types).
Do it at a natural pause if the enforcement appeals; the Q2 reader/loop split already clarified where
the boundary would fall.

### 4.3 Device availability, hotplug & reconnection (design agreed 2026-07-31 - NOT built)

Covers the deferred **USB hotplug**, **Battery/status surfacing**, and (surface-sharing) **daemon**
items together. **Single active device throughout** - several controllers may be *connected*, but the
engine runs/maps exactly **one** (multi-device -> multi-Mapper stays deferred). Decisions below are
agreed but **not yet implemented**.

- **Enumeration is presence-based (the topology model).** `enumerate()` returns **one `DeviceInfo`
  per Steam gamepad interface present**, independent of whether a controller is actually connected to
  it. The **dongle is a composite device exposing 4 slot interfaces** (distinguished by
  `DeviceInfo.interface`), all present whenever the dongle is plugged; the **wired controller is 1**.
  So a dongle + a wired controller = **5 `DeviceInfo`s from 2 USB ports**, all selectable.
  `Explicit`/`Transport` can open an **idle** slot (the interface exists -> opens fine, streams nothing
  until a controller pairs -> then `Report::Connected`); **`Auto` skips idle slots** (Auto = "first that
  *streams*"). *(HW-confirmed 2026-07-31: `--dongle` with the controller off enumerates all 4 slots.)*

- **`DeviceId` - a stable identity decoupled from the path.** Reconnect needs an identity that
  survives replug. hidapi's `path` is the **`/dev/hidrawN` node, reassigned on replug** (ephemeral);
  the **real USB topology path is *not exposed* by hidapi** (would need Linux `libudev`/sysfs, per-OS
  - **explicitly out of scope, no udev**). So `DeviceId` is derived from the **stable public fields**:
  `transport` + `serial` + `interface`(slot) (+ `pid`). Reconnect = match `DeviceId` in a fresh
  `enumerate()`, then open by the **current** path -> **survives the Linux path changing**. **Not
  pinned:** the physical USB *port* (no topology path) - moving the dongle to another port still
  reconnects; the only loss is telling two *identical* dongles apart, and **same-model-same-serial is
  pathological, ignored for V1**. *(Quick `list` check that the dongle populates `serial`; if `None`, a
  single dongle is still unique via `transport`+`interface`.)*

- **Selection resolves once, at `start()`; V1 reconnects only to that exact device.** The policy
  (`Auto`/`Transport`/`Explicit`) resolves **once at `start()`** to one concrete device; the engine
  **pins its `DeviceId`**. This keeps "device fixed within a start/stop pair" - the policy is
  irrelevant until the next `start()`. A physical drop -> `WaitingForDevice`, which waits for **that
  same pinned `DeviceId`** and reacquires only it; a *different* device that also matches the policy is
  **ignored** (true failover to a different device is surprising -> **deferred, opt-in later**, never
  default). Reconnect is therefore "the same physical device came back," never a mid-run switch.

- **Transport-gone (USB loss) -> `WaitingForDevice`, not `stop`.** Two disconnect flavors stay
  distinct. *Controller-off-dongle-alive* (`Report::Disconnected` value) is already handled - the
  reader keeps its interface and waits for `Connected`. *Transport-gone* (the interface vanishes;
  reader read -> `Err`) **currently just kills the reader thread silently while `status()` still says
  `Running`** - the resilience hole. Plan: on transport-gone, **tear down that reader thread, reconcile
  all outputs to neutral** (reconcile already prevents stuck keys), **keep the virtual pad plugged**
  (the game shouldn't lose its controller mid-session), and move to a new **`Status::WaitingForDevice`**
  (`Idle | Running | WaitingForDevice`) - **not** `stop`/`shutdown`, config retained - then
  **auto-reacquire the pinned `DeviceId`** when it reappears.

- **Topology awareness = on-demand, no monitor thread (DECIDED 2026-08-01 - was a poll-monitor).**
  An always-polling topology-monitor thread (poll `enumerate()` ~1 s, diff, debounce, push
  `DeviceAdded`/`Removed`) was planned but **dropped as YAGNI.** `enumerate()` re-scans the bus on
  every call (the D6 `refresh_devices()` fix), so a device plugged in *after* the daemon started is
  already discoverable + usable on demand - `list-devices` -> `set-input <id>` -> `start`, **no restart**
  (HW-confirmed). Device plug/unplug is a rare, user-initiated event, so a **manual UI refresh** (or the
  UI polling `list-devices` client-side *only while the picker is open*) covers it - a background thread
  spinning 24/7 to save one click isn't worth it. The never-emitted `DeviceAdded`/`Removed` events were
  removed (`ipc::Event` stays `#[non_exhaustive]` so they can come back). **Far-future option (only if
  ever genuinely needed):** a *native* OS device-change push - Linux udev / Windows `WM_DEVICECHANGE` /
  macOS IOKit - which would re-add those events; polling is explicitly *not* the plan. *(Unrelated: the
  reader's own reacquire poll runs only while `WaitingForDevice`, not continuously.)*

- **Event surface = a logging stub first.** The natural API for all this (hotplug add/remove,
  connect/disconnect, **battery**, binding lost/acquired) is one `subscribe() -> Receiver<EngineEvent>`
  stream - which is also what **Battery/status surfacing** needs and what the **daemon** must serialize
  onto its socket. Rather than commit that API early, **stub it now**: wherever an `EngineEvent` *would*
  be emitted, log a shaped `log::info!("event(stub): ...")` line - no channel, no API commitment, but the
  stream is visible in the log and doubles as the spec. Three kinds are detectable in today's code:
  **controller connect/disconnect** (reader already sees them), **battery** (reader already gets `0x04`),
  **topology add/remove** (once the monitor lands). Then: real event channel -> more event kinds ->
  daemon serialization - all later.

- **Sequencing & open points.** Build this **after / alongside the daemon**, since the event+status
  surface is shared (fold in Battery and the transport-gone resilience fix while doing the daemon).
  Concrete first symptom to validate against: **`--dongle` with the controller off ->
  `NotReady("no matching controller streamed")`** (from `open_device`'s `many` branch, which needs a
  stream to disambiguate the 4 slots) - the poster-child for "`start()` into `WaitingForDevice`".
  **Open:** cold-start "dongle present but no controller" (a live idle interface) vs "USB device gone
  entirely" (nothing) are *different* states and may deserve distinct handling - **do not flatten** them
  into one `WaitingForDevice` prematurely. **Assumption:** `hid-steam` is blacklisted on the dev box
  (raw hidraw open unobstructed); **deferred check** of hotplug/open behavior with the kernel module
  loaded (it would claim a freshly-appeared device -> reacquire needs the detach dance).
  **Two disconnect flavors, two behaviors (both DONE):** *transport-gone* (unplug) -> D5 waiting phase
  (reader dies, `release_all`, keep pad, `WaitingForDevice`); *controller-off-dongle-alive* (battery/
  range) -> mapper `release_all`s on `Report::Disconnected` but **stays connected** (reader alive,
  resumes on `Connected`) - fixes a real stuck-output bug (held stick kept a game character running).
  **DEFERRED (to-decide):** `release_all` drops *outputs* but keeps mapper latches (toggles, active
  layers), so a toggle re-asserts on reconnect - revisit whether a disconnect should reset transient
  mapper state too (raised 2026-07-31; don't lose it).

### 4.4 Daemon, client & control socket (design agreed 2026-07-31 - NOT built)

The **daemon** (`deckhandd`) and a thin **CLI client** (`deckhandctl`) that drives it over a local
socket. Both are **new binary crates** wrapping the engine library (`engine` stays library-only).
Sequenced with 4.3 (shared event/status surface). *(Self-daemonization / `--background` was
considered and **dropped**: under systemd or a Windows service you run **foreground** and the
supervisor manages you; manual runs use Ctrl-C. No daemonization code at all.)*

**`deckhandd` - one binary, two uses.** It is both a **standalone runner** (pass everything on the
CLI -> a working mapper, Ctrl-C to quit) and a **controllable daemon** (a client connects and drives
it further). It **always opens the control socket**; the two uses intermingle freely - the CLI seeds
initial state, the client mutates it live (`apply`/`set_chords` hot-swap, `set_input`/`set_output`
stage). Promote the current `deckhand-run` **example** into this **binary crate**; `engine` stays a
pure library.

**CLI = a thin, explicit seed for the engine.** Options:
- `-m/--main <profile.ron>` -> `apply(_, Main)`
- `-f/--fallback <profile.ron>` -> `apply(_, Fallback)`
- `-c/--chords <chords.ron>` -> `set_chords`; `-d/--devcfg <devcfg.ron>` -> `set_device_config`
- `-i/--input <dongle|wired|STRING|host:port>` -> `set_input`. `dongle`/`wired` = `Transport`; `STRING`
  = `Explicit` by **`DeviceId`** (the 4.3 stable-id string - parse it or error; this *is* the
  "sanitization"); `host:port` = the **`Network` input** (6 sink role - engine variant currently stubbed).
- `-o/--output <local|host:port>` -> `set_output`. `local` = `Local`; `host:port` = the **`Network`
  output** (6 source-role sender - engine variant currently stubbed). Symmetric with `--input`.
- `-s/--start` -> call `start()` after seeding.
- `-l/--list-devices` -> **the one non-persistent option:** enumerate the attached devices, print
  their `DeviceId` strings to stdout (same format as `deckhandctl list-devices`), and quit **before**
  any seeding or the socket is served. Handled first, ahead of `-m`/`apply()`.
- `-v/-vv/-vvv` -> verbosity (unchanged).

**The seeding rule - pass-through, with exactly one convenience.** The daemon issues to the engine
**only the calls whose options were actually given** - no automagic. **Sole exception:** when
`--start` is passed, a *missing* `--input`/`--output` is defaulted (`--input`->`Local(Auto)`,
`--output`->`Local`) so `start()` has a source/sink - but **only** because `--start` was passed.
Without `--start`, `set_input`/`set_output` fire **only** for explicitly-given options. Profiles are
**never** defaulted (there is no default program): `--start` with no `--main` -> `start()` returns
`Error::NotReady`, which is **logged, and the daemon keeps serving the socket** (identical to a client
asking to start an under-configured engine). `--start` itself is a real command (it calls `start()`);
only its **defaulting of a missing `-i`/`-o`** is the convenience.

**`deckhandctl` - thin one-shot client + a follow monitor.** One-shot commands mirror the Engine API
1:1: `main`/`fallback` (ship a profile), `chords`/`devcfg` (ship above-profile config),
`start`/`stop`/`shutdown`, `list-devices`,
`select-device`, `status`. Plus a persistent **`monitor`** mode (`-f`/follow, like `journalctl -f`)
that streams events and prints them (lands with the real event channel, 4.3 - a stub until then).
Connect -> send one request -> print reply -> exit; `monitor` stays connected until Ctrl-C.

**Who compiles: the client ships `ConfigDoc`, the daemon compiles.** `compile()` lives in `engine`;
a thin client must not link it. So `main`/`fallback` ship the **`ConfigDoc` (RON)** and the **daemon**
compiles + applies, returning diagnostics over the socket. This keeps the client genuinely thin **and**
is the *same split as 6 networking* (config over the wire, compile + Mapper on the sink) -> free
groundwork for the remote-controller role. The client *may* depend on `config` for optional local
pre-validation, but the daemon is authoritative.

**Crate structure (decided).** The wire protocol (request/response/event types, framing, codec) is
**shared by all sides**, so it lives in **one dedicated IPC library crate** (`ipc`) holding
the message enums + a `Client` helper + a server-listener helper. Deps: `serde` + `postcard` +
`interprocess` + **`config`** (messages carry `ConfigDoc`) - **not** `engine`. Then `deckhandd` =
`ipc` + `engine`; `deckhandctl` = `ipc` (thin); the **UI in daemon mode** also
depends on `ipc` and drives the daemon over the same socket - so the crate is the single
client-side entry point for both `deckhandctl` and the UI. *(Preferred over putting the client lib
inside `deckhandd`: a crate has one dependency set, so depending on the `deckhandd` crate would
transitively drag `engine`/`steam-hid`/`virt-out` into a thin client. The shared IPC crate keeps the
client clean while still keeping both sides' socket code in one crate.)*

**The control socket.**
- **Location (Linux):** `$XDG_RUNTIME_DIR/deckhand.sock` (under `/run/user/<uid>/`). Per-user, cleaned
  on logout.
- **Cross-platform, minimal code:** the **`interprocess`** crate - a unified **sync** local-socket API
  over **Unix domain sockets** (Unix) and **named pipes** (Windows), light and well-maintained (fits
  the dependency policy, no async/tokio). The only platform-specific code is the **name**:
  `$XDG_RUNTIME_DIR/deckhand.sock` vs `\\.\pipe\deckhand` (one `cfg`).
- **Wire format:** length-prefixed **`postcard`** messages (compact, reuses serde; RON was considered
  for `nc`-debuggability but rejected as a wire format). One `Request`, one `Response`, one `Event`
  enum. Low-rate control plane.
- **Single-instance:** binding the socket/pipe **is** the lock - a failed bind = "already running", no
  pidfile needed (clean up a stale Unix socket file on start).
- **systemd socket activation (BUILT - Linux user session):** `deckhandd --systemd` **adopts** the
  listening socket systemd binds and passes via `LISTEN_FDS` (fd 3) - it **never binds or removes one
  itself** (systemd owns the socket's lifecycle) - and reports readiness with **`sd-notify`**
  (`READY=1` once accepting, `STOPPING=1` on teardown; unit is `Type=notify`). The `sd-notify` crate
  (`listen_fds()` validates `LISTEN_PID`, sets the fd close-on-exec; `notify()` sends the state) is
  the one dependency. Adoption is clean because the socket-setup was already isolated in `ipc`: the
  daemon does the only `unsafe` step (`fd -> std::os::unix::net::UnixListener::from_raw_fd`), reads the
  socket's **real bound path** off it via `getsockname` (`local_addr()`) - used for the log line **and
  the Ctrl-C/SIGTERM self-connect wake, so shutdown works regardless of the unit's `ListenStream`
  name** - then hands the listener to a **safe** `ipc::Server::from_unix_listener` (-> interprocess
  `uds_local_socket::Listener`, which carries **no reclaim name** so `Drop` won't unlink the systemd
  socket -> generic `local_socket::Listener`). `incoming()`/`Conn` and the whole serve loop are
  untouched. `--systemd` with no `LISTEN_FDS` is a **hard error** (never a self-bind fallback), and it
  drops env_logger's own timestamp (the journal already stamps each line). Non-`--systemd` runs bind
  their own socket as before (single-instance = the bind, stale-socket dance, remove-on-exit). See
  4.5 for the two units + enablement model. *(Windows service still deferred.)*
- **Serving = thread-per-connection (DECIDED 2026-08-08 - was serial).** The original accept loop
  served one connection to completion, blocking on `recv()` for that client's *next* request - so a
  client holding its connection **open and idle** (the daemon-mode UI's persistent command
  connection) wedged the accept loop, and any second client (`deckhandctl`) hung indefinitely. Only
  `Subscribe` escaped it (already handed to its own thread). Now **each accepted connection is served
  on its own thread** over a shared `Arc<Mutex<Daemon>>`: a request briefly locks the engine, so a
  persistent-connection UI, an event-stream subscriber, and an occasional `deckhandctl` are served
  concurrently. Engine access stays **serialized by the mutex** (handling is fast; the mapping loop
  runs on its own threads regardless -> networking/engine internals untouched). `Engine` is `Send`
  (all fields `Send`; `hidapi::HidApi` is just a `Vec` device-list cache) so there's **no `unsafe`**.
  `Daemon::shutdown` now takes `&mut self` (releases HW **in place** after the accept loop, rather
  than consuming `self`), and the event monitor runs on the connection's own serve thread (no
  separate spawn). Regression test: a client holding its connection open must not block a second one.

### 4.5 Implementation steps - daemon, hotplug & events (checklist - NOT started)

Builds 4.3 + 4.4 across several sessions. Order is **dependency-driven** - a usable controllable
daemon exists after **D3**, then resilience/hotplug/events layer on. Each item is its own reviewable
commit(s), HW-validated on the Gordon dongle where it touches device lifecycle. `engine` stays a
library; `deckhandd`/`deckhandctl` are new binary crates. Prefix **D** (daemon phase).

- [x] **D0 - `DeviceId` (stable device identity).** `steam_hid::DeviceId` (`kind` + `transport` +
  `interface`(slot) + `serial`) with `Display`/`FromStr` (grammar `kind:transport:interface:serial`,
  e.g. `gordon:dongle:1:ABC`; serial = verbatim remainder); `DeviceInfo::id()`; `as_str`/`from_token`
  on `DeviceKind`/`Transport`; `Error::ParseDeviceId`. `DeviceSelect::Explicit` now carries a
  `DeviceId` and `open_device` resolves it against a fresh `enumerate()` (opens by the current path ->
  survives replug, 4.3). `list` example prints the id. **Still to verify on HW:** that the dongle
  populates `serial` (single-dongle stays unique via transport+slot regardless).
- [x] **D1 - `ipc` crate (wire protocol).** New lib crate: `Request`/`Response`/`Event`
  enums (+ `ProfileRole`/`StatusSnapshot`/`RunState`; messages carry `config::ConfigDoc`, selection specs
  + device ids travel as strings the daemon parses), length-prefixed **`postcard`** framing
  (clean-EOF -> `Ok(None)`), and `interprocess` local-socket **`Client`**/**`Server`**/**`Conn`**
  (Unix domain socket / Windows named pipe; only the socket *name* is platform-specific). Deps:
  serde/postcard/interprocess/config - **not** `engine`. Codec unit-tested + a real client<->server
  round-trip integration test. clippy clean.
- [x] **D2 - `deckhandd` binary (daemon).** New binary crate (`engine` + `ipc` + `config`).
  CLI `-m/-f/-g/-i/-o/-s/-v`; the **pass-through seeding rule** (only given options applied; `--start`
  the sole convenience, defaulting a missing `-i/-o` to `auto`/`local`; profiles never defaulted ->
  `--start` with no `-m` logs `NotReady` and keeps serving). Socket at `$XDG_RUNTIME_DIR/deckhand.sock`
  (override `$DECKHAND_SOCKET`, or `-k`/`--socket <path>` per-invocation - see the post-pass note),
  bind = single-instance lock + stale-socket dance; request/reply
  -> the `Engine` handle (serving was serial; now **thread-per-connection**, 4.4); Ctrl-C/SIGTERM
  (`ctrlc` `termination`) wakes accept via self-connect -> clean
  shutdown + socket removed. Client ships `ConfigDoc` -> **daemon compiles** -> `Ok`/`Diagnostics`.
  Input/output spec parsing shared by CLI + `SetInput`/`SetOutput`. Also derived `Debug`/`Clone` on
  engine `Input`/`Output`/`DeviceSelect`. End-to-end **subprocess test** drives the real binary over a
  temp socket (no HW). **HW-validated** on the Gordon dongle (`deckhandd -m ... -f ... -g ... -s`: mapping +
  Main/Fallback chord switching confirmed, socket at `/run/user/1000/deckhand.sock`); the old
  `deckhand-run` example is **retired** (superseded by the daemon).
- [x] **D3 - `deckhandctl` binary (client).** Thin one-shot: `status`, `list-devices`,
  `select-device <id>`, `input`/`output <spec>`, `main`/`fallback` (ship `ConfigDoc`) +
  `chords`/`devcfg` (ship `Chords`/`DeviceConfig`), `start`/`stop`/`shutdown`. Depends on `ipc` +
  `config`, **not** `engine`.
  Connect -> one request -> render reply -> exit (errors/diagnostics -> stderr + non-zero). `$DECKHAND_SOCKET`
  folded into `ipc::default_socket_path` so daemon+clients agree. End-to-end test (real
  `deckhandctl` vs an in-process fake daemon) + **live-checked** against a real `deckhandd` (status /
  select-device / list-devices showing the 4 real dongle slots / shutdown).
- [x] **D4 - Status + event stub + battery surfacing.** `Status` -> `Idle | Running |
  WaitingForDevice` (the variant; its transition is D5) + mapped in the daemon. `engine::EngineEvent`
  defined (`ControllerConnected`/`Disconnected`, `BatteryChanged`, `BindingLost`/`Acquired`, `State`;
  the `DeviceAdded`/`Removed` variants were later dropped with the poll-monitor - 4.3), the mirror of
  `ipc::Event`. **No channel yet** (D7):
  `EngineEvent::emit()` logs an `event(stub): ...` line. Wired now: the reader emits connect/disconnect
  (replacing the ad-hoc logs) + **`BatteryChanged`** (edge-triggered off the ~1 Hz `0x04` report ->
  surfaces the deferred Battery item at stub level); the handle emits `State(Running)`/`State(Idle)`
  on start/stop. (4.3 event surface.)
- [x] **D5 - Transport-gone resilience -> `WaitingForDevice`.** Reader on transport-gone `Err` sets a
  shared `waiting` flag + emits `BindingLost` + exits (device drops -> lizard). The mapper splits into a
  connected phase and a **waiting phase** (`run_waiting`): on frame-recv error with `waiting` set it
  `release_all()`s every applied output (drops all held keys/buttons/axes regardless of toggles/latches
  - a golden test covers it), **keeps the pad plugged**, emits `State(WaitingForDevice)`, and idles on
  `{control, tick}` accepting config hot-swaps + draining the pad's FF uploads (so a game's rumble
  thread can't block). `Runtime` carries the flag; `Engine::status()` -> `WaitingForDevice`. Control
  handling factored into `apply_control` (shared by both phases). Fixes the silent-dead-reader hole.
  **HW-validated:** dongle unplugged mid-session -> reader `Err` -> `BindingLost` + `State(WaitingForDevice)`
  events, `status()` -> `WaitingForDevice`, daemon survives with config retained (`main`/`fallback` still
  loaded). (Auto-reacquire is D6.)
- [x] **D6 - Seamless reattach (auto-reacquire, pad never leaves).** Chose **Option B** (keep the
  `Sink`/mapper alive across the outage) over A (restart - blips the pad, risks Proton not
  re-enumerating). The **reader is now a persistent device-session loop**: `read_session` reads until
  stop/transport-gone; on transport-gone it flags `waiting` + `BindingLost`, then `reacquire()` polls
  its **own lazily-created `Manager`** (~1 Hz) for the pinned `DeviceId` to reappear, reopens it, mints
  fresh channels, emits `BindingAcquired`+`State(Running)`, and sends `Control::Reattach` to the mapper.
  The **mapper alternates connected/waiting phases** in an outer loop with **swappable** frame/rumble/
  click channels, so the same `Sink` serves throughout - the game never sees a disconnect. `start()`
  pins `device.info().id()` (reacquire targets the *exact* resolved device, 4.3). **Prerequisite bug
  fixed:** `Manager::enumerate()` now `refresh_devices()` first (hidapi caches at context creation -> a
  long-lived `Manager` never saw hotplug; broke `devices()` freshness *and* reacquire); `enumerate`/
  `open_first` are `&mut self`. **Key correctness detail (HW-found):** on transport-gone the reader must
  **drop its old frame/rumble/click channels** so the mapper's `frame_rx` disconnects and it enters the
  *waiting* phase - else the mapper stays *connected* (never releases outputs, never emits
  `WaitingForDevice`) and the later `Reattach` is ignored there -> channels never swap -> no mapping +
  lizard stuck on. `apply_device_cfg` is **non-fatal** (a transient feature-write NAK must not tear down
  the reader) and reader-thread errors are logged (a silent death cost two debug round-trips). **HW-
  VALIDATED:** unplug+replug the dongle mid-game -> `WaitingForDevice` -> reattaches to the same device,
  controller never drops in-game, mapping + chord-switch resume, lizard off. *(General topology
  `DeviceAdded`/`Removed` events + the poll-monitor were later **dropped** - see 4.3 "Topology
  awareness = on-demand". The cold-start "dongle present, no controller" state Just Works via Explicit
  selection opening an idle slot - HW-confirmed.)*
- [x] **D7 - Real event channel + client monitor.** `engine::EventSink` broadcasts `EngineEvent` to
  subscribers (`Arc<Mutex<Vec<Sender>>>`; still logs each at info); `Engine::subscribe() -> EventStream`
  (thin wrapper hiding the channel). The sink is cloned into the reader+mapping threads and held by the
  handle, so every emission point feeds one stream; `EngineEvent` is no longer `#[non_exhaustive]` (a
  new variant should break the daemon's wire map). `ipc`: `Client::subscribe()`. Daemon: the
  serve loop **intercepts `Subscribe`** and turns that connection's own serve thread into the client's
  event pump (`monitor`), mapping `EngineEvent`->`ipc::Event` and streaming it - so a long-lived monitor
  no longer blocks request/reply (now moot - every connection is per-thread, 4.4). `deckhandctl
  monitor` subscribes + prints events until close/Ctrl-C.
  Verified: fake-server monitor test + a live real-daemon check (subscribe logged, concurrent shutdown
  still served, stream closes cleanly) + **HW-validated** (live device events over the socket during
  real use).

**This implementation pass is DONE (D0-D7, all HW-validated).** The daemon + client are complete and
usable. **Windows: HW-tested by the user - `deckhandd`/`deckhandctl` over the named-pipe socket work.**

**Post-pass refinements (redundancy + status snapshot).** (1) **Daemon holds no shadow state:** the
`Daemon` struct is just the `Engine` - status is read back from the engine, not kept in parallel copies;
`Input`/`Output`/`DeviceSelect` carry `Display`+`FromStr` so a reported spec round-trips straight back to
`set_input`/`set_output`. (2) **Status unified into one snapshot:** an intermediate step exposed five
engine getters that the daemon reassembled field-by-field, re-diverging the engine and wire shapes; that
merged back into **one `Engine::status() -> engine::StatusInfo`**, mirroring the single wire reply. The
parallel types follow the project's naming convention (`Status`/`RunState`, `Role`/`ProfileRole`):
**engine `StatusInfo` is typed** (`input: Input`, `output: Output` - the engine is the richer layer)
while the wire mirror **`ipc::StatusSnapshot`** carries the stringified round-trip specs, the daemon
stringifying in one `status_info()` map. (3) **Bound device surfaced in status + on every bind
(2026-08-02, for the UI):** status reported the *staged* `input` (which may be a policy like
`auto`/`dongle`), never the concrete device actually in use, and `BindingAcquired(DeviceId)` fired only
on *reacquire* (D6), never the initial bind - so a UI attaching to a live daemon could not learn what is
bound. Two complementary fixes. **(a) status field:** `Engine` gained `bound: Option<DeviceId>`, captured
at `start()` from the reader's pinned id (a cheap clone of a value the handle already holds before it
moves into the `Runtime` - *not* shadow state: the id lives across the reader thread boundary, so the
handle keeping a copy is its natural home, same category as staged `input`/`output`), cleared at `stop()`,
and **left untouched through the waiting phase** so it keeps reporting the id being reacquired while
`WaitingForDevice`. Threaded `StatusInfo::bound: Option<DeviceId>` (typed) ->
`ipc::StatusSnapshot::bound: Option<String>` (stringified) -> `deckhandctl` prints a `bound:` line. For
`Auto`/`Transport` selections this adds information the snapshot lacked (staged `input`=`auto` vs
`bound`=the resolved concrete id). **(b) event symmetry:** `start()` now emits `BindingAcquired(pinned_id)`
before `State(Running)` (mirroring the reader's reacquire order), so **every** bind - initial and reacquire
- emits `BindingAcquired`, symmetric with `BindingLost`; a live subscriber no longer special-cases the
first. The two halves are the standard **subscribe-then-snapshot-to-seed** pair: the event serves a client
already subscribed at bind time, the status field seeds current truth for one that connects afterward. No
IPC wire-map change for the event (`ipc::Event::BindingAcquired` already existed). **(4) Control-plane
sync - staged/profile/above-profile events + full above-profile config in status:** so a UI attached to
a running daemon stays current when another client mutates it on the side. New **absolute-valued** events
fire from the handle setters - `InputStaged`/`OutputStaged` (the staged selection changed),
`ProfileSet{role,name}` (a program was applied to a role), and (post-2026-08-19-split) the two
above-profile setters emit **`DeviceConfigSet(DeviceConfig)`** + **`ChordsSet(Option<Chords>)`** (the
old single `GlobalConfigSet(GlobalConfig)` is gone) - mirrored on the wire (`ipc::Event`; the daemon
stringifies specs / maps `Role`->`ProfileRole`). `StatusInfo`/`StatusSnapshot` also gained a full
**`device_config: DeviceConfig`** field plus a **`chords: Option<Chords>`** field (kept last), so one
`status()` seeds a client's complete view and later changes stream as events. **Above-profile config
travels whole, profiles by name** - the daemon holds only a compiled `Program` (no serde back to
`ConfigDoc`) whereas `DeviceConfig`/`Chords` are uncompiled, and `ipc` already depends on `config`; the
crate boundaries make this the only shape that fits. Events carry
absolute state (not deltas) -> the **subscribe-then-status-to-seed** race stays safe (a duplicate is fine,
a gap impossible). **Live active-role ("switch profile") event - BUILT 2026-08-11, see the
"Live thread-state readbacks" entry below** (`ActiveRole(Role)` + `status().active`; the follow-up
sketched here - a role edge from the mapping thread plus the live role fed to `status()` via a shared
atomic like `waiting` - is exactly what landed). **(5) `--socket`/`-k` on both binaries:** override `$DECKHAND_SOCKET` and the default
with an explicit control-socket path (Unix) / pipe name (Windows) - needed to run a second daemon on its
own socket for 6 localhost network testing, and the only socket override that works on Windows (the env
var is Unix-only). Both binaries resolve the default identically (`ipc::default_socket_path` /
`DEFAULT_PIPE_NAME`); `ipc::Client::connect_default`/`Server::bind_default` were dropped in favor of the
explicit `connect_path`/`bind_path` (Unix) / `connect_name`/`bind_name` (Windows), removing the
client/daemon asymmetry. A bind failure stays fatal.

**systemd integration (BUILT - Linux user session).** Two user units ship in `systemd/`, installed
(not enabled) by `install.sh` into `${XDG_CONFIG_HOME:-~/.config}/systemd/user` with the service's
`ExecStart` rewritten to the resolved `$CARGO_INSTALL_ROOT/bin/deckhandd`:
- **`deckhandd.socket`** - `ListenStream=%t/deckhand.sock` (`%t` = `$XDG_RUNTIME_DIR`, matching
  `ipc::default_socket_path()`, so an un-configured `deckhandctl` connects there and triggers
  activation), `Accept=no` (one long-lived daemon serves all connections), `SocketMode=0600`,
  `RemoveOnStop=yes`; `WantedBy=sockets.target`.
- **`deckhandd.service`** - `Type=notify`, `ExecStart=... deckhandd --systemd --verbose` (so `info!`
  reaches the journal), `Requires=deckhandd.socket` + `After=deckhandd.socket`, `Restart=on-failure`;
  `WantedBy=default.target`.
- **Enablement model (enable one, not both):** enabling **only the socket** -> lazy activation (first
  `deckhandctl` connection starts the daemon on demand); enabling **only the service** -> always-on
  from login - `Requires=` pulls the socket up first so the fd is always socket-activated (the daemon
  never self-binds under `--systemd`). A `.socket` passes its fd to the same-named `.service` by
  default, so no explicit `Service=` is needed. Enabling both is redundant (harmless - the service
  never binds), so we don't (no `[Install] Also=`). See 4.4 for the fd-adoption mechanics.

**Still deferred (not now):** a proper **Windows service** (the named-pipe socket compiles and the
daemon is user-tested on Windows, but no service wrapper is built). The `--background`/daemonize path
stays dropped (4.4) - real supervisors run the daemon foreground.

**Binding-event/status consistency + profile clear (2026-08-09).** Two related follow-ups the UI
bake-off surfaced.

**(1) Binding events now track the bound-device lifetime.** The earlier "(3)(b) event symmetry" model
emitted `BindingLost` on transport-gone and `BindingAcquired` on *both* initial bind and reacquire -
but `status().bound` (the pinned id) stays set across an outage and is only cleared at `stop()`, which
emitted *no* binding event. So a UI mirroring `bound` off the event stream never cleared its
bound-device display on stop, while `status()` reported nothing bound - a visible disparity. Fixed by
making the two binding events **bracket exactly the `self.bound` lifetime**: renamed `BindingLost ->
BindingRemoved`, emitted at `stop()` alongside `State(Idle)`; `BindingAcquired` stays **start-only**.
The reader's transport-gone/reacquire path no longer emits binding events - it emits
`State(WaitingForDevice)`/`State(Running)`. The mapper already emits those same edges, so locally
they're duplicated (idempotent - the events are absolute-valued), and on a 6 network split (reader on
the controller machine, mapper on the PC) each side's copy is the *only* run-state signal its own
subscribers see. Net invariant: **`BindingAcquired <=> bound set (start)`, `BindingRemoved <=> bound
cleared (stop)`**, so the event stream and `status().bound` never diverge. All four `ui-test` consumers
clear their bound field on `BindingRemoved`. (Supersedes the reacquire-emits-`BindingAcquired` behavior
noted in the (3)(b) narrative above.)

**(2) Clear a profile role (`Some -> None`).** `Apply` could only *set* a role's program; there was no
way to unset it, so a game-launch wrapper couldn't hand control back to the fallback when the game
exits. Threaded `Option` through the whole apply path - `ipc::Request::Apply{config:
Option<Box<ConfigDoc>>}`, `Engine::apply(Option<Program>, role)`, `Control::Apply{program:
Option<Box<Program>>}`, `Uplink::Apply{program: Option<Program>}` - where `None` **clears** the slot.
`apply_control` re-seeds the live mapper via `program_for`, so clearing `main` resolves straight to
`fallback` (both-empty still falls through to the shared empty placeholder). `deckhandctl main ""` (an
empty string) clears the role; the daemon skips compilation on a clear and emits `ProfileSet{name:
None}`. Enables **semi-automatic per-game switching** with no chords/UI: `deckhandctl main game.ron` ->
launch the game -> `deckhandctl main ""` on exit reverts to the desktop/fallback profile. The clear
round-trips over the wire (`clear_role_survives_the_tcp_frame_codec`), so it works networked too.

**Live thread-state readbacks - controller-connected + active-role (2026-08-11).** Two pieces of
live state that lived *inside* the runtime threads and so were invisible to `status()` (a pull built
only from handle-local fields): the reader's controller-presence and the mapper's live role (which
flips on `SwitchProfile` chords - the deferred item above). Both surfaced the same way, mirroring the
existing `detached`/`is_waiting` precedent: a **`Runtime`-owned `Option<Arc<AtomicBool>>`** the owning
thread writes and the handle reads in `status()`, paired with a **value-carrying event**. The
`Option` is `Some` **only in the roles whose thread runs** - controller-connected in local + client
(a reader exists), `fallback_active`->active-role in local + server (a mapper exists) - and `None`
otherwise, which is the *honest* answer: a forwarder's live role is on the remote server, a server has
no local controller, an idle engine has neither. `StatusInfo` gained `controller: Option<bool>` +
`active: Option<Role>`; the wire mirror `ipc::StatusSnapshot` gained `controller: Option<bool>` +
`active: Option<ProfileRole>`.

- **Events carry their value uniformly.** The old split `ControllerConnected`/`ControllerDisconnected`
  pair was **merged into `ControllerConnected(bool)`**, and a new **`ActiveRole(Role)`** parallels
  `State(Status)` (the profile-mode edge; distinct from `ProfileSet`, which reports a program loaded
  *into* a slot, not which slot is *live*). Wire: `ipc::Event::ControllerConnected(bool)` /
  `ActiveRole(ProfileRole)`. Both are **store-before-emit** (the atomic is never staler than the last
  event), so seed-then-subscribe stays race-free.
- **Controller presence is presumed present at every `read_session` start (all transports)** and
  cleared on transport-gone. On **wired/BT** the controller *is* the transport, so there's no
  connect/disconnect report - this presume is the only "connected" signal. It's needed on the
  **dongle** too: the receiver announces `Connected` only on the **first** open, **not** on a re-open
  of an already-on pad, so without the presume a second `start()` would sit at `false` forever. The
  cost is a brief `true`->`false` when a dongle slot is opened with the pad **off** (the receiver then
  reports `Disconnected`) - accepted, since a correct settled state beats no state (an earlier
  attempt to guard the presume out on the dongle broke exactly the re-open case). `set_connected`
  **always emits** (no change-detection): every call site is a genuine transition, a rare duplicate
  is a harmless absolute-valued repeat, and always-emitting is what lets a first reported
  `Disconnected` through past the flag's default `false`.
- **Active-role** publishes at mapper loop start (the initial role - **always Main** since the
  2026-08-19 split dropped `start_profile`/`start_role`; the chord evaluator's `fallback_base` seeds
  from Main - **always emitted** so a UI seeds from the event alone) and on each chord switch (already
  guarded on a real change).
- **Housekeeping:** `EngineEvent::emit` logs at **debug**, not info (events are per-transition and got
  noisy; `status`/`monitor` are the normal-usage surface). Thread-fn arg order settled to a
  convention: the two `Arc<AtomicBool>`s adjacent (`running` + the readback flag), `events` last.
  `deckhandctl status` prints `controller: Connected|Disconnected|(none)` and `active:
  Main|Fallback|(none)`; `monitor` renders both events.

## 5. UI

**Purpose.** The user-facing configurator: create/edit mappings, manage profiles /
action sets, and drive the running mapper.

**GUI (decided).** The `ui` app is a **GUI**. (The only TUI in the project is the
`steam-hid` test harness, 1.8 - kept deliberately for ANSI fun.)

**Toolkit (to test).** `libadwaita` is the preferred look, but its Windows portability
is doubtful - GTK4 itself runs on Windows (MSYS2 / `gtk-rs`), but libadwaita is
GNOME-oriented, non-native and heavy to package there. **Action:** test whether
libadwaita is viable on Windows at all; if not (likely), fall back to **`iced` or
`egui`** (portable, pure-Rust), which is the default expectation.

**Tray (decided).** The app lives in the **system tray regardless** of topology -
whether it ends up as just the configurator or configurator + embedded engine. The tray
is also the natural place for **manual profile switching** (3) - pick the active profile,
drop to the desktop/fallback profile - since automatic game-detection is out of scope
initially. **No toolkit provides a tray natively** (verified in the 5.1 bake-off - not iced,
egui, cxx-qt, nor gtk4); the plan is the **`tray-icon`** companion crate (from Tauri;
cross-platform Linux/Windows/macOS), run alongside whichever toolkit - so tray support does
**not** bias the toolkit choice.

**Dependencies (contingent on the 4 deployment model).**
- **Embedded (engine library linked into UI):** `ui` depends on `config` + `engine` - it
  embeds the runtime.
- **Daemon (engine library hosted in its own process):** `ui` depends on `config` +
  `ipc` (4.4) and drives the daemon over the socket - **not** `engine`.

So under the daemon deployment `ui` links `config` + `ipc` (the shared IPC crate,
never `engine`); the embedded one links `config` + `engine` instead. Resolved together with 4.

**Per-behavior setting scales (design note, from acceleration tuning).** A setting shared across
behaviors can have a **different sensible range per behavior** - the clearest case is `acceleration`
`factor` (pad velocity / gyro deg-s want ~0.02-0.05, stick->mouse deflection `0..1` wants ~1-4; see
`config::Acceleration`), but `sensitivity`/`deadzone` may differ too. Since the three mouse behaviors are
distinct `SourceBinding` variants with distinct settings structs, their settings panels are already
separate widgets, so each can carry its own slider range - **this is a non-issue as long as no shared
setting widget hardcodes a range.** Rule: **any reusable setting widget takes its range/scale as a
parameter from the parent behavior widget.** Escape hatch if the raw per-behavior scales are annoying:
expose a normalized **0-100 % "amount"** in config/UI and let the **engine** map it to the real
per-behavior factor (the engine already owns the speed-scale + gain knowledge) - a uniform slider
everywhere; a config-model decision to make when building the UI.

### 5.1 UI-toolkit bake-off & baseline design (iced reference BUILT; qt/gtk/egui pending)

Before committing to a toolkit, build the **same representative UI slice** in each candidate
(`qt`/`gtk`/`iced`/`egui`) and compare look, feel, and how each handles the daemon wiring
(blocking-call-off-the-render-thread + live events). Structure: five opt-in crates -
**`ui-test-common`** (lib) + **`ui-test-{qt,gtk,iced,egui}`** (bins) - added as workspace members but
**excluded from `default-members`** so a plain `cargo build` never pulls a GUI stack. Build one to a
polished baseline, refine, then copy the design to the rest. **`ui-test-iced` is the reference
implementation** (retained-mode Elm architecture - ports cleanly to gtk4/Qt, both retained-mode). The
design below is the **baseline for the tests and for the future real UI (5)**.

**Navigation model (adopted from the Steam Deck configurator - NOT the old visual-controller one).**
Four window-width regions in the widget hierarchy: **top daemon bar** / **left sidebar** / **scrollable
content** / **bottom status bar**. The sidebar is a **category list** that swaps the content pane
entirely; the content scrolls within the window. Sidebar layout: **per-input-type categories up top**
(Buttons/Triggers/Joysticks/Trackpads/Gyro - the profile-edit screens), **application-level entries
pinned at the bottom** (Settings, Globals; later profile load/select, maybe daemon mgmt). This same
list-swaps-content model serves **both** the application shell and, within it, profile editing.

**The four regions.**
- **Top bar (daemon management):** Start / Stop / Connect (enable driven by run-state), a manual
  **Refresh** (no USB-hotplug push - this re-enumerates), and **input/output pickers** on the right
  (input = static presets `auto|dongle|wired|bt` **+** the daemon's live `list-devices`; output =
  presets). Buttons left, pickers right.
- **Left sidebar:** the category nav (above).
- **Content:** the selected screen, scrollable. Per-input-type screens follow the Steam-Deck **list**
  model - grouped rows (Face Buttons / Bumpers / D-Pad / ...), a **group "Behavior" picker**, and a
  **per-input gear** that drills into that input's detail. (In the test, Buttons is a mockup of this;
  the rest are stubs.)
- **Bottom status bar:** live engine status - connection, run-state, bound device, Main/Fallback
  program names, chord count (everything from `StatusInfo` except input/output, which live up top).

**Application Settings (distinct from the daemon's config).** RON-persisted, in the UI's own file
(`$XDG_CONFIG_HOME/deckhand/ui-test.ron`), NOT the daemon: start-the-daemon-on-launch, load Main/
Fallback on start + their profile paths (each path greyed when its toggle is off; native file picker
via `rfd`), and the **theme stored by name** (toolkit-independent - each toolkit maps the name to one
of its own built-in themes, falling back to its default). Globals is a read-only widget preview for
now (item deferred).

**Daemon wiring (the architecture that matters - daemon-mode UI, 4.4).** The UI links `config` +
`ipc`, **never `engine`**; it ships `ConfigDoc` and the daemon compiles. Connection model:
- **One persistent event-stream (monitor) connection + short-lived per-command connections.** This
  works cleanly because the daemon is **thread-per-connection** (4.4) - the persistent monitor and
  the occasional command connection are served concurrently.
- **Commands run off the render thread** (each toolkit in its own idiom - iced `Task::perform`, gtk
  glib async, Qt signals/`QtConcurrent`, egui worker-thread + repaint), so a slow `start()` (e.g.
  dongle present, controller off) **never freezes the UI**.
- **Seed once, then deltas.** On (re)connect: fetch one full `Status` snapshot + `list-devices` to
  seed; thereafter the **absolute-valued event stream** carries every change, applied as in-place
  deltas to the cached status - **no per-event refetch**. Devices have no hotplug event (by design,
  4.3), so the manual **Refresh** + the on-connect seed cover topology.

**Shared vs per-toolkit (what `ui-test-common` owns).** Everything toolkit-independent, so each view
layer stays thin: **`AppSettings`** (+ RON load/save), a blocking **`Client`** over `ipc` (reconnects
on a dropped socket), **`run_event_loop`** (a resilient blocking subscribe loop each toolkit bridges
into its own event pump), the **`Category`** nav enum, and the input/output presets. Each `ui-test-*`
crate owns **only** its widget layer + the dispatch glue (running blocking calls off its render thread
and draining events) - and that glue *is* part of what the bake-off compares.

### 5.2 Things learned per toolkit (feeds the final choice)

Running notes as each `ui-test-*` is built - impressions that matter for a **long-lived, config-heavy
app**, not just the toy slice. Updated as toolkits are added; the choice is made once all four exist.

**iced 0.14 (BUILT - reference).**
- **Shape:** retained-mode, Elm architecture - explicit `State` / `Message` / `update` / `view`, split
  cleanly across files. More boilerplate up front; **structure scales well** and forces a tidy state
  model. This is why it's the reference: it ports almost mechanically to the other retained-mode
  toolkits (gtk4/Qt).
- **Async is first-class:** `Task::perform` runs blocking work off the render thread and feeds the
  result back as a `Message`; subscriptions bridge the event stream. The threading story is **hidden
  and clean** - no manual repaint/wake.
- **Theming:** ~22 built-in themes (`Theme::ALL`), each `Display`-named - trivially wired.
- **Widgets:** `pick_list`/`checkbox`/`text_input`/`scrollable`/`container` all present and composable;
  helper-fn API (`row!`/`column!` macros) reads well. Styling via theme presets + per-widget closures.
- **Friction:** a few type-inference/HRTB papercuts (closure annotations for `theme`/subscription
  builders; boxing the subscription stream). The functional builder API means **deep widget trees get
  verbose**. Font fallback rendered all glyphs (dot / warning / refresh / gear) fine.
- **Verdict so far:** the safe, structured choice; best fit for a large configurator that will grow.

**egui 0.36 / eframe (BUILT).**
- **Shape:** immediate-mode - one file, state + drawing interleaved, redraw every frame, **no
  message/task loop**. Fastest to stand up a screen; **less structure as it grows** (mutation happens
  mid-layout). Being immediate-mode it's the odd one out - a refined egui design does *not* port
  directly to the retained toolkits.
- **Async is fully manual:** no `Task` - spawn a thread per command + the event monitor, push results
  over an `mpsc` channel, and call `ctx.request_repaint()` to wake the UI, which drains the channel
  each frame. Works well and `ui-test-common` carried over unchanged, but the plumbing iced hides is
  **yours to write** here.
- **Theming:** only built-in Dark/Light - that picker is two entries (vs iced's list).
- **Widgets:** everything needed is there, but the **API churned noticeably** vs iced - panels unified
  into one `Panel` type, `SelectableLabel` removed (-> `Button::selectable`), `show_inside`->`show`.
  More breaking renames to absorb across versions matters for a long-lived app.
- **Friction:** `&mut self` inside immediate-mode closures fights the borrow checker (collect a choice
  into a local, act after) - a recurring papercut iced's message model sidesteps. Right-alignment is
  manual (`Layout::right_to_left`). Reactive repaint (eframe only redraws on interaction/`request_
  repaint`), so no idle CPU spin.
- **Verdict so far:** great for quick/lightweight or highly-dynamic UIs; the immediate-mode model and
  API churn are the concerns for a big, long-lived configurator.

**cxx-qt 0.9 / Qt 6 (BUILT).**
- **Shape:** not a Rust-native toolkit - the UI is **QML** (`qml/main.qml`) backed by a Rust `Bridge`
  QObject (`#[cxx_qt::bridge]`). A `build.rs` runs `cxx-qt-build` (moc + a C++ compile, ~45-60 s
  rebuilds). Two languages: declarative QML for layout + Rust for logic. Powerful and *very* mature
  widget set (Qt Quick Controls, Material/Universal styles, native dialogs), but the most moving parts
  by far.
- **Async / threading:** blocking work runs on std threads; results are marshaled back onto the Qt
  thread with **`CxxQtThread::queue`** (the only safe way to touch a QObject off-thread). Clean once
  understood; `ui-test-common` carried over unchanged.
- **Bridge boilerplate:** every value QML reads is a `#[qproperty]`; every action an `#[qinvokable]`.
  Qt types (`QString`, `QStringList`, ...) must be **declared inside the bridge** (`extern "C++"` +
  `cxx-qt-lib/....h`). By default Rust `snake_case` stays snake_case in QML - add **`#[auto_cxx_name]`**
  for idiomatic camelCase. Property/invokable count balloons for a config-heavy UI.
- **Theming:** the richest - all Qt Quick Controls styles; Material `theme` Dark/Light switched live
  from QML.
- **Friction:** the QML/Rust split means errors surface at **runtime** in QML (undefined property /
  "not a function") rather than at compile time - the weakest safety of the four. Heavy build (C++/
  moc). Needs a **C++ Qt install** (see below), which dominates the Windows/packaging story.
- **Verdict so far:** the most capable and polished UI, but the heaviest to build/ship and the least
  Rust-safe (runtime QML errors, two languages, native Qt dependency). Its strength (QML + Controls)
  is also its cost.

**gtk4 0.11 / GTK 4 (BUILT).**
- **Shape:** retained-mode, GObject/C-backed (auto-generated `gtk-rs` bindings). Build widgets once,
  hold handles, mutate on events - imperative, no `view` fn. Mature, complete widget set with real
  accessibility and native desktop integration; everything is a refcounted GObject (clone handles
  freely).
- **Async / threading:** the **glib main loop**. Off-thread work posts over an **`async-channel`**
  drained by `glib::spawn_future_local`; widgets are mutated on the main thread. Clean;
  `ui-test-common` carried over unchanged.
- **Boilerplate:** the heaviest closure/clone dance of the four - an `Rc<Ui>` cloned into every signal
  handler, `RefCell` for shared state, and a **suppress-guard** so programmatic dropdown updates
  aren't taken as user picks. `glib::clone!` helps but it's verbose.
- **Theming:** real GTK themes, but runtime *app* control is limited to the **light/dark preference**
  (no per-app palette like iced/Qt) - it follows the desktop theme. **libadwaita** would add the
  adaptive/GNOME look but is impractical on Windows, so this build is pure gtk4.
- **Friction:** a **native GTK4 dependency** (pkg-config) that dominates the Windows story (gvsbuild/
  MSYS2 + bundled DLLs). Widgets are runtime-typed (`upcast`/`downcast`), so some mistakes surface at
  runtime. The Rc/RefCell + GObject-signal discipline is a constant low-level tax.
- **Verdict so far:** the most "native desktop" feel + best accessibility, but the heaviest Windows
  deploy of the Rust-driven options and the most closure/clone ceremony. A Linux-first choice.

**Native libraries & Windows packaging (investigated for all four).** Decisive for a cross-platform
(Linux **and** Windows) app. No toolkit here auto-downloads its native deps in a way we can rely on.
- **iced / egui (Rust-native):** **nothing** to install on Windows. They bundle their renderer
  (wgpu/glow) + windowing (winit); only GPU drivers needed. `rfd` file dialogs are native. Default
  `x86_64-pc-windows-msvc` toolchain, done. **This is a major point in their favor.**
- **cxx-qt / Qt:** needs a real **Qt 6 install** at build time - `qt-build-utils` finds it via the
  **`QMAKE` env var -> else `qmake` in PATH**; **no auto-download** (an experimental `qt_minimal`
  downloader exists in `qt-build-utils` but cxx-qt-build 0.9 doesn't expose it, and "minimal" lacks Qt
  Quick - unusable here). **Windows:** install Qt 6 (Qt online installer / `aqtinstall` / vcpkg), use
  the **MSVC** build to match Rust's MSVC target (cxx compiles C++ - don't mix with MinGW Qt unless
  switching to the GNU Rust target); point `QMAKE` at `qmake.exe` or put Qt `bin` on PATH. **Runtime:**
  bundle Qt DLLs + the QtQuick/Controls/Material QML plugins with **`windeployqt`**. The app *code* is
  platform-agnostic; only the toolchain/deploy differs. (On the Gentoo dev box Qt 6.11 is installed;
  build with `QMAKE=qmake6`.)
- **gtk4 (gtk4-rs):** links native GTK4 via **pkg-config / `system-deps`**; **no auto-download**.
  **Windows:** either **gvsbuild** (builds GTK4 for MSVC -> MSVC Rust target, set `PKG_CONFIG_PATH` +
  PATH) or **MSYS2** (`pacman -S mingw-w64-x86_64-gtk4` -> **GNU** Rust target); gvsbuild is the more
  shippable. **libadwaita on Windows** is GNOME-oriented and painful to package -> expect **pure gtk4**
  there. **Runtime:** bundle GTK DLLs + loaders/schemas - the heaviest deploy of the four.
- **Bottom line:** to ship on Windows - iced/egui = zero native setup; Qt = install Qt (MSVC) +
  `windeployqt`; gtk4 = gvsbuild + bundle. The Rust-native pair is *dramatically* simpler to
  cross-target, which weighs heavily for a tool meant to run on both Linux and Windows.

**Summary / leaning (all four BUILT - input for the decision, which is the user's).** Every toolkit
rendered the same slice fine and reused `ui-test-common` unchanged; the split is architecture,
Rust-safety, and shipping - not looks.
- **iced** - front-runner for a large, long-lived, cross-platform configurator: structured retained
  (Elm) model that scales, **compile-time-safe**, first-class async, ~22 themes, and **zero native
  deps on Windows**. Cost: verbose builder trees, some HRTB papercuts.
- **egui** - strong for quick/lightweight or highly-dynamic UIs; also zero Windows native deps. The
  immediate-mode model (state mutated mid-layout, more borrow-checker friction) and the heavier **API
  churn** are the reservations for a big app.
- **cxx-qt / Qt** - the most capable + polished (QML + Controls), but the heaviest and least Rust-safe:
  two languages, **runtime** QML errors, a C++/moc build, and a native **Qt install + `windeployqt`**
  to ship on Windows.
- **gtk4** - the most native-desktop feel + accessibility, but Linux-first: most closure/clone
  ceremony, and the **worst Windows deploy** (gvsbuild + bundled DLLs, no libadwaita).
- **Where it points:** the two Rust-native toolkits dominate on cross-platform simplicity and
  compile-time safety; between them, **iced** for a UI meant to grow, **egui** if speed-to-build or
  a highly-dynamic UI matters more. Qt/gtk win on widget maturity but pay for it in native
  dependencies + Windows packaging.
- **DECISION (2026-08-09): iced**, pending a confirming Windows build. Chosen for zero-native-dep
  cross-platform shipping, compile-time safety, the structured retained model that scales for a
  config-heavy configurator, and being the most polished of the four prototypes (`ui-test-iced` is
  the reference to promote into the real `ui` crate). egui is the fallback if the app stays small/
  dynamic. Tray is a `tray-icon` companion crate regardless (5).

### 5.3 Profile editor (design record - FEATURE-COMPLETE 2026-08-15)

The in-app editor for a `ConfigDoc` profile: action sets, layers, per-input bindings, behaviours,
commands, settings, gaters, and global chords. Built over many sessions (2026-08-13 -> 2026-08-15).
Per the "UI changes are NOT documented" convention this was the **one documented UI area**. This
section is the **durable capture of that design + the knowledge gathered building it** so nothing is
lost - decisions and
rationale, NOT code shape (the code is read directly). Where a decision was labelled A-F in the
working notes, that letter is kept here as the stable reference.

**Status.** Feature-complete for the owner's needs on Linux: Profile page (sets/layers), the five
input pages (commands/activators/subcommands), command settings, the per-behaviour settings
framework, rumble, gaters + global chords over all hardware buttons. Remaining work is
**optional/deferred, not blocking**: device greying (decision D) and Windows compile-verification of
the win-only paths (virt-out axis buttons, `ico`/`png` build step). Future sessions are expected to
be **styling/finetuning** (undocumented) plus those deferrals if ever wanted.

**Module layout (where the code lives).** The editor is split into a mutation side and a view side:
`editor/mod.rs` (the doc-mutation seam + semantic types), `editor/authoring.rs` (authoring
defaults), `editor/settings.rs` (the settings edit path - `apply_setting` + `&mut` accessors);
`view/editor/mod.rs` (editor chrome + per-screen views), `view/editor/settings.rs` (settings
forms/blocks), `view/modal/{mod,action,buttons}.rs` (the modal shell + the two pickers),
`view/{device,chords,profiles,settings}.rs` (the old `view/globals.rs` became the `device` + `chords`
pages in the 2026-08-19 split), and `nav.rs` (category->input-group mapping). The
settings subsystem is deliberately split into its own file on **each** side, next to (not inside)
the respective `mod.rs`, mirroring `editor/authoring.rs`.

**Mutation discipline (the load-bearing invariant).** `editor::update` is the **single
doc-mutation site** - every change to the `ConfigDoc` funnels through it. This is what makes undo
(deferred) cheap to add later (snapshot a `Vec<ConfigDoc>` there + a Ctrl-Z message) and keeps the
Elm model coherent. Keep that discipline. Settings edits reach it through `editor::settings::apply_setting`.

**Navigation & category layout (decision C - Steam-style, static).** Sub-buttons (clicks/touches,
trigger full-pulls) nest under their **parent input's** page (pad click/touch on the Trackpads
page, etc.), never a generic Buttons page. A rich-source page = header (source name) + behaviour
selector + its sub-buttons. Mapping is encoded statically in `nav` (`nav::Category`->input-group).

**Addressing sets/layers - `EditTarget` is name-based, not index-based.** `EditTarget { set: String,
layer: Option<String> }`. Set names are unique among sets; layer names unique among a set's own
siblings (both enforced by the editor's uniqueness checks), so the pair is a **stable key that
survives reorder/removal** - an index would silently shift. Used two ways with independent values:
the sidebar's selected target (`Editing::target`) and the identity each Profile-page bar's gear acts
on (built from that bar's own name). Renaming a set/layer also **repoints action refs**
(`ChangeActionSet` globally; `Hold/Add/RemoveLayer` per-set).

**Sidebar selector scopes the input pages.** The sidebar action-set/layer **carousel selector**
sets `EditTarget`; the five input editor pages edit **that** set/layer's bindings. The Profile page
lists action sets with their layers **nested/indented** beneath (Steam-style); set/layer
create/rename/delete via per-bar **gear context menus** + unique-name dialogs; new profiles = one
`Default` set, no layers; Duplicate/Create-new open for editing via `load_for_editing`. The Profile
page management and the selector address the **same** set/layer via `EditTarget`. A set's gear hosts
"Add layer".

**Modal system - one shared `Popup` enum.** The old network `Popup` struct became a single enum (the
one modal shown at a time; backdrop click-away = generic `PopupCancel`): `Network | Menu | NameEntry
| ActionPicker | CommandMenu | SlotMenu | ButtonPicker`. **All modals live in `view::modal`** (the
shell, the `Popup` enum, every card); the *semantic* types (`EditTarget`, `NameEntryKind`,
`CommandRef`, `ActionTarget`, ...) stay in `editor`.

**The two pickers.**
- **Action picker** (`view::modal::action`, decision F): a fixed-size (840x520) **tabbed** modal
  (Gamepad / Mouse / Keyboard / Numpad / Action Sets) returning one `config::Action`,
  **immediate-confirm on click** (no OK). Gamepad's LT/RT + the eight stick directions are enabled
  (they emit the axis pseudo-buttons - vocab-out). Keyboard/Numpad cover all 127 `Key`s via an
  auto-complementing "Other keys" grid. Action Sets uses `pick_list` scoped to the **edited set's**
  layers. It writes to an `ActionTarget` stored in the popup. Resolved as tabbed-by-category, not a
  search field.
- **Button picker** (`view::modal::buttons`): single-select chooser of **all 28 raw
  `vocab_hid::Button` hardware bits** in grouped rows (face A/B/X/Y, dpad, bumpers, full-triggers,
  grips, system, stick/pad press **and** touch). `Popup::ButtonPicker` carries a `ButtonTarget`
  (`Gater(input)` / `Chord(i)`); `ButtonPicked(Button)` routes the pick to the gater edit
  (`SetSetting -> AddGater`) or the chord (`app.chords.chords[i].buttons`).
- Vocab labels (`action_label`/`key_label`/`button_label`/...) are **UI-owned** (both vocab crates
  stay presentation-free); gamepad labels are fully descriptive (no `LB`/`L3`).

**Input-page wiring (the five per-input pages).** Each renders the selected set/layer's
`BTreeMap<InputSource, SourceBinding>` (`editor::current_bindings`). Rich sources + button groups get
a **behaviour combobox** (`Behavior::valid_for` -> `default_binding`; `None`/`Unbound` = remove the
map entry). **Switching behaviour rebuilds from authoring defaults and discards the old binding's
commands** (accepted - no merge). **Plain buttons + sub-buttons have NO combobox** - the command bar
*is* the `SourceBinding::Button` (created on first action). Which virtual buttons a behaviour exposes
= `virtual_slots`; group members = `group_member_slots`.

**Command model (multi-command, subcommands, gear menus).** A slot is a `Vec<Command>`; a `Command`
= activator + `actions` (main = `actions[0]`, **subcommands** = `actions[1..]`, an ordered
press-in-order combo). Layout: **0** = `<unbound>` (inert gear); **1** = one command bar (main action
button + gear) with its subcommands double-indented; **>=2** = a top **slot bar** (label + gear, no
action button) over one indented "Command (activator)" bar each - every command bar keeps its own
action button, only the top slot bar drops it. Non-Regular activators show a blue "(Long press: 450
ms)" suffix; every action (main or sub) is click-to-repick; subcommand bars have a x remove, no menu.
Addressing = `CommandRef { input, slot, index }` + an `ActionTarget` enum (`Replace { cmd, action }`
/ `AddCommand { input, slot }` / `AddSubCommand { cmd }`) the picker writes on confirm.
`ActivatorKind` (UI enum) drives the activator combobox (Long = 450 ms, Double = 200 ms defaults;
menu stays open on change). Gear menus: **command menu** (Remove / activator / Settings -> the command
settings sub-page / Add sub command / + Add extra command **only when sole**); **slot menu** (Remove
all / Add extra). Remove-all drops a plain-`Button` entry from the map but only **empties** a rich
binding's slot vector (the behaviour persists). **Duplicate activators are allowed** (two Regulars =
a key + a gamepad button on one press - deliberate; no "offer only unused activators" guard).

**One button-label source (`view::slot_display`).** `slot_display(input, slot) -> (&str, Dot)` is the
**single** place a button's name + Xbox glyph-colour dot is decided (Y=yellow / A=green / X=blue /
B=red; dpad/virtual/plain names), rendered by the shared `view::label_row`. Used by both the command
bars **and** the gear-menu titles (so a menu shows the same "[dot] Y Button" as its bar). A future
glyph/colour/name change is one edit here.

**Behaviour three-state per layer: something / Inherited / Disabled (BUILT).** Per-`InputSource` a
layer has three states - **something** / **no entry** (Inherited, falls through) / explicit
`SourceBinding::None` (Disabled, nullifies the base). `Behavior` (UI enum, drift-guarded by `of()`)
has a `Disabled` variant: `of(SourceBinding::None) = Disabled`; a *missing* entry is the caller's
`Unbound` fallback. `valid_for(kind, on_layer)` - a layer adds `[..., Disabled, Inherited]` (Inherited
= the relabeled `Unbound`, always last). `SetBehavior`: `Unbound`->remove entry, `Disabled`->insert
`None`, real->`default_binding`.
- **Rich sources + button groups** carry all three via the combobox (Inherited dims the "Behavior"
  label = passthrough; Disabled reads normal; gear inert for both).
- **Standalone `SourceKind::Button` inputs** carry it via the **bar**: `<inherited>` (dim label) /
  `<disabled>` (normal), both clickable to add a command (so `apply_action::AddCommand` replaces a
  `None`); the gear opens a small `LayerButtonMenu` -> **Disable** (inherited) or **Remove** (disabled
  -> drops the `None`). **No `Inherit`** - Remove goes to no-entry. `on_layer` threads through the
  input-page view fns; `editor::on_layer(app)` feeds the menus.
- **Menu order (all contexts):** command menu leads with **Remove** (above the activator); slot menu
  is **Remove all**; **Disable** sits at the very top for a top-level Button on a layer. The
  Profile-page set/layer context menu likewise leads with **Remove** (above Rename).

**Settings pages are a full-page sub-route, NOT a modal (paradigm, decision 2026-08-14).** Every
settings page (command settings + per-behaviour settings, same shape) renders **instead of** the
current input page via `content()` (`view/editor::settings_screen` short-circuits the category
dispatch), reached from a gear menu, left via a **< Back** button. Chosen over a modal so the pattern
scales to the larger behaviour-settings forms, reuses the (scroll-fixed) content pane full-width, and
keeps "which command/behaviour" as navigation state. State = `Editing::settings: Option<SettingsView>`
(enum: `Command(CommandRef)` | `Behavior(InputSource)`); **cleared by ANY navigation**
(`Message::Navigate` and the target selector's `step_target`) so a `CommandRef`/behaviour ref never
outlives its set/layer - the page doesn't change `app.category`, so Back falls back to the input page
the sidebar still highlights. Form style = **plain label/control/readout rows** (the Globals/Rumble
aesthetic, `SET_LABEL`=160), NOT `card()` bars. **Sliders: `.step()`-snapped + a right-hand value
readout** (iced has NO tick marks; snapping is free, notches would need a custom widget - declined).

**Command settings contents** (all autosave through `editor::update`, applicability-gated per
decision B): the activator **kind combobox** (duplicated from the gear menu - deliberate, so the page
is the command's complete home) + its **additional-settings row** (`activator_additional_settings`:
Long `hold_ms` 100-2000 step 50 / Double `window_ms` 100-1000 step 25 / Regular **Interruptible**
checkbox - the activator's own parameter, one per kind); **Toggle**; **Turbo** (checkbox gates a
20-500 ms step-10 rate slider, seeded `DEFAULT_TURBO_INTERVAL_MS`=100; hidden on Release); **Haptics**
edge (Off/OnPress/OnRelease/Both) + **Strength** (Low/Med/High, shown only when the edge is on).
Haptic edge/strength labels are UI-owned (`config` stays presentation-free).

**Per-behaviour settings = a building-block framework (BUILT 2026-08-14).** Not one bespoke page per
behaviour - a **reusable block per setting element** (deadzone, sensitivity, curve, smoothing,
acceleration, invert, rotation, outer_ring, soft_pull, output, layout, space, activation) composed by
a small per-behaviour fn in the **canonical field order**. Shared **view AND logic**: (1) blocks emit
**one generic `EditorMessage::SetSetting(InputSource, SettingEdit)`** where `SettingEdit` is a **sum
type, one variant per field** (NOT a god-struct - the typed `config` settings structs stay the source
of truth); (2) `editor::settings::apply_setting` writes that one edit via small `&mut` accessors over
`SourceBinding` (`deadzone_mut`/`sensitivity_mut`/... -> `Option<&mut T>`), so mutation isn't duplicated
across the six behaviours. **Adding a behaviour = one compose fn** from existing blocks (+ any new
block). The behaviour-row **gear** opens the page iff `Behavior::has_settings()` (true for the six
analog behaviours; **false for Button/ButtonPad** and the pseudo-behaviours - governs ONLY the
behaviour-row gear; a plain Button's gear is a command menu, untouched). Curve = a Linear/Power kind
picker + an exponent slider when Power. Smoothing = an enable checkbox gating min_cutoff/beta.
**Activation = the mode picker + the gater set editor.**

**Button-set editor = chips + the single-select Button picker (gaters & chords share it).** Both
`Activation.gaters` and `Chord.buttons` are `Vec<vocab_hid::Button>` (raw controller bits - any
button incl. face buttons and dpad directions, so `Steam + A` / face+dpad gaters work), so one shared
widget edits both: `view::button_chips(buttons, on_remove, add)` renders each button as a removable
**chip** (`view::chip`, `style::chip`) with a trailing **+** that opens the Button picker
(`OpenButtonPicker(ButtonTarget)`); the pick appends (deduped for gaters). Chosen over a
bar-per-button (too heavy) and over a new multiselect modal (reuses the existing picker; upgradeable
to multiselect later without touching the chips). **Gaters** live on the behaviour Activation block
(add via `SetSetting::AddGater`, remove `RemoveGater`). **Chords** live on the **Chords page** (split
out from the old Globals page in the 2026-08-19 split, now separate from the Device page): one
**card bar per chord** - the trigger chips, an action-kind combobox (`ChordActionKind` Switch-profile
/ Run-command), its detail (SwitchMode picker, or a **single command-line text field**), and a x to
delete; an "Add chord" button below. Chord edits mutate `app.chords` and go through `apply_chords`
(persist + push; `chords::to_push` collapses an empty set to `None`). **Command-line round-trip:** the one field is split on the literal `' '` (keeping
empties) into `command` + `args` and rejoined with `' '`, so the field's exact text (incl. trailing
spaces) survives per-keystroke parsing without cursor mangling - cost is transient empty args while a
trailing space exists.

**Canonical behaviour-settings field order (config structs + settings pages MUST match).** `output` ->
**behaviour-specific** (layout/space/soft_pull/outer_ring, at position 2 right after output) ->
`sensitivity` -> `acceleration`/`curve` -> `smoothing` -> `deadzones` (deadzone, anti_deadzone) ->
`invert` -> `rotation` -> `activation`. Applied to the `config::settings` struct **definitions** (which
drive RON field order), the `example_profiles`/engine-test literals, and the compose fns. When
adding/reordering a settings field, keep struct def + compose fn + any literal in this order.

**`interruptible` lives on the activator, defaults on.** Moved off `CommandSettings` onto
`Activator::Regular { interruptible: bool }` (it was only ever meaningful on Regular -> invalid combos
now unrepresentable, and the engine's runtime `matches!(Regular)` guard is gone). RON:
`Regular(interruptible: false)`. New Regulars are born **interruptible** (the authoring default),
single-sourced in `ActivatorKind::to_activator` (`editor::apply_action` routes through it): a no-op
when the Regular is the node's only command (no `Long`/`Double` -> the engine's `has_interrupter` gate
makes it a plain press/release hold) and the intended short-vs-long behaviour the moment a
`Long`/`Double` is added - safe under the reworked engine activator model (3/4). Editor:
`SetInterruptible` rewrites the whole activator like `SetHoldMs`/`SetWindowMs`; the checkbox lives in
`activator_additional_settings`.

**Ownership line (firm).** `config` = data model + serde + **validation**. The **UI owns ALL
authoring/construction** of config values (that's what an editor is). So empty-profile creation,
authoring defaults, and the `Behavior` tag all live in the UI - `config` gained nothing for the
editor and shouldn't. (A `ConfigDoc::new_empty()` was reverted for exactly this reason.)

**Two "default" layers - keep distinct.** serde `Default` (in `config`) is the **neutral/minimal
on-disk fallback** (0/None/whatever's safe) - untouched. **Authoring defaults**
(`Behavior::default_binding`, in the UI) are the **context-tuned starting values** a freshly-picked
behaviour gets. Authoring defaults are UI-only because the *only* moment a tuned default is needed is
when the user picks a behaviour in the editor - no `ctl`/test/engine path needs them.

**UI slider ranges: per-behaviour, NOT per-input, and NO table.** They live inline in each behaviour's
settings-view function. Rationale: the only knobs that differ are velocity-signal ones (accel
`factor`, 1-Euro `min_cutoff`/`beta`) and the difference is **pad-vs-gyro = AsMouse-vs-GyroToMouse =
per-*behaviour*** (distinct settings types), not per-input. It's small in practice (gyro accel `0.02`
vs pad `~0.05`, `min_cutoff` `1` vs `3`, `beta` identical `0.5`) because the engine's per-behaviour
gain constants already absorb the deg/s-vs-pad-units/s scale gap. A single shared range per field is
fine; per-behaviour narrowing is a free later block param.

**Save model + undo (decision A).** **Autosave on edit** (no dirty flag - matches Steam, small files,
closed option set). **No undo for now**, deferred; safe to add later *because* every doc mutation
funnels through `editor::update` (keep that discipline) + the Elm model -> later a `Vec<ConfigDoc>`
history snapshotted there + a Ctrl-Z message.

**Validation UX (decision B).** The editor makes invalid states **unrepresentable** (offer only
behaviours valid for the source kind; gaters/chords are `vocab_hid::Button`s so "not a physical
button" is type-enforced, not validated). **Valid-but-no-effect is allowed** (e.g. `HoldToEnable`
with an empty gater - legal to the mapper). `config::validate()` stays the **apply-time backstop**.
No live-diagnostics panel.

**Two "Set as Main/Fallback" paths are intentional (decision E) - do NOT merge them.** The **top-bar**
buttons are editor-scoped: they send the **in-memory edited** profile, are shown on every editor page,
and are labelled by the profile name - for **edit-and-test-immediately**. The **Profiles-page** grid
sends the **selected on-disk file** - daemon management. Different workflows; merging harms the
test-while-editing flow.

**Device greying (decision D) - NOT built.** Grey inputs the connected device lacks
(`config::Shape::has`), behind a **toggleable UI setting**. When building it: cleanly knowing the
connected device's `Shape` means either parsing the `bound` id prefix (`gordon:`/`neptune:` -
interim/stringly) **or** adding kind/`Shape` to `StatusSnapshot` (a protocol/engine change =
functional, deliberately out of scope for the editor build). Decide then.

### 5.4 Forwarder UI (`forwarder-ui` - prototype, 2026-08-19)

A **second, deliberately minimal UI crate** (iced; bin `deckhand-forwarder`), built as a
**prototype** - the throwaway question below is unresolved.

**What it is.** A single-screen, touch-first "quick connect" tool: pick a local controller input,
type an `ip:port` output on an **on-screen numeric keypad** (3x4 phone layout `1-9`/`. 0 :` + a
backspace; every key edits the field regardless of focus), set a master-rumble slider, press Start.
No tray, no sidebar, no profiles/chords, no editor. **Fixed daemon policy** (no settings screen):
launch-the-daemon-if-absent (spawned with `--prevent-sleep` so the Deck doesn't idle-sleep while
forwarding), restore the last **input only**, push the device config, **never auto-start** (Start is
always manual; the output is set from the text field **verbatim at Start**, not on edit). Whole UI
drawn at a global **1.75x scale** (the Deck's touch display makes default widgets too small); the
window opens near Deck-native (1280x800) and the content pane scrolls if the scaled layout overflows.

**What it's for.** The **Steam Deck** - its touch screen, forwarding controller input to a PC over
the LAN (the 6 client/forwarder role). On a **normal PC** the full `deckhand` UI already does the
client job (it can `set_output(host:port)`), so the forwarder exists **only** for the Deck's
touch-driven quick-connect case, where the full UI is awkward.

**Construction.** A **deliberate duplicate** of the parts of `deckhand` it reuses - the daemon
client + resilient connect loop (trimmed to the fixed policy), RON persistence, a few styles -
**copied, not shared** (no common crate). Own settings file `forwarder.ron`
(`theme`/`window_{width,height}`/`last_input`/`last_output_network`); **shares `devcfg.ron`** with
the main UI (loads the full struct, mutates only the bound device's rumble, preserves the rest).
Reacts to the
daemon event stream for what it displays (state / controller / bound device / input); the output
field is user-owned and never overwritten by an event. Workspace member but **NOT a
`default-member`** (heavy iced tree - build with `-p forwarder-ui`). Linux install is opt-in:
`install.sh -f/--forwarder` adds the binary + icon + `.desktop` + a `~/Desktop` launcher symlink.

**Open question (unresolved).** It's unclear the forwarder is worth keeping. Options when revisited:
(a) drop it if the main UI grows a good enough touch/quick-connect mode; (b) keep it standalone;
(c) factor the duplicated daemon/persistence/style code into a shared crate the two UIs import.
Deferred deliberately - treat the crate as **throwaway** until this is decided; per the "UI changes
NOT documented" rule its internals live in the code (`crates/forwarder-ui/src/main.rs`), not here.

## 6. NETWORKED / REMOTE CONTROLLER (future capability)

**Goal.** Use a Steam Deck (or any machine with a Steam controller) as a controller for a
*different* computer over the network - Deck in hand, mapping + virtual output on the
gaming PC. Low-latency, LAN-first.

**Feasibility.** Very doable - the Steam Remote Play / Steam Link / Moonlight input
pattern. On a LAN, UDP input latency is sub-ms to a few ms, well within what games
tolerate. It also **validates the ports-and-adapters split**: the network is just another
adapter around the pure engine.

**Architecture - one engine, network as an adapter** (refinement of the original "two
engines communicating" idea). You want a *single* mapping engine, not two. The pipeline
`steam-hid -> engine -> virt-out` is simply cut at one seam and stretched over the wire.
Two app **roles**:
- **Source role** (on the Deck): `steam-hid` + a network **sender**. Reads
  `ControllerState`, ships it over UDP. Minimal, "dumb" end.
- **Sink role** (on the gaming PC): network **receiver** -> `engine` -> `virt-out`. The
  mapping engine and the `config` live here (natural - games are configured on the PC).

The net sender/receiver are peers of the local adapters: a **network input source** (peer
of `steam-hid`) feeds the engine exactly like a local device would. (So "two engines" is
really two app *roles*; the engine logic runs only on the sink side.)

**Recommended seam: send raw `ControllerState`** (cut at input->engine). State is small and
fixed-size, config/engine stay centralized on the PC, and the Deck end stays trivial. The
seam is flexible - you *could* run the engine on the Deck and send output events instead -
but state-forwarding is the cleaner default.

**Bidirectional.** State flows Deck->PC; **commands flow back** PC->Deck - rumble/haptics
(the 2 back-channel becomes a network round-trip) and any config the source needs.

**Transport: UDP.**
- **Forward (state): unreliable UDP, latest-wins.** A dropped `ControllerState` is
  harmless - the next packet supersedes it a few ms later. Use the report `seq` (u32) to
  drop stale/out-of-order packets. No TCP (retransmit stalls add latency /
  head-of-line blocking to a continuous stream).
- **Backward (commands): light reliability.** Rumble/config are events, not idempotent
  state -> sequence numbers + optional resend, or accept occasional rumble loss.
- Send rate matches the device (~250 Hz-1 kHz); tiny packets -> negligible bandwidth.

**Where the code lives.** Likely a small shared crate (`link` / `net`) holding the wire
format + UDP transport, used by both roles - or adapters split across `steam-hid` /
`virt-out`. Decided later. **Future capability**, tackled after the local pipeline works;
the crate split already makes it a natural extension, not a rewrite.

**Later concerns.** Pairing/auth (don't accept input from arbitrary hosts - LAN, so
minimal at first); no time-sync/prediction needed for direct input passthrough (unlike
video streaming); WiFi adds a few ms jitter vs wired.

### 6.1 Transport abstraction - the `CommChannel` seam (design discussion 2026-08-02, NOT built)

**Goal of this refinement.** Contain *all* the network complexity in one place so it does
not smear across `reader` / `mapping` / `runtime`. The fear is legitimate - a naive network
implementation would leak sockets, seq handling, reconnection, and serialization into the two
device-driving loops. The fix is to abstract the **inter-thread communication seam itself**,
not the transport mechanism via a heavy trait hierarchy. This is the same ports-and-adapters
move used everywhere else in the project, applied to the `reader`<->`mapping` seam.

**The abstraction.** Today the two threads talk over crossbeam channels (`frame`, `rumble`,
`click`, plus the `control` channel from the Engine handle / daemon). Wrap those channels in a
small type - **`CommChannel`** - with **two adapters chosen at `Runtime::start()`** (from the
staged `Input::Local` vs `Input::Network`): a **local** adapter (the channels *are* direct
crossbeam pairs - nearly free) and a **network** adapter (sockets + hidden bridge threads).
`reader` and `mapping` are rewritten to talk only to `CommChannel` and become **identical**
across local and network - the prize. No network concept leaks into their code; the only
network-specific surface is the constructor argument (`IP:PORT`).

**Two endpoint types, not one symmetric class.** The directions and method sets differ, and
they map cleanly onto the **already-locked client/server roles** (client = controller/Deck
side; server = output/PC side) - so name them accordingly:
- **`LinkClient`** - the **device side** (reader), always hosted where the controller is (the
  client): `frame_tx()`, `rumble_rx()`, `click_rx()`, `control_tx()`, plus `detach()` /
  `reattach()` lifecycle methods.
- **`LinkServer`** - the **mapper side** (mapping loop), always hosted where the output is (the
  server): `frame_rx()`, `rumble_tx()`, `click_tx()`, `control_rx()`.

This is not a new vocabulary - it's the same client/server split the docs already use, now
naming the types, and it checks out method-by-method: config/control flows client->server, so
`control_tx` sits on `LinkClient` (the reader's reattach path *and* the Deck's config uplink)
and `control_rx` on `LinkServer`; frames flow client->server, rumble/clicks server->client.

Each type is an `enum` internally (Local | Network) - runtime dispatch, matching "one class,
two implementations underneath, chosen at construction." No `dyn`, no trait zoo. **Locally there
is no network: both `LinkClient` and `LinkServer` are co-located in one process, wired by
crossbeams - a *loopback link*.** That's a feature, not a hack: it reinforces "one artifact,
role chosen at runtime" - the reader always talks to a `LinkClient`, the mapper always to a
`LinkServer`; whether the link is a crossbeam loopback or real sockets is the adapter's
business.

**Where it lives - in the engine, NOT a separate crate (decision 2026-08-02, supersedes the
"small `link`/`net` crate" note above).** The `Link{Client,Server}` types must live at the
engine seam because they carry engine types (`Program`, `Control`/`RumbleCmd`/`Click`, `Report`)
- so put the whole thing in **`engine/src/runtime/link.rs`** (beside `mod.rs`/`reader.rs`/
`mapping.rs`). A separate `link` crate is actually *awkward*, not just unnecessary: an engine-
type-aware crate would depend on `engine` while the engine's Network adapter depends on it ->
**dependency cycle**. The only cycle-free split is a **payload-generic transport** (`T:
Serialize` over UDP+`seq`+TCP+reconnect, knowing nothing about `Program`) - but that is small
and, crucially, has **no second consumer** (the network client is a full engine deployment;
deckhandctl doesn't network), so the old shared-crate rationale (from the pre-"one engine"
framing) no longer applies. If `link.rs` grows: first a `runtime/link/` submodule
(`mod`/`client`/`server`/`wire`), and *only* if the generic-transport part gets genuinely heavy,
extract a payload-generic crate later - never an engine-type-aware one. Matches the "refactor
freely, no premature abstraction" policy.

The reader's `control_tx()` is the crux of what must be abstracted: today (reader.rs:85) the
reader, on reacquiring a device, **mints fresh frame/rumble/click channels and hands the
mapper's ends over via `Control::Reattach`** so the pad never leaves. That channel-swap dance
becomes a `CommChannel` internal - the reader just calls `detach()` when the device is lost and
`reattach()` when it returns, and the type does whatever its adapter requires underneath.

**Key mechanism - expose crossbeam `Receiver`s, do NOT invent a `poll()`.** `CommChannel`
returns the actual crossbeam endpoints (`&Receiver<Report>`, `&Receiver<Control>`, ...). Both
adapters produce the *same* crossbeam endpoints:
- Local: `frame_rx()` is a plain crossbeam `Receiver` paired with the reader's `frame_tx()`.
- Network: `frame_rx()` is the receiving end of an **internal, in-process** crossbeam channel
  fed by the adapter's hidden socket threads - frames arrive over **both UDP and TCP** (the
  idempotency split below); `control_rx()` is fed by a hidden **TCP** thread. The crossbeams
  are always local (thread<->thread); there is no such thing as a "network crossbeam". The
  bridge threads are created and owned by the network adapter and are
  **completely invisible** to `reader`/`mapping`/`runtime` - those three never know the extra
  threads exist.

Because crossbeam `select!` operates on `&Receiver`, the mapping loop's existing
`select! { frame, control, tick }` composes with `CommChannel` **verbatim** - the mapper is
unchanged whether input is local or networked. The `tick` insurance arm stays a local timer
outside `CommChannel` (it provides two of the three arms; the timer is the third). This is
why no custom multiplexing `poll()` is needed: crossbeam already gives blocking-with-timeout.

**Wire contract = `Program` (decision B, 2026-08-02).** The engine's internal control channel
already carries `Program` (`Control::Apply { program: Box<Program> }`), and the Runtime **never
calls `compile()`** - `compile(&ConfigDoc) -> Program` is an *external* convenience the daemon
uses before `apply()`. So the network path ships **`Program`**, keeping the compiler external
(the client/Deck compiles `ConfigDoc -> Program` before the wire, symmetric with the local
daemon compiling before `apply()`) and keeping the engine core compile-free. `control_rx()`
then yields `Control::Apply{Program}` from **both** the local Engine handle and the remote TCP
feed with no compile step in the adapter - local and remote config apply *transparently*
through the same receiver. **This extends the 4.4 "client ships `ConfigDoc`, daemon compiles"
decision:** that still holds for the *local ipc* client (deckhandctl -> daemon, thin, no engine);
the *network* client (Deck forwarder) is a full engine deployment that compiles locally, so
compile diagnostics surface **on the client, where the user and the config live** - a bonus that
falls out for free.
- **Cost:** `Program` and its whole IR (`CompiledSet/Layer/Binding/Command/Action`,
  `SetId`/`LayerId`, the opaque `SourceMap`) must gain **serde** derives. Note the `Box` in
  `Control::Apply` does **not** imply serialization today - the local control channel is an
  in-process ownership *move* (a pointer handed between threads, zero bytes encoded); the `Box`
  only keeps the enum small / moves the big IR cheaply. The network wire needs real serde.
  `program.rs`'s "runtime state, never persisted" note becomes "transmitted over the 6 wire,
  still never written to disk" (`ConfigDoc` remains the sole on-disk form).

**Frames split by idempotency; both merge into one `frame_rx()`.** `Report` is
`State | Connected | Disconnected | Battery`:
- `State` snapshots -> **UDP** (idempotent, latest-wins, `seq`-drop, loss-tolerant).
- `Connected` / `Disconnected` / `Battery` -> the **reliable TCP** channel (events, not
  snapshots - must not be lost).

The network adapter merges both back into the single `frame_rx()`, so the mapper sees an
ordinary `Report` stream. **Adapter-internal detail:** a late UDP `State` can arrive *after* a
TCP `Disconnected` (separate channels, no cross-ordering) - the adapter gates UDP behind a
connected/epoch flag so a stray snapshot can't re-apply outputs after a release. Invisible to
the mapper.

**Lifecycle unification - "transport lost" is transport-lost.** The mapper's existing D5/D6
connected/waiting phase machine is reused verbatim for the network case. All of {local device
`Err`, local `Disconnected` value, network transport loss} drive the **same** transition into
the waiting phase -> `WaitingForDevice`; recovery (device reacquired *or* peer reconnected)
drives the reattach -> `Running`. `detach()` / `reattach()` express this per adapter, and *how*
each adapter implements them is fully internal: the local adapter drops the frame/rumble/click
channels and re-mints them (control survives in-process - an implementation detail, not an API
promise); the network adapter tears down the connection entirely and re-establishes it via TCP
reconnection + renegotiation. Same two methods, adapter-specific mechanics, identical
downstream state (the mapper's phase machine).

**Well-behaved server for free.** The mapper side's `control_rx()` is fed by **two** sources:
the local Engine handle (in-process, always alive) *and*, in the network adapter, the TCP
thread. On a full network partition the network feed dies but the local-handle feed survives ->
the server stays controllable (`stop`/`shutdown` work) while it sits in `WaitingForDevice`. The
client side is trivial on drop: retry the TCP reconnect, nothing to send or receive meanwhile.

**Shared runtime state beyond the channels.** `reader`/`mapping` today also share two
`Arc<AtomicBool>` (`running`, `waiting`) and an `EventSink` (`runtime/mod.rs`). Under the
two-runtimes-over-the-wire model these mostly dissolve rather than needing a new wire:
- **`running`** - stays **local, not a Link concern.** It is a *handle->own-threads* stop
  signal, shared today only because one `Runtime` owns both threads. Network mode has two
  independent runtimes, each with its own `running` (client stops its reader; server stops its
  mapper; a peer shutdown just looks like transport loss to the other side). Off the list.
- **`waiting`** - **already covered by `detach()`/`reattach()`; just expose it *through* the
  Link** instead of a raw shared bool. The mapper picks its phase and `status()` reports
  `WaitingForDevice` by asking the `LinkServer` "are we detached?"; the far side flips it (the
  reader locally, or over the network the `LinkServer` deriving it from link-down *or* a client
  "device gone" message). Not new scope - the visible face of the lifecycle we already defined.
- **`EventSink`** - **each side keeps its own local sink** (its own daemon's `subscribe()`);
  passed *into* the Link at construction like the channels, **not shared across it.** No
  dedicated event wire: the server synthesizes every controller event from data the Link
  already carries - `ControllerConnected`/`Disconnected`/`BatteryChanged` from the frame stream
  (they ride TCP in the idempotency split), `BindingLost`/`BindingAcquired` from link
  down/up, and `State` is each runtime's own. The one new `LinkServer` job: re-emit those
  events into the local sink as it decodes frames / detects link transitions.

**Discipline (why it stays clean).** The `CommChannel` API is deliberately **common to both
use cases even where that is not optimal for a single case** - `reader`/`mapping` must not get
more complex than today (and may get simpler). No network concept (peer address, seq,
retransmit) may appear on the device-side *local* type; if that pressure arises, it belongs
network-internal. The shared surface is exactly {frame, rumble, click, control endpoints +
the lifecycle methods}.

**Status:** design agreed; the **local loopback adapter is BUILT + HW-validated** (2026-08-05,
`engine/src/runtime/link.rs` - `LinkClient`/`LinkServer`, reader/mapper rewired through it; see
6.2). The **network adapter is the next piece** (6.2 slices below).

### 6.2 Implementation slices (network adapter - plan agreed 2026-08-06; slices 1-4 BUILT)

The role is chosen by the handle's input/output staging (no new entrypoint - "one engine, role
at runtime"): `set_output(Network(connect))` -> **client/forwarder role** (reader + client link,
no mapper/Sink); `set_input(Network(bind))` -> **server role** (mapper + server link + Sink, no
device); `Local`/`Local` stays the loopback (both threads). The 6.1 `Link*` method surface is
unchanged - the network variants slot in behind it, so `reader`/`mapping` stay untouched.

Sliced to front-load the zero-design-risk prerequisites:

1. **Serde prerequisites** - derive `Serialize`/`Deserialize` on `Program`+IR (engine:
   `CompiledSet/Layer/Binding/Command/Action`, `SetId/LayerId`, `SourceMap`) and on
   `ControllerState`/`Report` (+ their sub-types) in steam-hid. Mechanical, no design risk,
   needed by everything. (`Program` stays disk-unpersisted - `ConfigDoc` remains the on-disk form;
   this only adds a *wire* path.)
2. **Wire protocol** - message enums + framing (reuse `ipc`'s length-prefixed **postcard**
   pattern) and the hidden bridge-thread machinery, all inside `runtime/link.rs`: `State`->UDP
   (`seq`, latest-wins); `Connected`/`Disconnected`/`Battery` + control (`Program`/chords)->TCP;
   rumble/click back server->client. **Decision to settle here:** back-channel reliability
   (lean: rumble over UDP latest-wins like frames - it's a level; clicks TBD, possibly TCP).
3. **`LinkClient`/`LinkServer` network variants** - turn the internal repr into a
   `Local | Network` enum behind the same methods; the network variant spawns the UDP/TCP bridge
   threads that feed *local* crossbeams (so the mapper's `select!` composes verbatim) and merges
   the two frame feeds into one `frame_rx` (epoch-gate stale UDP after a TCP `Disconnected`).
   The client's `control_tx` (config uplink) reappears here (net-only; the local client has none).
4. **Role branching** in `handle::start` + `Runtime` - `output=Network` builds reader-only +
   client link (no Sink); `input=Network` builds mapper-only + server link (no device); the
   client routes `apply`/`set_chords` to the control uplink instead of a local mapper (`DeviceConfig`
   stays local - never on the wire). `status`/
   events per side (client = device state; server = mapping state), each synthesizing the far
   side's events from the wire per 6.1.
5. **Reconnect / lifecycle / epoch-gating.** **5a BUILT + HW-validated (server reconnect):**
   `NetServer` loop-accepts (survives a client close/restart), a shared `detached` flag, epoch-reset
   of the UDP seq gate between connections. **5b BUILT (server surfaces it; local D6 re-check
   pending HW):** the mapper polls `is_detached()` -> `WaitingForDevice`; reattach unified behind
   `LinkServer::poll_reattach()` (local session-swap / network resume-on-same-channel); the mapper
   owns the `State(Running)` reattach edge; net server logs connect/disconnect. 5b changed the local
   D6 reattach from `select!` to a 50 ms poll. **5a+5b HW-VALIDATED CROSS-MACHINE (2026-08-06 - the
   6 goal): a Steam Deck (Neptune) forwarding to a PC server over a LAN, with mapping, Main/Fallback
   chords over the wire, and client disconnect->`WaitingForDevice`->reconnect->`Running`; plus a local
   Gordon dongle unplug/replug D6 regression, all confirmed.** **5c BUILT (pending HW):** the client
   uplink thread `pump`s + re-`dial`s on any drop, gated on `device_present` so a device outage drops
   the link (not reconnect-to-nothing); `LinkClient::Network` `detach()` shuts the TCP (-> server
   `WaitingForDevice`) + `reattach()` re-dials (reader-driven); an `Uplink::Ping` keep-alive (~1 s)
   surfaces a dead server over the otherwise-idle TCP. **NETWORK ADAPTER (slices 1-5) COMPLETE**;
   5c wants HW (mid-session drop / server restart -> re-dial; client controller unplug -> server waits
   -> replug -> resume). *Superseded note:* client-side `LinkClient::Network` `detach()`=drop the link
   (device-loss -> server
   `WaitingForDevice`, Q1), `reattach()`=re-dial, + internal re-dial on a network blip while the
   device is fine (today the client uplink thread just exits on a write error). **Tested with two
   daemons on localhost** (`--socket`/`-k`, one daemon per socket).

**BUILT (2026-08-06, slices 1-4; HW/two-daemon validation of the end-to-end path still pending):**
- **S1** serde on the `Program` IR (steam-hid `ControllerState`/`Report` were already serde-ready
  behind their feature - just enabled it). `Program` stays disk-unpersisted.
- **S2** `runtime/link/wire.rs`: `FramePacket`(=`ControllerState`, UDP), `Uplink{Hello,Apply,
  SetChords,Event}` (TCP; **only chords cross the wire, never `DeviceConfig`**), `Downlink{Rumble,
  Click}` (UDP back-channel), + datagram/frame codecs.
  **Back-channel decision settled: rumble AND click both over UDP** (latest-wins rumble; a dropped
  click is an imperceptible missed tick - revisit if it matters).
- **S3** `runtime/link/net.rs`: `NetClient`/`NetServer` - the hidden UDP/TCP bridge threads feeding
  local crossbeams; server binds TCP+UDP on one port and learns the client's UDP return address from
  the first datagram; `Hello{PROTOCOL_VERSION}` handshake; clean shutdown. Single connection.
- **S4** `LinkClient`/`LinkServer` are now `Local | Network` enums (4a); `Runtime::start_local/
  start_client/start_server` + `handle::start` branch on the staged input/output (4b), with
  `Input::Network`/`Output::Network` (`host:port`) parsing.

**Design refinements settled during the build (supersede earlier 6/6.1 notes):**
- **Config is ordinary commands, NOT a handshake step (Q3).** The client does **not** auto-ship
  config at connect; `--output host:port` alone just forwards frames and the **server maps with its
  own config**. `-m/-f/-c -s` on the client issues `Apply`/`SetChords` after connecting (the
  forwarder's `push_staged_config`, which pushes chords **only when `Some`** so a thin client retains
  the server's), exactly like local seeding; `DeviceConfig` (`-d`) stays machine-local and is never
  shipped. The **server retains its config across reconnects** (revises "config shipped to server at
  connect"). Handshake = TCP connect + `Hello` only.
- **Lifecycle: the client drops the link on device-loss -> the server has ONE outage path (Q1).**
  When the client loses its device (transport-gone / reader `detach`) it drops the network link, so
  a client-device-loss and a real network drop both manifest as **link-down -> server
  `WaitingForDevice` -> (re)connect -> `reattach`**. This yields *faithful local semantics for free*:
  transport-loss (link drop) -> `WaitingForDevice`; controller-off (`Report::Disconnected` value,
  forwarded over the still-alive link) -> mapper `release_all` but Running. Detection + re-dial is
  **slice 5** - the S4 network arms are single-connection stubs (`detach`/`reattach` no-op,
  `is_detached`=false, a never-ready `reattach_rx`).

## 7. COMMAND FEEDBACK (haptics + audio)

Per-command tactile/audible feedback: extends today's single-click command haptic into a small,
bounded set of **presets** - single/double/triple haptic clicks, short/long audio tones (single/
double/triple, morse-like), and trackpad-actuator sweeps ("chirps"). Design agreed 2026-09-03; the
**hardware facts this builds on are in 1.9** (the `beep*` exploration, lines ~1184-1238) and are NOT
restated here. This section is the durable **model + build-order** record (a multi-session feature
like 5.3); once the reader/HW step lands, its knowledge folds into `CLAUDE.md` haptics.

### 7.1 The either/or constraint (why the model is shaped this way)

Audio on these controllers **is the haptic actuator driven as a voice-coil/LRA** - there is no
separate speaker (1.9). So per side, a haptic **click** and an audio **tone** share one physical
actuator and cannot coexist; the later command just preempts the earlier. This is a hardware fact,
not a policy: hence one **`Effect`** per edge is a single medium (click OR tone OR chirp), and "both
at once on one side" is simply inexpressible. Two independent axes remain:
- **Rumble** (per-profile, continuous) is a *different channel* on Deck/Triton (motors, `0xeb`/`0x80`)
  -> it coexists with audio; only on Gordon does everything share `0x8f` (a feedback sequence briefly
  steps on rumble, resumed by the re-fire). Rumble ducking is a future reader policy, not modelled.
- **Left/Right** are independent actuators. Haptic uses the triggering input's side; **audio plays
  both sides** (heard, not felt) - the reader **ignores `side` for audio**. `side` still crosses the
  wire so an audio-per-side policy stays possible later without a wire change.

### 7.2 Config model (authoring; crosses the 6 wire inside `FeedbackReq`)

Replaces the old `CommandSettings.haptics: Haptics` **wholesale** (no back-compat - API-not-stable
policy). Deliberately **preset, not free-form** (no user-set freq/interval); the model mirrors the
UI 1:1 (one combobox = one enum; arity encoded as distinct variants so each note gets its own
configurator; two independent per-edge checkboxes).

```
enum Click { Low, Medium, High }                                    // was HapticStrength - renamed
enum Tone  { ShortLow, ShortMedium, ShortHigh, LongLow, LongMedium, LongHigh }  // pitch x length
enum Sweep { Up1, Down1, Up2, Down2, Up3, Down3 }                   // noop on Gordon (no sweep path)

enum Effect {                       // one medium per edge (enforces the 7.1 either/or)
    Haptic(Click),
    HapticDouble(Click, Click),
    HapticTriple(Click, Click, Click),
    Audio(Tone),
    AudioDouble(Tone, Tone),
    AudioTriple(Tone, Tone, Tone),
    Chirp(Sweep),
}
struct Feedback {                   // replaces `Haptics`; two independent edges
    on_press:   Option<Effect>,     // UI: a checkbox each; its Effect configurator appears below
    on_release: Option<Effect>,
}
// CommandSettings { toggle, turbo, feedback: Feedback }
```

**Decisions settled:** (a) **arity-as-variant** (not `Vec`) - keeps UI 1:1 and makes the per-arity
interval natural; cap = triple. (b) **length folded into `Tone`** (6 variants) not a separate
`AudioLen` - so every note in a double/triple is independently short/long (morse patterns) with
*fewer* `Effect` variants; a `Click` has no length (impulse), so the audio/haptic asymmetry is
honest. (c) **per-edge `Option` + two checkboxes** (not a 4-way `Off/Press/Release/Both` combobox) -
toggling one edge never disturbs the other, and each edge's settings render under its checkbox. (d)
**Melody dropped** (was a gimmick; no enum slot - re-addable later, nothing depends on it). (e)
**`Sweep` flat Up/Down**, direction not split out (would need 2 comboboxes).

### 7.3 Mapper seam (routing UNCHANGED, payload widened)

The mapper resolves the edge (press->`on_press` / release->`on_release`, exactly as `haptic_fires`
does today) and emits **one** request per edge carrying the chosen `Effect`; the reader owns *when*/
*how* (golden-test purity holds - the mapper says *what*, deterministically per tick):

```
struct FeedbackReq { side: Side, effect: Effect }   // was HapticReq; only Effect+side cross the wire
```

The **three-way effect routing is reused verbatim** - only the ferried payload changes
`HapticReq`->`FeedbackReq`: `Level` (inline on output edges), `PersistentOpSet` (deferred into
`pending_feedback`, fired iff the op mutated state), `Hold` (rides the held-layer lifecycle via
`HoldFeedback`). **Naming:** the mapper's private routing enum (currently `Effect { Level,
PersistentOpSet, Hold }`) is **renamed `Route`** to free `Effect` for the config type above (and
`Route` is the better name for it anyway). Rename cluster: `HapticReq`->`FeedbackReq`,
`haptic_req()`->`feedback_req()`, `haptic_fires`->`feedback_fires`, `pending_haptics`->
`pending_feedback`, `HoldHaptic`->`HoldFeedback`, `HapticStrength`->`Click`, the `Click` downlink type
+ `click_tx/rx` -> `feedback_tx/rx`.

### 7.4 Reader sequencer (device-INDEPENDENT expansion; device only at fire time)

The reader-local vocab is built from `Effect` and **never crosses the wire** (so each machine maps a
preset to freqs audible on *its own* hardware - the per-device tone ceilings differ, 1.9). The key
layering the design turns on: `from(FeedbackReq)` needs **no `DeviceKind`** (pure arity -> notes +
intervals over presets); `DeviceKind` enters **only** at `fire_note` (pitch -> per-device freq,
length -> shared const ms; audio -> both actuators; chirp -> noop on Gordon). Lives in a new
`feedback.rs` module, NOT `reader.rs` (the split is clean; rumble stays in `reader.rs`).

```
const SHORT_MS: u16 = 30;  const LONG_MS: u16 = 100;   // tone durations; used by from() AND fire_note
// gaps (fire_end -> next onset), placeholders to tune on HW:
//   AUDIO_DOUBLE=40 AUDIO_TRIPLE=30   HAPTIC_DOUBLE=50 HAPTIC_TRIPLE=40  (clicks ~0 duration)

enum Note { Haptic(Click), Audio(Tone), Chirp(Sweep) }     // Note::Audio is a single Tone, no length arg
struct SeqNote  { interval_ms: u16, note: Note }           // interval = wait BEFORE this note fires
struct Sequencer { side: Side, notes: VecDeque<SeqNote> }  // + next_due
```

**Interval semantics (all in `from()`):** a note's `interval` is "wait after the *previous* note's
onset until *I* fire" = `duration(prev) + gap` (audio; haptics = gap only, clicks have ~0 duration).
So `AudioDouble(ShortHigh, LongLow)` -> note1.interval = `SHORT_MS(30)+AUDIO_DOUBLE(40)` = 70; Long
first -> 140. Triple audio: 60/130. Haptic double/triple: 50/40. The predecessor-duration coupling
lives *only* in `from()`; the `Sequencer` consumes intervals blindly (it never reasons about played
notes - that's what makes the pre-note/pop-front shape clean).

**Play loop (pre-note, pop-front, latest-wins):**
- **On receive `FeedbackReq`:** build the `VecDeque`, **`pop_front` + fire it immediately** (its
  interval ignored), set `next_due` from the new front, install as the active sequencer **replacing**
  any prior tail. This fire-on-arrival is an **explicit action at receive time, NOT** a
  `note0.interval=0` run through the lap loop - otherwise two `FeedbackReq`s arriving in the same
  drain lap would let the 2nd replace the queue before the 1st's note 0 fired, losing today's "two
  simultaneous single clicks both fire". (So single-note effects always fire; only a multi-note
  *tail* yields to a newer effect.)
- **Each reader lap** (the loop already laps at the ~4 ms `poll` timeout - so timing is free, NO
  deadline-capped wait needed): while `now >= next_due`, `pop_front` + `fire_note`, then advance
  `next_due += front().interval` (schedule-relative, drift-free); empty queue -> clear the sequencer.

One sequencer total (not per-side): each *note* decides actuator count (audio=both, haptic=side), so
a left-click + right-tone don't need two timelines - latest-wins is enough (feedback edges are sparse).

**BUILT (2026-09-04), + volume knobs.** `feedback.rs` is as above, with one refinement: `fire_note`
takes the reader's **`DeviceTuning`** (the bound device's rumble+audio tuning, `ReaderRumble`
renamed) instead of `DeviceKind` - the variant is both the device discriminant *and* its audio
volume, so `DeviceKind` is gone from `feedback` entirely. Per-device volume is a **fixed knob**
(not strength-scaled) in `config`: `GordonTuning.audio_duty` (0..=100 %) and `MotorTuning.audio_gain`
(dB, -16..8; `RumbleTuning` renamed `MotorTuning`, rumble fields renamed `rumble_*`). Gordon maps
`audio_duty` linearly across a new `AUDIO_MAX_DUTY = 0.5` (the beep saturates ~50 % duty, HW),
mirroring `RUMBLE_MAX_DUTY`; Deck/Triton pass `audio_gain` straight to the firmware tone/sweep. The
knob rides `cfg.tuning`, so a live devcfg change lands on the next note. UI: the device page's three
groups (headers no longer say "rumble"; sliders relabelled `Rumble *`) each end with an `Audio duty`
/ `Audio gain` slider (deckhand + forwarder-ui).

**Starting-point values (HW-tuned a little; more finetuning later):** tone pitch Low 800 / Med 1200 /
High 1600 Hz (all under every ceiling); sweeps (Up = lo->hi, Down = hi->lo) 1: 800<->2000, 2:
800<->1400, 3: 1400<->2000, `SWEEP_MS=200`; `SHORT_MS=30`/`LONG_MS=100`; gaps audio 40/30, haptic
50/40; default volume Gordon 50 % duty, Neptune 2 dB, Triton 0 dB.

### 7.5 Build order (BUILT - de-risk the model via UI before touching hardware)

1. **Config types + validation + mapper routing.** Add the 7.2 model, do the 7.3 rename cluster, route
   `FeedbackReq` through the existing `Route` machinery. **Reader is a stub:** it receives a
   `FeedbackReq` and *demotes* it to today's single click - `Haptic*` -> fire the first `Click` as the
   current click; `Audio*`/`Chirp` -> ignored (HW not built; doesn't matter for UI work). No sequencer,
   no `feedback.rs` yet.
2. **UI** - the editor command-settings feedback block (two checkboxes + per-edge `Effect` combobox +
   per-note configurators), 1:1 with 7.2.
3. **Tune the model shape** by using the UI; adjust variants/naming if the UI reveals a better shape.
4. **Reader sequencer + per-device `fire_note`** (7.4) + finetune the consts/freqs on real hardware.

**Steps 1-4 all BUILT (2026-09-03/04); feature is functional + HW-validated on all three devices.**
Remaining is **finetuning only** (not blocking): dial the freqs/sweeps/gaps/gains + per-device
volumes; consider driving **Triton clicks off `0x81` pulse** instead of the 2-strength `0x82` for
finer granularity (PLAN 1.9 notes `0x81` single-pulse width = strength). Open/deferred as before:
rumble-ducking on Gordon (accepted collision); a UI warning for moot combos (turbo re-firing a long
sequence only replays note 0; OpSet-only fires OnPress) - validation-warning territory, later.
