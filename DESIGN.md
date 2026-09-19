# deckhand - Design

deckhand is a Steam Input-like tool that maps Steam Controllers (the original Steam
Controller, the Steam Deck, and the 2026 Steam Controller) to keyboard, mouse, and gamepad
output on Linux and Windows.

This document describes the design of the project: what each crate is for, how the pieces fit
together, and the reasoning behind the non-obvious choices. It is organized as an architectural
overview followed by one section per crate, in dependency order:

    1  vocab-hid
    2  vocab-out
    3  steam-hid
    4  virt-out
    5  config
    6  mapper
    7  engine
    8  ipc
    9  deckhandd
    10 deckhandctl
    11 deckhand
    12 forwarder-ui


## 0. Overview

### 0.1 Purpose and scope

deckhand reads input from Steam controllers and maps it to mouse, keyboard, and virtual-gamepad
output - the same idea as Steam Input, as a standalone tool. It supports three controllers - the
original Steam Controller, the Steam Deck's built-in controls, and the 2026 Steam Controller - over
USB and Bluetooth, and produces keyboard and mouse input plus a virtual Xbox 360 gamepad.

Beyond mapping a local controller to local output, it also runs networked: a controller host - a
Steam Deck, say - forwards its raw controller input over the LAN to an output host, a PC, which does
the mapping and drives its own virtual devices. This is the same system in a different arrangement,
not a separate mode.

It is deliberately narrow. It is not a game launcher, an overlay, or a desktop integration; it has no
notion of which application is running, and choosing which profile is active is left to the user
interface, never the runtime. (The product name avoids "Steam", a trademark; the internal crates are
named descriptively.)

### 0.2 Architecture and crate map

The workspace is twelve crates in a ports-and-adapters shape, organized around one idea: **the mapper
is pure**. It reads one vocabulary and writes another and depends on nothing else - no hardware
abstraction layer, no operating system, not even the runtime that drives it. Everything hardware- or
process-shaped is an adapter layered around that core.

The crates, from the leaves up:

    vocab-hid     the input vocabulary - what a controller reports (buttons, a per-frame snapshot)
    vocab-out     the output vocabulary - the keys, mouse buttons, and gamepad outputs a binding emits
    steam-hid     the input HAL - real controllers over USB/Bluetooth, decoded to vocab-hid
    virt-out      the output HAL - vocab-out realized as virtual keyboard/mouse/gamepad per OS
    config        the authoring model - the on-disk configuration and its validation
    mapper        the pure mapping core - a vocab-hid frame + a compiled program to vocab-out events
    engine        the headless runtime - owns the devices, compiles config, runs the mapper on threads
    ipc           the daemon control protocol - requests, responses, events over a local socket
    deckhandd     the daemon - embeds the engine and serves it over ipc
    deckhandctl   the command-line client
    deckhand      the graphical application - tray, configurator, and profile editor
    forwarder-ui  the touch-first quick-connect client for the Steam Deck forwarder case

Dependencies point strictly downward. The two vocabulary crates are the leaves; the two HALs and
`config` sit above them (and `config` depends on the vocabularies but on neither HAL, so authoring
never drags in hardware); the pure `mapper` sits above those; `engine` composes everything below it;
`ipc` is a sibling protocol crate; and the three client binaries plus the daemon are at the top.
Nothing depends on `engine` except the daemon, and the clients depend only on `ipc` and `config` - so
a thin client links neither the runtime nor a HAL. The one placement worth calling out is that
compiling the authoring model into the mapper's program lives in `engine`, not in `config` or
`mapper`: the compiled form is the runtime's own input contract, so it belongs with its consumer, and
`config` stays pure authoring data.

**The core** reads the input vocabulary and writes the output vocabulary, and nothing more:

    vocab-hid  ->  mapper (+ program)  ->  vocab-out
    input          the pure core:          output
    vocabulary     no HAL, no OS,          vocabulary
                   injected clock

**The mapping path** runs that core between real hardware and virtual hardware, with the two HALs at
the edges and the compiled program feeding the mapper:

    hardware --> steam-hid --> vocab-hid --> mapper --> vocab-out --> virt-out --> virtual devices
    (a Steam     input HAL     input         pure       output        output       (Xbox pad,
     controller)               vocabulary    core       vocabulary    HAL           keyboard, mouse)
                                               ^
                                               |
                                            program   (config compiled to the mapper's IR)

**The runtime path** is `engine` composing the pieces into a running process. It owns the threads -
one reader per device feeding a central mapping loop over channels - compiles the configuration it is
given into a program, and drives the mapper. It is headless: no UI, and no concept of the desktop.

    config  --engine compiles-->  program --.
                                            v
        steam-hid  <-- reader thread --  [ engine ]  -- mapping loop drives mapper -->  virt-out

**The process path** is the daemon and its clients over `ipc`. The daemon embeds the engine and
exposes it over the control protocol; three clients drive it. The graphical application also edits
configurations directly, without the daemon - it uses `config` to build and validate a document and
the two vocabularies to present the input and output names a binding can choose - and ships the result
to the daemon over `ipc`.

    deckhandctl      deckhand (UI)      forwarder-ui        three ipc clients
          \                |                  /
           '----------->  ipc  <-------------'              the control protocol (its own crate)
                           |
                       deckhandd                            the daemon: embeds engine, serves ipc
                           |
                        engine

**The networked path** reuses all of the above. The network is simply another adapter around the
engine: the controller host transmits raw `vocab-hid` snapshots on the wire, and the mapper runs on
the output host. A snapshot is idempotent and latest-wins, which is what makes it safe to send over an
unreliable transport; both hosts are the same engine artifact, their role chosen at runtime.

    controller host (client)                             output host (server)
    steam-hid -> raw ControllerState  == network ==>  mapper -> virt-out

### 0.3 Cross-cutting conventions

**Synchronous throughout, no async runtime.** The whole system is plain threads and channels: a reader
thread per device feeds a central mapping loop, commands flow back over channels, and output emission
is a syscall on the loop's thread. Nothing pulls in an async runtime.

**The crate wall keeps the core pure.** The mapper's isolation is enforced by the crate boundary, not
by discipline alone - it simply does not depend on any HAL or on the OS, and its only notion of time
is an injected clock. That is what makes it deterministic and golden-testable from recorded traces
without hardware.

**Two shared vocabularies at the leaves** let every layer share one set of names without depending on
one another: the authoring model names inputs and outputs, and the HALs produce and consume them, yet
`config` and the HALs never depend on each other.

**One version, pinned dependencies.** All twelve crates share a single workspace version, and shared
dependency versions are pinned once in the workspace manifest.

**APIs break freely; the config format will not.** The internal Rust APIs are deliberately unstable
and are refactored whenever that yields a better design, rather than carrying compatibility debt. The
one planned exception is the on-disk configuration format, which becomes a versioned, non-breaking
user contract at a stable release.

**Diagnostics and errors** go through a logging facade throughout, with typed error types in the
library crates that the binaries surface. Terminal output, logs, and code are kept ASCII-only;
Unicode is reserved for the graphical interface.

**Dependencies are conservative** - only well-maintained, widely-used crates, never small or unproven
ones. The project is Rust edition 2024 on the latest stable toolchain with no minimum-version policy,
dual-licensed MIT or Apache-2.0 (only protocol facts are taken from external references; the code and
API are the project's own).

### 0.4 Supported platforms

deckhand targets Linux and Windows, and its architecture is shaped so a third operating system is an
additive port rather than a rewrite. All platform-specific code is confined to a few edges: the input
HAL's HID backend, the output HAL's per-OS sink, and the graphical application's system-tray
companion. Everything else - the vocabularies, `config`, the pure `mapper`, `engine`, `ipc`, and the
clients - is platform-agnostic.

Each HAL selects its platform implementation at compile time as a concrete type, not through a runtime
abstraction, so there is no speculative indirection carried for a platform that does not exist yet.
The control socket differs between platforms only in the socket's name. Consequently the per-OS
specifics stay localized: on Linux the input side uses raw HID and the output side uses uinput, with
systemd socket activation and D-Bus for the tray and the idle inhibitor; on Windows the output side
uses the system input-injection call plus a compile-time-selected virtual-gamepad backend, with a
named-pipe control socket and a native tray. Porting to another OS means adding backends at those two
HAL edges and a tray, and touching none of the core.


## 1. vocab-hid

The hardware-independent input vocabulary: the names and shapes that describe what a controller
reports, with no reference to any particular device's wire format.

### 1.1 Role and boundaries

vocab-hid is one of the two leaf crates at the bottom of the dependency graph (the other is
`vocab-out`). Together they let the rest of the system share a single vocabulary without the
layers depending on each other: the authoring model (`config`) names controller buttons in chords
and gaters, and the input HAL (`steam-hid`) produces per-frame snapshots in these types - yet
`config` never depends on a HAL and no HAL depends on `config`.

Its only real dependency is `bitflags`, for the unified button bitfield. Serialization is an
opt-in `serde` feature: `config` enables it for on-disk RON and `steam-hid` for the network wire,
while a consumer that needs neither (for example `mapper`) pays for neither.

The crate is split into two modules only to make explicit who needs what; both are re-exported
flat at the crate root, so consumers see one namespace:
- **core** holds the part config needs: the atomic vocabulary a binding can name.
- **state** holds what the mapper additionally reads: the per-frame snapshot and the types it
  is built from. This is the part that pulls in bitflags.

### 1.2 Public surface

- **Button** (core) - an enum with one variant per hardware button, across every supported
  device: face buttons, dpad, bumpers and trigger full-pulls, back grips, View/Menu, the Steam
  and Quick-Access buttons, pad and stick press/touch, and the capacitive grip-touch sensors.
- **Buttons** (state) - a `u64` bitfield mirror of `Button`, one flag per bit. `button_flag`
  maps a `Button` to its `Buttons` flag.
- **Axis** - the normalized analog channels: stick and pad X/Y, the two analog triggers, and the
  two pad-pressure channels.
- **Value types** - `Vec2` (normalized), `Vec2i` / `Vec3i` / `Quati` (raw wire i16 vectors and
  quaternion), `TrackPad` (a normalized pad sample), and `Timestamp`.
- **ControllerState** - the unified snapshot the mapper consumes each frame, plus an `axis()`
  accessor. `ACCEL_RES_PER_G` and `GYRO_RES_PER_DPS` document the IMU raw-value scale.
- Each enum exposes an `ALL` array in bit / declared order for diffing and UI listings.

### 1.3 Design and reasoning

**A unified superset.** `Button` and `Buttons` enumerate every button any supported device has;
a device that lacks one simply never sets it. This is what lets profiles be device-independent -
they are authored against logical inputs, and whichever the bound device reports are the ones
that fire.

**Two views of the button set.** The atomic enum (`Button`, config's currency) and the packed
bitfield (`Buttons`, the snapshot's currency) coexist because each fits its consumer: naming one
target versus testing many bits per frame. `button_flag` is a free function rather than a method
precisely because it bridges the two views, which live in different modules.

**Normalized analog, raw IMU.** Sticks, pads and triggers are normalized to `f32`, but the IMU
(accel, gyro, orientation) passes through as raw `i16` with published scale constants. Passing
the IMU through faithfully - rather than pre-normalizing - lets each consumer rescale as it
needs while keeping one documented interpretation. The IMU frame is right-handed with X to the
right, Y forward (toward the nose), and Z up.

**Value types are `Clone` but not `Copy`.** Copies are made explicitly, keeping accidental
churn of the larger geometry types visible at the call site.

**Timestamp is relative.** A frame's capture time is elapsed-since-stream-start, not wall-clock,
so it serializes cleanly into traces and can drive a replay clock deterministically.

**Battery is deliberately absent** from `ControllerState`: it is a device-level signal that
belongs to the HAL, not part of the per-frame input the mapper reasons about.

**Snapshot stability matters.** `ControllerState` is the value serialized across the network
link between a controller host and an output host, so its shape is a compatibility surface: it
is kept stable rather than reshaped casually.

The vocabulary is presentation-free - it carries no display labels. Human-facing names are the
UI's concern, keeping this crate a pure data vocabulary.


## 2. vocab-out

The hardware-independent output vocabulary: the bare names of the keyboard, mouse, and gamepad
outputs a binding can produce, and the emit item that carries them.

### 2.1 Role and boundaries

vocab-out is the output counterpart of `vocab-hid`, and the second leaf crate. It is shared by
`config` (which names the target of a binding) and `virt-out` (which realizes those targets on the
OS), so neither depends on the other. The enums carry no platform values: each backend maps a
bare name to its OS code (Linux evdev codes, Windows scancodes or virtual-key codes).

It has no dependency beyond optional `serde`, enabled by `config` for on-disk RON and left off for
`virt-out` so the output HAL does not pull `serde` in. As with `vocab-hid`, the crate is split into
**core** (the leaf target enums config needs) and **event** (the emit item the mapper builds
from them), re-exported flat at the crate root.

### 2.2 Public surface

- **Key** - a keyboard key, grouped by function (modifiers, editing, arrows, navigation,
  punctuation, digits, letters, function keys, the print/system cluster, keypad, and media /
  brightness keys).
- **MouseButton** - the standard buttons plus four `Scroll*` discrete-scroll pseudo-buttons.
- **GamepadButton** - an Xbox 360 button layout, including the dpad directions and two families
  of pseudo-buttons (full-trigger pulls and the eight stick-direction pushes). `is_dpad` and
  `is_axis_button` classify these; `is_scroll` does the same for mouse buttons.
- **GamepadAxis** - the analog outputs only: the two sticks and the two triggers.
- **SCROLL_HI_RES_PER_DETENT** - the high-resolution scroll unit (120 per wheel detent).
- **OutputEvent** - the batch item handed to the output HAL's sink.
- Each enum exposes an `ALL` array in declared order.

### 2.3 Design and reasoning

**Bare names, no platform values.** Keeping OS codes out of the vocabulary is what allows a
single output model to serve both backends; the mapping to real codes is each backend's job and
stays at the edge.

**Levels versus deltas.** `OutputEvent` distinguishes state-carrying outputs (a key, a button, or
an axis position) from relative ones (pointer motion and scrolling). The mapper emits only
changes and the HAL simply realizes them, so the two never have to agree on more than the current
event.

**Scroll ticks are pseudo-buttons.** A discrete scroll tick behaves like a momentary button - it
fires once per activation and repeats under turbo - so the four scroll directions live among the
mouse buttons rather than as a separate concept. The backend realizes them as wheel ticks, not as
button bits.

**One high-resolution scroll unit.** Modern input stacks drive scrolling from a high-resolution
axis where 120 units make one detent (the shared kernel/libinput/Windows convention). Exposing
that single constant gives the engine one scale to convert motion into and every backend one unit
to emit, synthesizing a legacy notch every 120 so non-high-resolution consumers still scroll.

**Gamepad pseudo-buttons.** Two families of `GamepadButton` are not plain button bits. The dpad
directions fold into the pad's hat, and the full-trigger and stick-direction buttons drive an
analog axis to its extreme (opposing directions cancelling to neutral). Modeling them as
bindable buttons keeps authoring uniform - anything can be bound to a button - while the backend
handles the realization. Consequently the dpad is not a `GamepadAxis`: it is four logical
buttons, and the hat is purely a backend detail. Output names follow the XInput convention
(Back/Start/Guide, bumpers) rather than the controller-side input names.


## 3. steam-hid

The input HAL: talks to real controllers over USB and Bluetooth and produces input state.

### 3.1 Role and boundaries

steam-hid is the input hardware-abstraction layer: the adapter between real Steam controllers and
the rest of the system. It discovers and opens devices over USB and Bluetooth, decodes their input
into the shared `vocab-hid` snapshot, and sends commands to them (lizard mode on/off, IMU on/off,
haptics and rumble, LED brightness, idle timeout, power off).

It is the input side only. It does no mapping - that is the mapper and the engine above it - and no
virtual output - that is virt-out, its symmetric peer. It depends on `vocab-hid` (whose
`ControllerState` it produces) and on nothing else in the workspace; consumers keep using
`steam_hid::ControllerState`, `::Buttons`, and the value types, which it re-exports.

Its outside dependencies are `hidapi` (a C-backed HID library with a native backend on every
platform), `bitflags`, `thiserror`, and `log`. Serialization of `ControllerState` / `Report` is an
opt-in feature, enabled where snapshots are recorded to traces or sent across the network link and
left off elsewhere.

Two boundaries shape the crate:
- **hidapi is confined to one module.** A single backend module speaks to the library - the context
  that enumerates and opens devices, and the per-device raw handle for timed reads and feature /
  output reports. Everything else is hidapi-free, so the wire decoding and the command surface are
  ordinary data code.
- **A `Device` is a transport endpoint, not a live controller.** It corresponds to a wired gamepad
  interface or a dongle slot, and it outlives the controller connecting and disconnecting: the read
  stream reports those as frames rather than as the device appearing and vanishing. A `Device` is
  `Send` (it can be moved onto a per-device reader thread) but not `Sync` (it is a single-writer
  handle), and it spawns no threads of its own.

### 3.2 Public surface

- **Manager** - owns the HID context. `enumerate()` returns the connected Steam controller gamepad
  interfaces; `open()` opens a specific one; `open_first()` is a convenience.
- **Device** - an opened endpoint. `info()` reports what it is. Input is read as frames:
  `read()` / `poll()` return a `Report`, and `events()` is a change-driven view over the same
  stream. Commands are methods: `set_lizard_mode`, `set_imu_mode` / `set_gyro`, `set_led_intensity`,
  `set_idle_timeout`, `dongle_get_wireless_state`, `power_off`; the read-back queries
  `get_string_attribute` / `get_attributes` / `get_settings`; and the haptic family (below).
- **Report** - `State(ControllerState)` for an input frame, or the lifecycle signals `Connected`,
  `Disconnected`, and `Battery(Battery)`.
- **Identity** - `DeviceKind` (`Gordon` / `Neptune` / `Triton`), `Transport`
  (`UsbWired` / `UsbDongle` / `Bluetooth`), `DeviceInfo` (a discovered entry), and `DeviceId`, a
  stable path-independent name with the string form `kind:transport:interface:serial`. `DeviceId`
  is built from the fields that survive a replug rather than the OS path, so a selection can be
  pinned to the same physical device across reconnects and named on the command line.
- **Events / Event / diff** - the streaming change-log view and the stateless snapshot-to-snapshot
  diff underneath it.
- **Command vocabularies** - the per-device raw button bitfields (`GordonButtons` / `NeptuneButtons`
  / `TritonButtons`), `GyroMode`, and the haptic parameter enums (`HapticSide`, `HapticPosition`,
  `HapticType`, `HapticStyle`, `HapticIntensity`), plus `ControllerStringAttributes`.
- **Error / Result** - with two deliberate non-errors: a read timeout is `None` from the `poll`
  methods, and a controller disconnecting while its transport stays alive is a `Disconnected`
  *value*, not an error. Only a transport that genuinely goes away (a wired unplug, the dongle
  removed) is `Error::Disconnected`.

The haptic surface is intentionally per-device rather than one abstract call, because the actuators
and the levers that drive them differ enough that a single abstraction would erase the useful
distinctions: `haptic_pulse` (the trackpad-actuator pulse), `haptic_cmd` / `haptic_tone` /
`haptic_logsweep` (the Deck's synthesized click, tone, and chirp), `rumble_cmd` (the Deck's
dual-motor rumble), and the Triton output-report haptics `rumble_triton`, `pulse_triton`,
`haptic_command_triton`, `lfo_tone_triton`, and `logsweep_triton`. Which ones a given device honors
is documented per method; the engine above chooses and shapes them per device.

### 3.3 Design and reasoning

**One physical read is one frame of some type.** The controller pushes complete state snapshots at a
high rate - it never sends deltas - but the *same* stream also carries lifecycle frames (connect,
disconnect, and battery on wireless links). So a read yields an enum, `Report`, never a bare state.
A bare-state return could not represent a disconnect, and it would then block forever the moment a
controller powered off while its dongle stayed connected. Modeling the stream as a tagged frame is
what lets a `Device` outlive the controller.

**Decoding is a single step.** Raw HID bytes convert straight into a `Report` - an input frame into
a normalized `ControllerState`, a lifecycle frame into its signal. There is no decoded-report
middle layer between the wire packet and the unified snapshot; each device's raw button bitfield is
folded into the shared superset at the same point.

**A malformed frame is never surfaced.** An unknown or too-short report decodes to "nothing this
read", and the reader simply reads again; only genuine transport loss becomes an error. This is a
firm contract because the consumer treats *any* read error as the transport being gone, which would
trigger a false disconnect and a reacquire. Length-safe access throughout means a short buffer folds
to a skip rather than a panic, and an unrecognized report type is logged once rather than dropped
silently.

**"Blocking" reads are built from short timed reads.** The blocking read loops over
bounded-timeout reads, so a thread parked in a read can still notice a shutdown request and unwind -
a truly infinite read could not be cancelled, and tearing a reader thread down would hang. On the
platform where a signal interrupts the read mid-wait, that interruption is folded into an ordinary
timeout so the loop laps and exits cleanly. Clean exit matters because `Device`'s drop restores
lizard mode; a controller left with its mappings cleared is dead until replugged.

**protocol.rs is the single canonical home for every hardware and wire-protocol fact.** Command and
setting IDs, the exact payload layouts, report framing, and the inbound report structures all live
in one module; the device and decoding layers only *use* it, never restate it. The rationale is that
protocol knowledge is the most error-prone thing in the crate and the hardest to verify, so it is
kept exact, sorted, and in exactly one place.

- **Serialization is a byte cast, not a serde pass.** Wire structs are laid out so their in-memory
  bytes *are* the wire layout, and a small trait casts them to and from byte slices. The single
  piece of `unsafe` lives once inside that trait, guarded per struct by a compile-time size
  assertion, and the cast assumes a little-endian host (the only kind the project runs on). This is
  why the raw wire chunk types are kept separate from the serde-friendly value types they decode
  into: the packed wire structs cannot safely derive the usual traits (those would take references
  to unaligned fields), and the conversion is a set of plain field-copy functions.
- **Framing is a device-layer concern, never baked into a payload.** The payload structs carry only
  their own fields; the report-id byte, the length header, and any segmentation are applied when the
  command is sent, so one payload definition serves every transport that command rides.
- **Identifier types follow one rule.** An id space that is outbound-only and closed is an enum
  whose discriminant is the wire byte; anything received back, dispatched on, aliased, or used as a
  bitmask is a plain constant or a bit-flag set. This keeps a new inbound report type from being
  silently absorbed while an outbound command set stays exhaustive.

**No keep-alive thread.** Some controllers revert to lizard mode after a short idle unless the
lizard-off is periodically re-asserted, which requires a write while reads are in flight on the same
handle. Rather than add a lock, a second file descriptor, or an internal thread, the crate stays a
single-writer `Device` and leaves the cadence to whoever owns the read loop - the engine's reader
thread interleaves the re-assert with its other writes. A standalone consumer re-asserts itself. The
device kind exposes whether it needs this at all, so the owner can gate the cadence.

### 3.4 Hardware and protocol

**Supported devices.** All carry Valve's USB vendor id; the three codenames are Gordon (the original
Steam Controller), Neptune (the Steam Deck's built-in controls), and Triton (the 2026 Steam
Controller).

    Device                                   Codename  PID     Transport
    Steam Controller, wired                  Gordon    0x1102  USB wired
    Steam Controller, wireless dongle        Gordon    0x1142  USB dongle
    Steam Controller, Bluetooth              Gordon    0x1106  Bluetooth
    Steam Deck                               Neptune   0x1205  USB wired
    New Steam Controller, wired              Triton    0x1302  USB wired
    New Steam Controller, Bluetooth          Triton    0x1303  Bluetooth
    New Steam Controller, "puck" dongle      Triton    0x1304  USB dongle

Enumeration is pinned to these product ids. A block of Valve-vendor ids just below the Deck's is
*not* real hardware - it is a translation daemon's emulated Steam Deck, one id per handheld brand -
and matching it would bind the wrong physical device, so those are deliberately excluded. A second
Triton dongle variant is left out until one is seen.

**Interface filtering.** Each device exposes several HID interfaces, only one of which (or, on a
dongle, a few) is the real gamepad; the rest are the emulated mouse and keyboard of lizard mode.
Enumeration keeps only the vendor gamepad interface, identified by its usage page, and drops the
emulated ones - otherwise opening the first interface would read nothing useful. The same filter
works over Bluetooth, where the single node is listed once per top-level collection.

**Three protocol families, keyed on device and transport.** The framing differs enough that the read
path branches three ways:
- **Gordon and Neptune over USB** deliver 64-byte reports that begin with a fixed prefix and an
  event byte; decoding dispatches on that byte (input state, deck input state, connect / disconnect,
  battery status).
- **Triton** puts a report id in the first byte and is dispatched on that instead, across all its
  transports. Its command channel has a hard constraint: commands ride one specific feature report
  and the whole HID report must be exactly 64 bytes, or the device stalls the transfer and the
  command silently fails while input keeps streaming.
- **Gordon over Bluetooth** is a different transport with the same command bytes: writes and input
  both ride a segmented 20-byte report, and input arrives as a compact *delta* stream - a header
  nibble plus a chunk mask, then only the chunks that changed - which is reassembled and accumulated
  into a running snapshot. Triton over Bluetooth, by contrast, is *not* segmented: the OS
  HID-over-GATT stack reassembles it and prepends a report id, so it arrives like USB and rides the
  same Triton path.

**On opening a wireless endpoint** the crate prompts the original dongle for its current wireless
state, so a controller that was already connected surfaces without waiting for a fresh connect
frame. The Triton puck streams state on its own when a controller is present and needs no prompt.

**The IMU frame is unified and right-handed** - X to the right, Y forward toward the nose, Z up -
with published raw-value scales for accel and gyro. One device's raw gyro has a channel mounted
inverted and is negated on decode so its triple is right-handed; the other two already sit in that
frame and pass through raw. One device's gyro runs at a slightly different full scale, a small
difference accepted rather than rescaled. A small radial deadzone on the per-frame gyro delta
removes a DC drift without harming fine aim.

**The original controller multiplexes its left side over USB.** The left pad and the analog stick
share one coordinate field and a click bit. The snapshot resolves this so it is clean and matches
the Bluetooth path, which has no multiplex: the coordinate is read as the pad when the pad is
touched and as the stick otherwise, and a shared click bit is attributed to the pad only when the
pad is engaged (touched, or flagged as in simultaneous use with the stick) and to the stick
otherwise. The touch button is reported as steady through the per-frame tag that flickers during
simultaneous use.

**Battery is per device, not per transport.** Battery arrives out of band as a `Report::Battery`
frame - never a field of the input snapshot - so a device that does not report it simply never emits
one, and any device that does report it surfaces here. What a device reports is its own affair: a
wireless link carries real charge, while the original controller pins a fixed placeholder charge on
its wired USB link (a firmware quirk). Nothing filters battery by transport.

**Keep-alive by device.** The Deck reverts to lizard mode about ten seconds after lizard-off and
Triton after about three; the original controller holds its configuration and needs none. Whether a
device needs the periodic re-assert is exposed on the device kind, and the single cadence that
serves the devices that need it lives in the owning read loop, not in this crate.

**Haptics differ per device, and the surface reflects that.** The salient hardware differences the
methods encode:
- **The original controller** has only the trackpad-actuator pulse and no motors. Its amplitude
  lever is the pulse duty cycle rather than a gain field, and it has no "both pads" primitive, so the
  two pads are fired separately. Its two pad positions are swapped on the wire.
- **The Deck** honors the pulse's gain, adds real dual-motor rumble as a separate command, and has
  a firmware-synthesized haptic engine for a finely-tuned click and for clean tones and frequency
  sweeps - the actuators are resonant, so a hand-timed square wave collapses off-resonance and the
  synthesized path is what produces usable pitch. Its rumble and its synthesized commands are fixed
  short bursts, so a sustained effect is re-issued periodically.
- **Triton** drives haptics over output reports on the interrupt-OUT endpoint rather than feature
  reports: continuous rumble, a discrete-strength click, a single or repeated pulse, a synthesized
  tone, and a frequency sweep. Its pulse's two sides are swapped on the wire like the original
  controller's; its rumble safety-times out quickly and is re-issued to sustain.

Synthesized tones have a per-device pitch ceiling above which they go silent, and the finer amplitude
lever on the motor rumble is a wider inverted value than the coarse gain trim - both documented on
the methods that expose them.

The read-back queries (serial and other string attributes, the read-only attribute list, and
setting values) round-trip a request and its reply over the feature channel on the original
controller and the Deck. Triton's feature channel is effectively write-only - its replies come back
over the input stream with a framing that is not decoded - so the read-back queries are a
Gordon/Neptune facility.


## 4. virt-out

The output HAL: realizes output events as virtual keyboard, mouse, and gamepad devices.

### 4.1 Role and boundaries

virt-out is the output hardware-abstraction layer, the symmetric peer of steam-hid: it realizes the
abstract output events the engine produces as virtual keyboard, mouse, and gamepad input to the
operating system. steam-hid turns real controllers into a shared input vocabulary; virt-out turns a
shared output vocabulary into synthetic OS input.

It consumes `vocab-out` (the shared output vocabulary) and depends on no other workspace crate. The
engine drives it and stays platform-agnostic, because all platform code is isolated behind a single
concrete `Sink` type selected at compile time by target OS - a cfg-picked concrete type, not a trait
object, exactly as steam-hid selects its HID backend. Extracting output emission into its own crate
is what keeps the engine a pure function of input and configuration to output events, testable
without an operating system: output realization is as large and as platform-specific as HID input,
and folding it into the engine would platform-lock the mapping core.

It is synchronous - `emit` is a syscall made on the engine's mapping-loop thread. Its outside
dependencies are `thiserror` and `log`, plus `evdev` and `libc` on Linux and the Windows API
bindings (with the optional virtual-pad backends) on Windows. It carries no serde: serialization is
the authoring layer's concern, not the output edge's.

### 4.2 Public surface

- **Sink** - the output sink. `new()` creates the virtual devices; `emit()` realizes a batch of
  output events; `poll_rumble()` returns the virtual pad's current rumble. Dropping the sink
  destroys the virtual devices (unplugging the virtual pad).
- **Rumble** - the force-feedback a game sends *back* through the virtual pad, as heavy and light
  motor magnitudes (the Xbox model), to be routed onward to real-controller haptics.
- The `vocab-out` vocabulary is re-exported so consumers keep one namespace.

The output model is an imperative event stream, not a desired-state description. `emit` takes a
batch of `OutputEvent`s and flushes them; the engine holds the "applied" state and sends only
changes, so the sink stays a thin realizer. The vocabulary distinguishes state-carrying outputs
(a key, a button, or an axis position) from relative ones (pointer motion and scrolling), and the
sink simply applies whichever it is handed.

### 4.3 Design and reasoning

**The sink is deliberately dumb.** The engine is level-driven and already reconciles desired
against applied output, so virt-out does not track output state or diff anything - it realizes the
events it is given and flushes. This is what keeps the engine golden-testable on the emitted event
stream: the interesting logic is all one side of the boundary, and the other side is a
straight-line translation to OS calls.

**Pseudo-buttons are realized backend-side.** Two families of gamepad output are bindable buttons in
the vocabulary but are not plain button bits on the wire, and virt-out is where they are folded into
their real form:
- The four dpad directions fold into the pad's hat - one value per axis, with opposing directions
  cancelling to neutral.
- The full-trigger pulls and the eight stick-direction pushes drive an *axis* to its extreme rather
  than setting a button bit. A held direction or trigger overrides the analog value on that axis;
  on release the axis falls back to the last analog value seen, which the sink caches, so an axis
  driven by both an analog behaviour and a direction button behaves sensibly. Opposing stick
  directions cancel to neutral.

Keeping these as bindable buttons lets authoring stay uniform - anything can be bound to a button -
while the hat-and-axis realization, which is OS- and backend-specific, stays at the edge. The
folding logic is shared across the backends by two small helpers, so every backend behaves
identically here.

**Force feedback flows back through the pad.** The virtual gamepad advertises rumble and reads what
a consumer of the pad sends back; `poll_rumble` returns the current commanded rumble for the engine
to route onward to real-controller haptics, completing the ports-and-adapters loop. On Linux this
means speaking the uinput userspace force-feedback protocol - a request/response ioctl exchange, not
plain event emission - which includes honoring an effect's replay length locally (the kernel does
not stop playback for us) and managing a small pool of effect ids; playback is single-slot, so the
last-played effect wins. On Windows the rumble arrives over the selected backend's notification
channel. A build with no controller backend returns zero.

**Emission is best-effort where the OS can refuse it.** Synthetic input can be refused by the OS
when a higher-integrity or switching foreground owns the input desktop (a fullscreen or elevated
game exiting, a secure-desktop prompt); virt-out drops the refused frame and carries on rather than
tearing down the mapper, since the condition is transient and environmental. Any other failure stays
fatal, because it most likely indicates a malformed request. Separately, an output with no
realization on the current platform (a key the OS has no virtual key for) is dropped with a
one-time warning rather than silently, so a mis-bound key is visible.

### 4.4 Hardware and platform

**Linux uses three separate uinput virtual devices** - a keyboard, a mouse, and a gamepad - which
gives each a cleaner identity than one combined device would. Creating them needs write access to
the uinput node, the same permissions story as the input HAL's raw HID access.

**The virtual gamepad is an Xbox 360 pad on both platforms.** A uinput device becomes one through
its device identity (USB bus, Microsoft vendor, Xbox 360 product) plus the standard button and axis
code set; the common game-input layer derives a controller GUID from that identity and applies its
built-in Xbox mapping, so games see a standard pad with no driver of its own. The Windows backends
present the same Xbox 360 identity.

**Windows keyboard and mouse always go through the system input-injection call**, independent of
which gamepad backend is compiled in, so even a build with no controller backend maps keyboard and
mouse fully. The virtual gamepad, by contrast, is a compile-time, mutually exclusive choice of
backend:
- **ViGEm** - a virtual Xbox 360 pad through the ViGEmBus kernel driver; the proven default, with
  the rumble back-channel over the driver's notification channel.
- **VIIPER** - experimental, a virtual pad presented over a USB/IP system whose server runs on
  localhost. Its USB/IP transport is an internal detail of this one backend and is unrelated to the
  project's own networking; it is treated as a purely local virtual pad, exactly like ViGEm.
- **none** - no virtual gamepad; keyboard and mouse still work, and controller output is dropped
  with a warning.

Enabling both real backends is a compile error rather than a silent choice.

**Windows keyboard injection uses scancodes**, because games commonly read scancodes rather than
virtual keys; the scancode is derived from the virtual key, and extended keys (arrows, the
navigation cluster, right ctrl/alt, the meta keys, and a couple of keypad keys) carry the extended
flag. The media, volume, and browser keys are the exception - they are injected by virtual key,
because their only scancodes are the extended kind and injecting the bare scancode would collide
with ordinary letters. A few keys have no Windows virtual key at all (compose, the brightness and
keyboard-illumination keys, and some media-transport keys) and are the ones dropped with a warning.
Linux realizes every key via evdev.

**Sign conventions.** The shared vocabulary is evdev-signed - a stick's Y is positive-downward.
Linux passes axis values straight through; the Windows backends negate the thumbstick Y axes because
the native gamepad convention there is positive-up while the shared vocabulary is not. (The
controller-side physical sign is already normalized upstream in the mapper; this crate only reconciles
the remaining backend divergence.)

**Scroll.** A discrete scroll tick behaves like a momentary button and is realized as a wheel notch.
Modern input stacks drive scrolling from a high-resolution axis where a fixed number of units make
one detent. On Linux, once a device advertises the high-resolution wheel - which ours does, for
smooth scrolling - the stack ignores a lone legacy wheel event, so every discrete tick emits both
the high-resolution value and the legacy notch, and smooth scrolling synthesizes a legacy notch each
time a full detent of high-resolution units accumulates, so consumers that do not understand
high-resolution scroll still move. On Windows the high-resolution units pass straight through in the
native wheel scale and the OS accumulates them for applications that do not do smooth scrolling, so
no legacy fallback is needed.


## 5. config

The authoring model: the on-disk configuration and its validation.

### 5.1 Role and boundaries

config is the authoring model: the on-disk configuration data model, its RON serialization, and its
validation. It is deliberately pure data - no platform backends, no runtime, no timers - so anything
that needs to read or edit a configuration (above all the UI) can depend on it without pulling in the
HALs or the engine.

It sits above the two vocabulary crates and below `mapper` and `engine`. It depends on `serde`, `ron`,
and both vocabulary crates `vocab-out` and `vocab-hid` (with serialization enabled): an action names
an output target from `vocab-out`, and a gater or chord names a hardware button from `vocab-hid`, but
`config` depends on neither HAL. It speaks no controller's wire layout; turning a device's report onto
the logical inputs is the engine's boundary job, not this crate's.

Configuration is a three-layer pipeline, on the model of source to bytecode to virtual machine:
- **`ConfigDoc`** - the on-disk, user-facing, forgiving format; what the UI edits.
- **compile** - validate references, resolve names to indices, flatten.
- **`Program`** - the flat, index-based form that is cheap to execute in the hot loop.

config owns only the first layer and the validation. The compile step and the `Program` IR live in
the engine, because the IR is the runtime's contract and belongs with its consumer; config stays
pure authoring data and stops at the model plus `validate()`. serde is a hard dependency rather than
a feature, because serialization is the crate's whole purpose, and the format is RON - the model is
nested and enum-heavy, which RON represents cleanly, and it supports comments and hand-editing.

### 5.2 Public surface

- **ConfigDoc** - one profile (one per file): a version, a name, the per-profile rumble feel, and a
  list of action sets. The first action set is active on load.
- **ActionSet / Layer** - an action set is a full-controller mode (one active at a time), holding
  base bindings keyed by `InputSource` and a list of stackable layers; a layer is a partial overlay
  that overrides only the inputs it names.
- **InputSource / SourceKind / Side / Shape** - the logical hardware-independent input (the
  superset), its behavioural kind, the controller half it sits on (for haptic actuator selection),
  and a device's capability descriptor.
- **SourceBinding** - what attaches to one input: a button's commands, a button group's four
  members, each rich behaviour (`Joystick`, `DirectionalPad`, `AsMouse`, `JoystickMouse`,
  `GyroToMouse`, `Trigger`) with its settings and its virtual-button command slots, and `None` (an
  explicit unbind). Helpers report whether a binding is valid for a source kind, its commands, and
  its activation gate.
- **Command / Activator / CommandSettings** - a command is one activator: an `Activator`
  (`Regular { interruptible }`, `Long`, `Double`, `Start`, `Release`), an action combo, and settings
  (toggle, turbo, feedback).
- **Action** - an output (`Key`, `MouseButton`, `GamepadButton`), an engine action
  (`ChangeActionSet`, `HoldLayer`, `AddLayer`, `RemoveLayer`), or `None`.
- **The settings palette** - shared setting types (`Deadzone`, `AntiDeadzone`, `Curve`,
  `Sensitivity`, `Acceleration`, `Rotation`, `Invert`, `OneEuroFilter`, `OuterRing`, `SoftPull`,
  `Activation`) and per-behaviour settings structs, plus the output-target enums
  (`StickOutput` / `TriggerOutput` / `MouseOutput`), the `Axis` limit, `DpadLayout`, and `GyroSpace`.
- **Feedback authoring** - `Feedback` (an optional `Effect` on press and on release), `Effect` (a
  haptic click, an audio tone, or a chirp, singly/doubled/tripled), and the `Click` / `Tone` /
  `Sweep` parameters.
- **Above-profile config** - `DeviceConfig` (LED and idle settings plus per-device rumble tuning:
  `GordonTuning` / `MotorTuning` built from `Lever`s) and `Chords` (`Chord` firing a `ChordAction`,
  with `SwitchMode`).
- **validate()** - returns all diagnostics (a `Vec<Diagnostic>` with `Severity`), implemented for
  `ConfigDoc`, `DeviceConfig`, and `Chords`.

### 5.3 Design and reasoning

**Config declares; the engine resolves.** The three-layer pipeline keeps a rich, permissive,
editable format out of the few-millisecond hot loop and localizes every validity check in one
testable place, so the runtime can assume a well-formed program. Validation is per-profile and
collect-all rather than fail-fast - it reports every dangling reference and malformed binding at
once, so the UI can surface them all together rather than one per fix.

**Profiles are device-independent.** A profile references logical inputs only and never embeds a
`Shape`. The input vocabulary is a superset across all supported controllers, and a given device
provides a subset - Triton includes everything the Deck has plus the capacitive grip-touch sensors,
and the Deck adds a handful over the original controller. `Shape` is reference data used two ways:
the UI authors offline against it (filtering out inputs a device lacks) and the engine ignores
bindings for inputs the live device does not have. This is what makes a profile portable across
controllers - it is authored against intent, and whichever inputs the bound device reports are the
ones that fire.

**Two levels of mode, and no third.** Configuration has action sets (a full-controller swap, one
active at a time) and stackable layers (partial overlays applied by hold, add, or remove). There is
deliberately no separate per-input "mode shift": a single-input hold-layer expresses everything it
could and more, so adding it would be format complexity for no new capability. The runtime resolves
both the same way - active set as base, active layer stack applied on top - so action sets cost
little beyond layers.

**One uniform node, with plain buttons degenerate.** There is one node type per control. Every
hardware digital bit - face buttons, bumpers, grips, system buttons, and equally the stick and pad
clicks, the pad touches, and the trigger full-pulls - is a plain button that carries bindings and/or
serves as a gate, with no special cases. What a rich behaviour *synthesizes* from its analog input -
a trigger's soft-pull, a directional-pad's directions, a stick's outer ring - is instead a virtual
button living under the source, bindable exactly like a real one but not a hardware bit. The guiding
principle is to avoid special cases wherever the model does not force one.

**A command is an activator, and combos are held, not sequenced.** Every button-like node holds
several commands, each an activator type plus a combo of actions. The actions of a firing command are
held together while it fires rather than played in sequence; the engine emits them as one output
level set per tick, sorted by the output enum's ordinal. Modifier combos come out correct because the
modifier keys occupy the lowest ordinals and so are emitted before the letters they modify, not
because declared order is honored. `interruptible` lives on the `Regular` activator variant rather
than in the shared settings, which makes the invalid combinations unrepresentable. The full activator
semantics (how `Regular`, `Long`, `Double`, `Start`, and `Release` interact) are the engine's to
resolve; config only declares them.

**Two distinct nullifiers.** `SourceBinding::None` is an explicit unbind valid on any source, used
mainly in a layer to override a base binding that would otherwise fall through; in a base set it is
the same as omitting the input. Separately, the `None` variant of `StickOutput` / `TriggerOutput`
drops a behaviour's primary gamepad axis while keeping its virtual buttons (a trigger bound only to a
mouse click, with no phantom axis). `MouseOutput` deliberately has no such variant, because a mouse
behaviour has no virtual buttons - "no output" there is already `SourceBinding::None`, so there is
one clear way rather than a redundant second.

**The behaviour settings are one knob per axis of control.** Shared setting types are defined once
and reused. Response shaping splits by input type: the deflection behaviours (`Joystick`,
`JoystickMouse`, `Trigger`) carry a `Curve` that reshapes a held deflection, while the velocity
behaviours (`AsMouse`, `GyroToMouse`) carry an `Acceleration` that scales by instantaneous speed -
never both, because on a held deflection acceleration would just be a narrower curve, and a relative
delta has no deflection to curve. Smoothing is offered only on the two velocity behaviours, where the
signal is a noisy rate; a stick is already smooth. The `Axis` limit turns a two-dimensional behaviour
into a true one-dimensional control, applied early in the mapper's pipeline rather than as a naive
output null. `Joystick` and `DirectionalPad` are each one shared behaviour valid on both a pad and a
stick, parametrized rather than split; the only pad-versus-stick difference, "requires click", is
expressed through the general activation gate.

**One activation mechanism.** A behaviour's activation is a mode (hold-to-enable or hold-to-disable)
plus a set of physical-button gaters, OR-combined with no thresholds, defaulting to always-active.
The same mechanism expresses gyro gating and a pad's "requires click", which is what lets the shared
behaviours stay single types.

**Command feedback is one medium per edge.** Each action edge (press, release) carries an
independent, optional `Effect`, and each effect is a single medium - a haptic click, an audio tone,
or a chirp - because a click and a tone played at once would fight the same actuator. Feedback fires
on the action's edges, so a long-press cue fires after its timeout and a turbo cue repeats. Doubles
and triples are short patterns whose notes each carry their own strength or pitch, so a pattern can
rise or spell out a rhythm; the chirp has no realization on the original controller, which has no
sweep path.

**Above-profile configuration is two independent beings, each its own file, neither compiled.**
`DeviceConfig` is device-local settings applied on the reader side - LED and idle, and the per-device
rumble shaping - and never crosses the network link, since device settings belong to each machine.
`Chords` is the top-level switch layer, held by the engine as a third slot beside the two profile
roles. The rumble shaping is built from a `Lever` - either a fixed value or the per-motor strength
scaled linearly into a band - with one tuning struct per device because the hardware differs: the
original controller has no motors and rumbles through a pulse train, so its levers are duty and
frequency, while the dual-motor devices use speed and gain. A zero-strength motor is always silent
regardless of the lever, so a band's minimum floors only nonzero strength. There is no global rumble
knob, because each device's levers already scale its own strength. A chord is a set of physical
buttons AND-combined; the engine evaluates chords first and consumes their buttons, so they work even
with no profile loaded. A chord either switches between the main and fallback profile roles or runs a
headless external program - the escape hatch for system actions (an on-screen keyboard, an audio
switch) that keeps the engine itself free of any desktop or audio dependencies.

**RON is kept forgiving.** Settings structs carry struct-level defaults and command lists default to
empty, so a file omits everything it does not override; an older file migrates because a container's
defaults fill in new fields and unknown fields are ignored. Validation checks the kind-validity of
each binding, dangling action-set and layer references, duplicate names, and empty commands, but it
deliberately does *not* check device presence - a binding for an input a device lacks is not an
error, since profiles are device-independent and the engine simply ignores it.


## 6. mapper

The pure mapping core: input state plus a compiled program to output events.

### 6.1 Role and boundaries

mapper is the pure mapping core. Given a per-frame view of controller input and a compiled program,
it produces output events - and feedback requests - deterministically, touching no hardware and no
wall clock.

Its crate wall is what enforces that purity. It links no HAL and no operating system; its only
device-facing dependencies are the two vocabulary crates - it reads the input snapshot from
`vocab-hid` and emits events in `vocab-out` - plus `config`, whose settings and enums are embedded
unchanged in the compiled form, and `serde`, which derives on the IR so it can cross the wire. Its
only notion of time is an injected `Tick`, so a recorded stream of `(frame, Tick)` pairs replays
bit-for-bit and the entire core is golden-testable in isolation.

The same core is driven two ways: the engine's manager loop compiles configuration into a program
and ticks the mapper, and the network sink drives the identical core on the output host. The
compilation step itself lives in the engine (the IR is the runtime's contract), but the IR type is
defined here. The IR derives serialization so it can cross the network link - the controller host
compiles the authoring model into a program and ships that - yet it is never written to disk;
`ConfigDoc` remains the only on-disk form.

### 6.2 Public surface

- **Program** and its parts (`ProgramMeta`, `Role`, `SetId`, `LayerId`, `CompiledSet`,
  `CompiledLayer`, `SourceMap`, `CompiledBinding`, `CompiledCommand`, `CompiledAction`) - the
  compiled, name-resolved, index-based mapping. Layers are per set, and a layer's index in its set
  *is* its declared-order precedence. `empty_program()` is a shared placeholder that maps nothing,
  used when a role has no program applied.
- **LogicalFrame** (and `Dir`) - one controller frame viewed through the logical `InputSource`
  vocabulary over a unified snapshot, with accessors for a button, a button-group member, a
  stick/pad position, a full pad reading, a trigger pull, and the raw gyro/accel. `button_held`
  answers a raw hardware button (for chords and gaters), and `masked()` returns a copy with a
  chord's buttons cleared so profile bindings do not also see them.
- **Mapper** - the retained runtime state. `new()` starts it on a program's default set; `tick()`
  runs one mapping pass, appending output events and feedback requests; `switch_program()` re-seeds
  it for a role swap or a hot-apply while keeping the applied output levels so nothing sticks across
  the swap; `release_all()` drops every currently-held output when the bound device goes away. A few
  read-only getters expose the live layer-stack view.
- **Tick** - the injected monotonic clock stamp (milliseconds since the loop started).
- **FeedbackReq** - one feedback effect with the side of the triggering input and the edge already
  resolved; drained by the owning device's reader, and serializable so it doubles as the engine's
  downlink back-channel over the network.

### 6.3 Design and reasoning

**One acyclic pass per tick.** The layer state is frozen from the previous tick; then, for this
frame, gaters are resolved from the raw frame, the single winning binding per input is resolved,
behaviours evaluate, commands become output-level edges and timers, any layer or set changes are
collected for the *next* tick, and finally the desired output levels are reconciled against what is
applied (emitting only the differences) and relative motion is flushed. Deferring layer and set
changes to the next tick is what keeps the pass acyclic - nothing a tick decides can feed back into
the same tick's resolution - and therefore deterministic.

**Bindings re-resolve every tick; there is no press-time latch.** A binding is resolved as the
highest-precedence active layer that binds the input, else the base binding, else nothing (a silent
layer falls through). This is deliberately Steam-like bleed-through: a held button whose layer or set
changes under it adopts the new binding rather than clinging to the one it was pressed under.
Activator state is keyed by the resolved binding and resets when the winner changes, so a long-press
does not bleed across bindings. Gaters are physical-only, because they must be resolved before
behaviours run; a layer can emulate a virtual gate, at a one-tick cost.

**Output is reconciled by level, not emitted imperatively.** Each tick the mapper computes a desired
level set and diffs it against the last-applied one, emitting only the changes. This is what
guarantees no output can stick - a released or newly-shadowed binding reconciles to neutral - and it
is what makes the core golden-testable on the emitted stream. Relative outputs (mouse motion,
scrolling) accumulate a sub-pixel remainder so fractional deltas integrate faithfully rather than
being lost. `release_all` reconciles against an empty desired set, which drops everything regardless
of what was holding it - a toggle, a latch, a layer - where merely feeding a neutral input frame
would not undo a toggle.

**The activator model is uniform across every button-like node.** Physical buttons and the virtual
buttons a behaviour synthesizes (a trigger soft-pull, a directional-pad direction, an outer ring)
run the same command evaluation. Five activators fall into two roles: `Start` and `Release` are
independent one-shot taps; `Long` and `Double` are the interrupters and hold-takers, each holding
from the moment it fires until release and never contesting one another, so several can be active at
once. `Regular` is the only interruptible command, and its whole behaviour collapses to one rule - it
presses and holds the moment it becomes safe from interruption (no `Long` or `Double` on the node can
still fire). All timing runs off the injected clock.

**Persistent set and stack ops fire once per engagement.** A command that only changes the action
set or the layer stack fires on its rising edge and is then held off by an armed-node latch keyed to
the physical node. Without this a self-toggling button would strobe, because the op flips the node's
own winning binding and would otherwise re-fire every tick it stays held. Keying the latch to the
physical node - not the binding - is what lets it survive both the binding swap the op itself causes
and an action-set change. A `HoldLayer` participates in the same arming; an already-engaged hold
merely continuing is the separate held-layers node-latch, not a re-fire. The mechanism is robust for
triggers that are re-derivable from the raw frame; a trigger derived deep in the behaviour pipeline
cannot stay armed, a user-error corner that can oscillate but never sticks, since reconciliation
always catches up.

**Command feedback is paired to the effect that lands, not to the raw output level.** The mapper
resolves the edge and ships a single feedback request carrying the full effect; a routing step
classifies each command three ways. A level command (any output, an explicit no-op, or a mix) fires
its feedback inline on its own output edges. A persistent-op command defers its feedback and fires
it only if the op actually mutated state this tick - past the dedup, past a same-tick set change
winning, and only on a real insert or remove - so an op that changed nothing stays silent. A hold
command's feedback rides the held layer's real engage and disengage lifecycle, which is what lets an
on-release cue fire even though the command that requested it is self-shadowed on the release tick.

**The motion pipeline has one shared op order for the velocity behaviours.** `AsMouse` (a pad swipe)
and `GyroToMouse` (angular velocity) both run rotate-into-the-output-frame, then mask to the allowed
axis, then optional smoothing, then acceleration on the masked speed, then scale and invert, then
emit. Masking early - before any axis-coupling stage - is what turns a two-dimensional behaviour into
a true one-dimensional control, so the discarded axis' speed cannot inflate the kept axis'
acceleration. The two differ in one intended way: `AsMouse`'s base motion is positional and
time-independent (a finger displacement applies even on a same-millisecond poll), while gyro's is a
rate integrated over the tick. The deflection behaviours instead shape their response with a radial
deadzone and a curve. Smoothing is the per-axis 1-Euro filter, carried only by the two velocity
behaviours; a radial pixel deadzone on gyro removes the resting-bias drift.

**Sign and orientation are settled here.** The controller reports stick and pad Y as positive-up
while the output convention is positive-down, so the mapper flips Y at emission (composed on top of
any per-axis invert), keeping "up on the controller" as "up on screen". Gyro yaw is negated into
horizontal motion for the same left-is-left reason. The remaining backend-specific sign difference
belongs to the output HAL, not here. For gyro, vertical is always local pitch, and horizontal comes
either from a local combination of yaw and roll or, in player space, from projecting the angular
velocity onto a low-passed gravity vector so that turning around real-world vertical maps to
horizontal regardless of how the controller is tilted.


## 7. engine

The headless runtime and manager: owns the devices, compiles configuration, and routes.

### 7.1 Role and boundaries

engine is the headless runtime and manager - the central place the mapper runs, with no UI. It owns
and drives the layers below it: it opens devices through steam-hid, compiles configuration into a
mapper program, runs the mapping loop over the device's input, drives virt-out with the result, and
routes the game-rumble back-channel from the virtual pad to the real controller's haptics.

It is not a pure core - it does real I/O - but the purity is preserved *inside* it, at the mapping
step. The pure mapper is a separate crate, and the engine is the shell that drives it, so the mapping
logic stays golden-testable from recorded traces without hardware. The engine is strictly headless:
it has no concept of running applications or the desktop and links no windowing or desktop-environment
code on Linux. Deciding which profile is active belongs to the UI; the engine just runs whichever
program is active and is told which.

The compile step lives here rather than in config or mapper: `compile(&ConfigDoc) -> Program` is the
runtime's input contract, so it belongs with its consumer, leaving config as pure authoring data.
Its dependencies are `config`, `mapper`, and both HALs (`steam-hid` and `virt-out`), plus
`crossbeam-channel` for the synchronous concurrency and `postcard` for the network-wire codec.
Concurrency is synchronous throughout, with no async runtime. The Windows controller-backend choice
is forwarded from `virt-out` as an engine feature, defaulting to the proven backend.

### 7.2 Public surface

- **Engine** - one owned handle over the runtime, deliberately narrow and transport-agnostic so the
  same calls work embedded or behind a socket. It has no profile concept - just programs, device
  config, and chords. `new()` is idle and acquires no hardware, so hardware errors surface at
  `start()`; `start()` and `stop()` acquire and release the device and sink while configuration is
  retained across the pair; `shutdown()` consumes the handle. `apply()`, `set_chords()`, and
  `set_device_config()` hot-swap live while running and stage while idle; `set_input()` and
  `set_output()` are staged only and take effect at the next start. `status()` returns a snapshot,
  `devices()` enumerates, and `subscribe()` returns an event stream.
- **Input / Output / DeviceSelect** - the staged source and target: a local device selection (auto,
  a transport, or an explicit pinned `DeviceId`) or a network endpoint, each round-tripping through
  its string form so a selection can be reported and passed straight back.
- **Status / StatusInfo** - the run state (idle, running, or waiting-for-device) and a point-in-time
  snapshot read atomically: staged input and output, the bound device, controller presence, battery,
  the whole device config, the loaded program names, the live role, and the chords.
- **EngineEvent / EventStream** - the out-of-band event stream (device lifecycle, battery, binding,
  run-state, live role, staged and applied config, and a monitor-only layer-stack view).
- **compile** - `ConfigDoc` to `Program`, or all diagnostics when any is an error.
- Re-exports of the mapper's `Program` and `Role` (part of the daemon's contract) and the steam-hid
  identity types.

### 7.3 Design and reasoning

**Synchronous, with one reader thread per device feeding a central mapping loop.** A reader thread
owns the `Device` - which is Send but not Sync - and is its single writer; it forwards frames over a
channel to the mapping loop, and commands, rumble, and feedback flow back over channels. This is
where the keep-alive-versus-reader question is resolved: rather than a lock, a second file
descriptor, or a thread inside steam-hid, the reader thread is the one I/O thread that owns the
handle and interleaves the periodic lizard re-assert with its reads and writes, so steam-hid stays
thread-free and single-writer.

**The reader is a persistent device-session loop.** It reads with a short timeout so it laps
regularly to check the run flag, the keep-alive cadence, and pending rumble; it forwards each frame,
surfaces connect/disconnect/battery, applies device settings on start and on every reconnect (a
controller resets its configuration when it rejoins a dongle), re-asserts lizard-off on the cadence,
and emits rumble and command feedback. Across a transport outage it reacquires the same pinned
device and re-mints the session behind the link, so the virtual pad never leaves.

**The mapping loop runs one tick per frame.** It owns the sink, the mapper, the programs, and the
chords, and selects over the frame channel, the control channel, and a periodic timeout (so rumble
is polled even while idle). Chords are evaluated first and consume their trigger buttons before the
frame reaches profile bindings; a chord may flip the main/fallback role - re-seeding the mapper - or
run a headless external command on a detached thread, so the loop never blocks and the engine keeps
no desktop or audio dependencies. The loop emits the mapped output, ships each feedback request to
the reader, and polls the pad's rumble back.

**Roles and program resolution.** The two roles are main and fallback, and the engine always boots
into main. A role resolves to its own program slot, falls through to the other when its own is
unset, and only when both are unset uses an empty placeholder that maps nothing - so the engine runs
before any profile is applied. Applying a program, chords, or device config hot-swaps live while
running and is otherwise retained for the next start.

**Device config is machine-local and reader-side.** Setting it pushes on a channel dedicated to the
reader - deliberately not the control channel, which is the network uplink - so device settings never
cross the wire. The reader rebuilds its per-device config latest-wins and re-applies LED and idle at
once, while the rumble levers are read at emit so a live change lands on the next re-fire. Each
device carries exactly its own tuning variant, so there are no inert fields, and both the rumble emit
and the feedback path dispatch on that variant rather than on the device kind.

**The rumble back-channel** scales the game's force feedback by the active profile's strength and
curve - where the strength may exceed one hundred percent, to lift a game that under-drives its
feedback back toward the actuator's saturation - and sends a level to the reader, which applies the
per-device shaping at emit and re-fires on a per-device cadence.

**Auto-open is transport-aware.** A single or explicit candidate is opened directly - a wired or
Bluetooth device is idle until moved, so there is no point gating on a frame, and an explicit pinned
id is resolved against the current enumeration so it survives the OS path changing across a replug.
Multiple dongle slots, by contrast, are poll-probed to find the one that actually streams, since the
receiver exposes phantom slots whether or not a controller is on. The probe is kept short because
`start()` is synchronous.

**The feedback sequencer** is owned by the reader (the single device writer). It expands a feedback
request's effect into device-independent notes and inter-note intervals, and a per-device step
realizes one note at a time (mapping pitch to frequency, length to duration, and choosing the packet
per device). It fires the first note on arrival, advances the tail as notes come due, and is
replaced latest-wins by a new request. Clicks play on the triggering side, audio on both actuators,
and a chirp is a no-op on the original controller, which has no sweep path.

**Live state is published back to the handle** through atomics - controller presence, battery, and
the live role - each present only in the roles whose thread runs, so a status read needs no
round-trip to the threads. Events are absolute-valued (each carries the full new value), which makes
subscribe-then-seed race-free. The live layer-stack view is the exception that is not mirrored in the
status snapshot: the mapper stays pure (read-only getters) and the runtime diffs a last-emitted
snapshot to emit it, a transient monitor-only view.

### 7.4 Networked operation

**The seam transmits raw input on the wire and runs the mapper on the output side.** A snapshot is
idempotent and latest-wins, which is what makes it safe to send over an unreliable datagram
transport. The client is the controller host (the Deck) - it owns the configuration, connects
outward, and is a thin forwarder when networked - and the server is the output host (the PC) - it
binds once, is headless, maps, and survives reconnection. Both are the same engine artifact; the role
is chosen at runtime by the staged input and output: local-to-local maps here, a network output
forwards this machine's frames to a server, and a network input maps a remote client's frames to the
local sink.

**One link seam serves both the local and the networked case.** It sits between the reader and the
mapping loop, and each end is a local-or-network enum. The local variant wires the two co-located
threads with in-process channels; the network variant adds a second construction behind the same
method surface, so the reader and the mapping loop are written once, transport-free. The seam
exposes channel receivers, so the mapping loop's select composes identically over a local session or
a network one, and every kind of transport loss folds into the same waiting phase the local outage
uses - the pad stays plugged, outputs are released, and the loop resumes on reattach.

**On the wire**, frames ride an unreliable datagram channel (latest-wins, sequence-gated), while
events, control messages, and the initial hello ride a reliable stream; rumble and feedback return
over the datagram channel. The configuration the mapper needs - the main and fallback programs and
the chords - can be set from either side, and when it is set on the client side it travels the
control channel to the server, which retains it across reconnects (configuration is an ordinary
control message, not a handshake step). Device config is the exception: it is reader-side and
machine-local, so it never crosses the wire and each machine keeps its own. The client compiles its
authoring model into a program before the wire - the program form is serializable but is never
written to disk.

**In networked mode the two sides' status views are not kept in sync.** Configuration set on the
client and pushed to the server is applied by the mapper, but it is not reflected back in the
server's own status; and configuration the server holds is not reported to the client. There is no
status or config mirroring across the link in either direction. This is a deliberate simplification -
the mapping itself works, and only the cross-machine reporting is missing, a conscious trade for
simpler, less stateful code, not a bug. It affects networked operation only; a local engine reports
everything faithfully.

### 7.5 Hardware and protocol

**Keep-alive cadence.** The Deck reverts to lizard mode about ten seconds after it is turned off and
Triton after about three; the original controller holds its configuration and needs none. A single
roughly three-second re-assert, gated by the device kind, runs in the reader loop, and the original
controller additionally re-asserts its lizard and gyro state on every reconnect.

**Battery reporting.** The reader publishes the charge in an atomic with an unknown sentinel, so both
"no reader for this role" and "no battery frame yet" read as unknown, and it emits a battery event
only on a genuine change. Battery surfaces for whatever device reports it, regardless of transport;
nothing filters it (the original controller's wired link reports a fixed placeholder rather than real
charge, a firmware quirk, but is not otherwise treated specially).

**Connection presence is seeded per transport at session start.** A wired or Bluetooth link is
point-to-point - the controller is the transport - so it is presumed present the moment the endpoint
opens. A dongle multiplexes a possibly-absent controller and does not reliably announce a current pad
on first open (Triton never retransmits it), so it starts disconnected and the first state or connect
frame edge-fires the connected state.

**Binding events bracket the bound-device lifetime** - acquired at start, removed at stop. A
transport outage and reacquire emit run-state changes rather than binding events, because the device
stays pinned across the outage; the reader reacquires it against its stable identity, which survives
the OS path changing across a replug.


## 8. ipc

The daemon control protocol: requests, responses, and events over a local socket.

### 8.1 Role and boundaries

ipc is the control-plane wire protocol plus the local-socket client and server, shared by the daemon
(`deckhandd`) and its clients (`deckhandctl` and the daemon-mode `deckhand` UI).

It depends on `config` - the messages carry the on-disk `ConfigDoc` - but never on `engine`, and that
boundary is the whole point: a thin client must be able to speak the protocol without linking the
engine and, through it, the HALs. Depending on `deckhandd`'s own crate would drag all of that in
transitively; a dedicated protocol crate keeps both sides' socket code in one place while leaving the
client clean.

It is synchronous. The transport is `interprocess` (a cross-platform local-socket library) - Unix
domain sockets on Unix, named pipes on Windows - and the framing above it is a length-prefixed
`postcard` codec (serde-native compact binary) over any byte stream, so the framing is testable
without a socket.

### 8.2 Public surface

- **Request / Response / Event** - the three message enums. A client sends a `Request` and receives
  one `Response`; after a `Subscribe` request the daemon instead streams `Event`s on that connection
  until it closes.
- **StatusSnapshot / RunState / BoundDevice / ProfileRole** - the payload of the status reply, a
  transport-agnostic mirror of the engine's status types.
- **read_msg / write_msg** - the length-prefixed framing over any reader/writer.
- **Client / Server / Conn** - the local-socket ends. A `Client` calls (send request, read reply),
  subscribes, and reads pushed events; a `Server` yields accepted connections; a `Conn` receives
  requests and writes replies or events. The socket-name helpers (a default path on Unix, a default
  pipe name on Windows) and an adopt-an-existing-listener constructor round it out.

### 8.3 Design and reasoning

**The messages are a wire vocabulary, not the engine's own types.** Because the crate never depends
on engine, it cannot carry the engine's typed device id, role, or status struct; instead the messages
mirror them in serializable forms. Selection specs travel as strings that the daemon parses with the
same grammar as its command line, and they round-trip: the daemon stringifies a typed selection into
the status snapshot, and a client passes that same string straight back to reselect the source.

**The client ships the authoring model; the daemon compiles.** Compilation lives in the engine, and
a thin client must not link it, so applying a profile ships the `ConfigDoc` and the daemon compiles
it, returning diagnostics over the socket when it is rejected. This keeps the client genuinely thin,
and it is the same split as the network seam - configuration travels as data and is compiled on the
far side - so the two reinforce one design.

**A bound device carries its derived shape.** The daemon sends the device's stable id together with
the input-layout shape it derives from the device, so a UI can render device-specific inputs without
re-parsing the id (the engine's typed device-kind does not cross the wire). The two always travel
together, since both come from the one device, so neither is optional; the "nothing bound" case is the
option around the pair, never a missing field.

**Events are absolute-valued, and the status reply is whole.** Every event carries the full new value
of what it reports, and every event mirrors a field of the status snapshot, so a client can seed its
complete view from a single status call and then ride events without a race (subscribe, then seed).
The one exception is the live layer-stack view, which is a transient monitor-only debug view - not
seeded on connect and not mirrored in the snapshot.

**The framing is deliberately trivial and stream-agnostic.** Each message is a small
little-endian length followed by that many compact-binary bytes, over any reader/writer, so it works
over the local socket or any stream and is unit-testable without one. A binary codec is chosen over a
human-readable one for a compact, serde-native low-rate control plane. A read at a clean frame
boundary reports end-of-stream so a read loop can finish gracefully, and a maximum-length guard bounds
the allocation a bogus length could request.

**The transport's only platform-specific code is the socket name.** A unified synchronous local-socket
API sits under both ends, so everything above it - framing, messages, the client and server helpers -
is shared; a single conditional picks the Unix path or the Windows pipe name. The default location is
per-user under the session runtime directory, overridable, so the daemon and its clients always
agree on where to meet.

**Subscribe converts a connection into an event stream** rather than opening a second channel: the
daemon pushes events on that same connection until it closes, so one connection serves one role at a
time. The adopt-an-existing-listener constructor supports socket activation: it takes an
already-bound listener, never touches the filesystem, and carries no reclaim name, so it will not
unlink a socket whose lifecycle a supervisor owns. Isolating the socket setup in this crate is what
makes that a small, safe boundary; the single-instance policy and the concurrent serving belong to
the daemon, not here.


## 9. deckhandd

The daemon: hosts the engine and serves clients.

### 9.1 Role and boundaries

deckhandd is the daemon: it hosts the headless `engine` and serves the control socket for clients
(`deckhandctl` and the daemon-mode `deckhand` UI).

It is one binary with two uses that intermingle freely. As a standalone runner, everything is passed
on the command line and it maps until interrupted; as a controllable daemon, a client connects and
drives it live. It always opens the control socket - the command line merely seeds the engine's
initial state, and clients mutate it from there. It owns `engine`, speaks the wire protocol through
`ipc`, and reads `config`; it forwards the controller-backend choice to `engine`. There is no
self-daemonization: under a supervisor the daemon runs in the foreground and the supervisor manages
its lifecycle.

### 9.2 Public surface and CLI

Being a binary, its surface is the command line, and the seed options map one-to-one to engine calls:
a main and a fallback profile, chords, and device config (each a config file), the input and output
specs, and a flag to acquire hardware and start. Alongside those are `--list-devices` (the one
non-persistent option, handled before any seeding or socket), a socket override, a systemd
socket-activation flag, an idle-inhibitor option on Linux, and verbosity.

Internally a small `Daemon` type wraps the engine and turns one request into one response; the same
seed helpers back both the command line and the socket requests, so the two paths stay identical.

### 9.3 Design and reasoning

**The daemon holds no shadow state - it is just the engine.** Everything the protocol reports (the
staged input and output, which programs are loaded, the live role, controller presence, battery, the
device config and chords) is read back from the engine as one status snapshot, so there is a single
source of truth and the command-line and socket paths cannot drift apart.

**Seeding is pass-through, with one convenience.** Only the options actually given are issued to the
engine - no automagic - and the sole exception is that the start flag defaults a missing input or
output so the engine has a source and a sink. Profiles are never defaulted: starting with no profile
logs a not-ready and keeps serving the socket, exactly as a client asking to start an
under-configured engine would.

**The client ships the authoring model; the daemon compiles.** A profile applied over the socket
arrives as the on-disk config, and the daemon compiles it - applying on success, or returning the
diagnostics on failure. The command-line seed path compiles up front instead. This is the same split
the protocol and the network seam use, so a thin client never links the compiler.

**Serving is one thread per connection over a shared, mutex-guarded daemon.** Each accepted
connection runs on its own thread and briefly locks the engine per request, so a UI holding a
persistent idle connection, an event-stream subscriber, and an occasional one-shot client are all
served concurrently and no long-lived connection can wedge the accept loop. Engine access stays
serialized by the mutex - handling is fast, and the mapping loop runs on its own threads regardless -
and the engine is Send with no unsafe, so this needs none. A subscribe request turns its serve
thread into that client's event pump, forwarding engine events in their wire form until the client or
the engine goes away.

**Single-instance is the bind itself.** Binding the socket is the lock: a failed bind that reaches a
live listener is a genuine second instance and is fatal, while a stale socket file with nothing
listening is removed and rebound. On a clean exit the daemon removes the socket file it created.

**Shutdown is clean on every path.** Ctrl-C and the termination signals flip a flag and self-connect
to unblock the blocking accept; the loop then releases hardware in place - restoring lizard mode and
unplugging the virtual pad. A shutdown request from a client does the same. Tearing down in place
(rather than consuming the daemon) is what lets it live behind the shared mutex.

**Hotplug needs no polling thread.** Enumeration re-scans on demand, and the reader's reacquire poll
runs only while it is waiting for a device to return, so there is no background topology watcher; a
push-style hotplug event was judged unnecessary, since on-demand device listing plus a UI refresh
cover it.

**systemd integration adopts the socket rather than binding one.** Under the systemd flag the daemon
takes the listening socket systemd bound and passed to it, reads the socket's real bound path off it
(for logging and the shutdown self-connect), reports readiness through the systemd notify protocol,
and leaves the socket file for systemd to reap - no stale-socket dance and no cleanup, since systemd
owns its lifecycle. Isolating the socket setup in `ipc` is what makes this a small, safe adoption.
The shipped user units are a service and a socket; enabling the service pulls the socket in. The
Windows service path is a separate concern.

**The idle-inhibitor option keeps a forwarding machine awake.** When a controller's input is consumed
here and forwarded elsewhere, the local compositor sees no activity and would eventually blank or
suspend. On Linux the daemon can hold a D-Bus idle or sleep inhibitor for its lifetime; because no
single interface is implemented everywhere, the mode selects among the screensaver, power-management,
desktop-session, and login-manager services (the default tries several in turn). All are unprivileged
and best-effort - if nothing answers, it warns and runs without one rather than failing.


## 10. deckhandctl

The command-line client.

### 10.1 Role and boundaries

deckhandctl is the thin command-line client: one-shot control commands over the local socket, plus a
follow monitor. Its commands mirror the daemon's API one-to-one.

It depends only on the `ipc` protocol crate and on `config` (to read the RON profiles it ships),
never on `engine`. This is the payoff of the client-ships-config split: the client links no compiler
and no HALs, so it stays small and quick to start.

### 10.2 Public surface and CLI

Being a binary, its surface is the command grammar. A global socket override precedes a sequence of
commands: show status, list devices, stage an input or output spec, load a main or fallback profile,
load chords or device config, start, stop, shut the daemon down, and follow the event stream. The
shutdown and monitor commands are terminal - they end or take over the connection - so they must come
last.

### 10.3 Design and reasoning

**A whole workflow is one invocation, chained over one connection.** The client parses a sequence of
commands and runs them in order over a single connection, since the daemon serves many request/reply
pairs per connection. So loading a game profile, setting a desktop fallback, picking an input, and
starting is one command line rather than four.

**Parse and read everything up front, then fail fast.** All commands are parsed and any profile files
read before the daemon is touched, so a typo or a missing file fails before the chain half-runs. Once
running, each reply is checked and the chain stops on the first error, and every reply is labeled with
the command as typed so a chain's acknowledgements and errors are attributable.

**A profile role is cleared with an empty argument.** Applying a role with an empty string ships a
clear rather than a file, reverting that role live so the other role takes over - clearing the main
role reactivates the fallback. This is what enables a game-launch wrapper: apply a profile, run the
game, then clear it, with the clear carried end to end as an absent config. The above-profile verbs
follow the same shape - they ship their RON and the daemon compiles and applies it, and the chords
verb clears the same way; the client reads and ships, the daemon is authoritative.

**The terminal commands take over the connection.** Shutdown ends the daemon, and monitor turns the
connection into an event stream - subscribing, then printing each pushed event until the stream closes
or the user interrupts - so nothing may follow either in a chain, which is enforced at parse time.

**Output is self-describing.** Status prints an aligned block - run state, the staged input and
output, the bound device and its shape, controller presence, battery, the device config, the loaded
profiles, the live role, and the chords - and device listing prints the stable device ids one per
line, each id being both a display label and the token to pass straight back as an input selection.
The device-config and chord renderings are shared between the stacked status block and the one-line
event stream. The client resolves the socket exactly as the daemon does, so the two always agree on
where to meet.


## 11. deckhand

The graphical application, including the profile editor.

### 11.1 Role and boundaries

deckhand is the graphical application and the project's user-facing binary: a tray and configurator
that drives the daemon over the control protocol, together with the in-app editor for a profile.

It depends on the `ipc` protocol crate (to talk to the daemon), on `config` (the model it edits), and
on both vocabulary crates `vocab-out` and `vocab-hid` (to render the action and button pickers) -
never on `engine`. So the UI links no runtime and no HALs; it reaches the engine only through the
daemon over the socket, exactly like the command-line client. It is a single binary with no library
target, built on an Elm-architecture GUI toolkit, and it hosts a system-tray presence through a small
per-platform companion.

The application has two halves: a configurator that drives the daemon (connect to it, pick the input
and output, manage the above-profile device config and chords, apply profiles, and watch live
status), and the profile editor that authors a profile document.

### 11.2 Structure

The window has four regions: a top daemon bar (the connection and the input/output selection, plus
start and stop), a left sidebar, a scrollable content pane that shows one screen at a time, and a
bottom status bar. The sidebar is two bands - the profile-editor pages on top (disabled until a
profile is loaded, since they edit it) and the always-available application pages below: profile
management, chords, device config, and the app's own settings.

Two pieces face the daemon. A thin blocking command client carries the request/reply calls (status,
list-devices, apply, set-input and set-output, start, stop); it connects lazily and drops its
connection on any error so the next call reconnects. A separate connection, on its own thread, is
subscribed to the event stream and reconnects forever. This split mirrors the protocol, where one
connection serves either request/reply or a terminal event stream.

Everything toolkit-independent is kept out of the widget layer so the widget code stays about
widgets: the application's own settings (persisted to disk, separate from any daemon config), the two
daemon-facing pieces above, the persistence of the UI-owned device config and chords, profile-file
management, the sidebar navigation model, and the profile editor's state and mutation. The editor is
itself split into a mutation side and a view side, with its settings subsystem a file of its own on
each. The system tray is a small per-platform companion.

### 11.3 Design and reasoning

#### The configurator

**The daemon connection tolerates a daemon that comes and goes.** The command client reconnects
transparently, and the event subscription reconnects forever, re-running its on-connect sequence each
time. The subscription reads the application's settings fresh from disk on every attempt, so a
changed option takes effect on the next retry with no state shared between threads.

**A connecting UI seeds its whole view, then rides events.** One status call returns a complete
snapshot, and every subsequent change arrives as an absolute-valued event, so the UI stays correct
even when something else - the command-line client, another UI - drives the same daemon on the side.
This is exactly the contract the protocol was built to offer.

**The UI-owned above-profile config is held in three-way lock-step.** The device config and the
chords are kept in memory as their screens' source of truth, in step with both a local file and the
daemon at once: an edit writes the file and ships the change to the daemon, and a daemon event writes
the file and replaces the in-memory copy. Both directions converge on the same value, so the file, the
UI, and the daemon never drift.

**The application's own settings are separate from any daemon config**, and persisted on every edit:
which controller's inputs the editor shows, the profiles directory (a default or a custom one), and
whether the UI should manage its own daemon. A stale or missing settings or config file never blocks
launch - it falls back to defaults - while a profile that fails to parse surfaces its error with the
path, since that is a file the user is actively working on.

**Managing the daemon is optional.** The UI can drive an already-running daemon, or start and keep
its own; the managed case is what makes a doomed relaunch possible during OS shutdown (see the
platform notes).

**Profile management is distinct from editing.** One screen lists the on-disk profiles and
loads, saves, duplicates, and creates them, and can push a selected file to a role; opening one for
editing is what enables the editor pages.

#### The profile editor

The editor edits a profile document - action sets, layers, per-input bindings, behaviours, commands,
settings, gaters, and the top-level chords.

**Every document change funnels through one mutation site.** This is the load-bearing invariant: a
single update function is the only place the loaded document is mutated. It keeps the model coherent
and is the natural hook for a future undo stack, which becomes a snapshot taken at that one point.

**Sets and layers are addressed by name, not index.** The editor points at an action set (its base
bindings) or one of that set's layers by name; set names are unique among sets and layer names unique
among a set's own siblings, both enforced, so the pair is a stable key that survives reordering and
removal where an index would silently shift. Renaming a set or a layer also repoints the action
references that name it.

**Navigation is static and follows the hardware.** A sub-button - a click, a touch, a trigger
full-pull - nests under its parent input's page rather than a generic buttons page, and a rich
source's page is a header, a behaviour selector, and its sub-buttons.

**The behaviour and command model mirrors the authoring model directly.** A rich source or a button
group picks a behaviour from those valid for its kind; switching behaviour rebuilds from authoring
defaults and discards the old binding's commands, with no attempt to merge. A plain button has no
selector - its command bar is the binding. A slot holds several commands, each an activator plus an
ordered action combo (a main action and its subcommands), and duplicate activators are allowed on
purpose, so one press can drive both a key and a gamepad button. On a layer, each input carries a
three-way state - a real binding, inherited (no entry, falling through to the base), or disabled (an
explicit unbind that nullifies the base) - which makes the override intent explicit.

**Two pickers, tabbed rather than searched.** An action picker, tabbed by category and confirming on
click, returns one action; a single-select button picker over all the raw hardware bits is shared by
the behaviour gaters and the chords, since both are sets of raw controller buttons - and both are
edited as removable chips with an add control that opens that one picker.

**Settings are a full-page sub-route, not a modal.** Command settings and per-behaviour settings share
one shape and render in place of the current input page, reached from a gear menu and left with a back
control. A full page was chosen over a modal so the pattern scales to the larger behaviour-settings
forms and so "which command or behaviour" stays navigation state, cleared by any navigation so a
reference never outlives its target.

**Per-behaviour settings are a building-block framework, not a bespoke page each.** There is a
reusable block per setting element - deadzone, curve, sensitivity, smoothing, activation, and the
rest - composed by a small per-behaviour function in one canonical field order. The blocks emit a
single generic set-setting message with one variant per field, applied through small typed accessors,
so the mutation is not duplicated across behaviours and adding a behaviour is one compose function.
The typed config structs stay the source of truth, and the canonical field order is kept in lock-step
across those struct definitions (which drive the on-disk field order), the example literals, and the
compose functions.

**Config owns the data; the UI owns the authoring.** config is the data model, its serialization, and
its validation; the UI owns all construction of config values - empty-profile creation, the
context-tuned authoring defaults a freshly-picked behaviour gets (distinct from the neutral on-disk
defaults), and the behaviour tag. The human-facing labels for the vocabularies are UI-owned too, so
both vocabulary crates stay presentation-free.

**Inputs a device lacks are filtered out, not greyed.** A show-inputs setting selects which device's
inputs to display; on automatic it follows the bound device's shape - which the daemon sends on the
wire beside the device id - and falls back to showing everything when nothing is bound. An unreported
input is omitted, and a group that empties drops its header.

**Save is automatic and validation is by construction.** Edits autosave with no dirty flag (the files
are small and the option set closed), and undo is deferred but cheap to add thanks to the single
mutation site. The editor makes invalid states unrepresentable - it offers only behaviours valid for
a source's kind, and gaters and chords are typed raw buttons so "not a physical button" cannot be
expressed - while allowing valid-but-no-effect combinations; compile-time validation remains the
backstop at apply time, so there is no live-diagnostics panel.

**Two "set as main/fallback" paths are intentional.** The top-bar buttons send the in-memory edited
profile, for editing and testing immediately; the profiles-page grid sends the selected on-disk file,
for daemon management. They serve different workflows and are deliberately not merged.

### 11.4 Platform notes

The system tray is per platform, avoiding a shared-toolkit conflict with the renderer: a pure-Rust
status item over D-Bus on Linux, and the native tray on Windows driven from its own message-loop
thread. The tray and window icons derive from one shared image.

On Windows the binary is built as a GUI-subsystem application, so launching never spawns a console
window; errors surface in the status bar rather than on a stream. A Windows-specific hardening
concerns OS shutdown: on shutdown or logoff a managed UI can briefly relaunch a daemon that the OS is
already killing, and the toolkit surfaces no session-end event to stop it cleanly, so the process
error mode is set at startup to make that doomed launch fail silently instead of raising a hard-error
dialog.


## 12. forwarder-ui

The touch-first quick-connect application for running a machine as a network forwarder.

### 12.1 Role and boundaries

forwarder-ui (the `deckhand-forwarder` binary) is a minimal, touch-first "quick connect" companion
for running a machine as a network forwarder: pick a local controller as the input, type a host and
port as the output, and press Start.

It exists for the Steam Deck's touch screen, forwarding controller input to a PC over the LAN - the
client role of the networked engine. On an ordinary PC the full deckhand UI already does the client
job, since it can set a network output; the forwarder is only for the Deck's touch-driven
quick-connect case, where the full UI is awkward to operate by touch.

Like `deckhandctl` and the main `deckhand` UI, it depends on the `ipc` protocol crate and `config`
and never on `engine` - it drives the daemon over the socket.

### 12.2 Structure

It is a single, always-visible window with no tray and no sidebar: the daemon controls (an input
selection and a numeric output field), an on-screen keypad for the address, a per-device rumble
control, and a status bar. It reuses the parts of the main UI it needs - the daemon command client
and resilient connect loop (trimmed to a fixed policy), the RON persistence, and a few styles - as a
deliberate copy rather than through a shared crate, given how narrow its scope is. Its own settings
file remembers only what it must between runs (the theme, the window size, and the last input and
output), and it shares the device-config file with the main UI, loading the whole structure and
editing only the bound device's rumble.

### 12.3 Design and reasoning

**One fixed daemon policy, no settings to choose.** Where the main UI lets the connection behaviour
be configured, the forwarder bakes in the single policy its one job needs: start the daemon if it is
not running (running a client on the Deck is the whole point), spawned so the machine does not
idle-suspend while forwarding; restore only the last input; push the device config on connect; and
never auto-start - Start is always a deliberate press, and the output address is applied verbatim at
that press rather than on every keystroke.

**The whole interface is drawn at a uniform enlarged scale.** The Deck's touch display makes
default-sized widgets too small to hit reliably, so the entire UI is scaled up as a render scale -
text, buttons, dropdowns, and padding all grow together - and the window opens near the Deck's native
size, with the content scrolling if the scaled layout overflows.

**Display follows the daemon's events; the output field belongs to the user.** What the window shows -
run state, controller presence, the bound device, the input - is driven by the daemon's event stream,
while the output address field is user-owned and never overwritten by an event, so typing is never
disturbed by a status update.

**The address is entered on an on-screen keypad.** A phone-layout numeric keypad (the digits, a dot,
a colon, and a backspace) edits the field regardless of widget focus, so the entire flow is
completable by touch alone.

**It duplicates rather than shares.** It copies the slice of the main UI it reuses instead of
factoring out a common crate, because the tool is small and single-purpose; sharing the device-config
*file* (not the code) is what keeps its rumble edits consistent with the main UI without coupling the
two binaries.
