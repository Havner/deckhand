//! `getters` — exercise the read-only GET round-trips, showing **what was requested vs what came
//! back**: serials (`GET_STRING_ATTRIBUTE`), read-only attributes (`GET_ATTRIBUTES_VALUES`), and a
//! few settings (`GET_SETTINGS_VALUES`). Safe reads only, no writes.
//!
//! Attributes and settings are both `const` ids (per the protocol.rs const-vs-enum rule — inbound /
//! dispatched-on tags are consts, not enums), so they're labelled by name against a local `(name, id)`
//! table. HW-verified on Gordon (dongle). The dongle's feature endpoint is flaky under back-to-back
//! I/O, so the getters re-send + settle + validate each reply (see `Device::get_roundtrip`).
//!
//! Gordon-focused; **USB only** (dongle/wired). `--wired`/`--dongle` pick the transport. Run:
//!   `cargo run -p steam-hid --example getters -- [--dongle]`

mod common;

use steam_hid::{ControllerStringAttributes, Manager, Result};

/// Read-only attribute tags — `(name, id)` from `protocol::attribute` (a private const module, so
/// spelled out). `GET_ATTRIBUTES_VALUES` returns the full set; this names each tag that comes back.
const ATTRIBUTES: [(&str, u8); 12] = [
    ("UniqueId", 0),
    ("ProductId", 1),
    ("Capabilities", 2),
    ("FirmwareVersion", 3),
    ("FirmwareBuildTime", 4),
    ("RadioFirmwareBuildTime", 5),
    ("RadioDeviceId0", 6),
    ("RadioDeviceId1", 7),
    ("DongleFirmwareBuildTime", 8),
    ("BoardRevision", 9),
    ("BootloaderBuildTime", 10),
    ("ConnectionIntervalInUs", 11),
];

/// Settings to request — `(name, id)` from `protocol::setting` (a private const module, so spelled
/// out): 45 LED_USER_BRIGHTNESS, 48 IMU_MODE, 50 SLEEP_INACTIVITY_TIMEOUT, 71 STEAM_WATCHDOG_ENABLE.
const WANT_SETTINGS: [(&str, u8); 4] = [
    ("LedUserBrightness", 45),
    ("ImuMode", 48),
    ("SleepInactivityTimeout", 50),
    ("SteamWatchdogEnable", 71),
];

fn main() -> Result<()> {
    let mut manager = Manager::new()?;
    let Some((desc, mut device)) = common::select_device(&mut manager)? else {
        println!("No matching controller found — connected/on?");
        return Ok(());
    };
    println!("selected {desc}");

    println!("\n== serials  (GET_STRING_ATTRIBUTE 0xAE) ==");
    for tag in [ControllerStringAttributes::UnitSerial, ControllerStringAttributes::BoardSerial] {
        match device.get_string_attribute(tag) {
            Ok(s) => println!("  {tag:?}: {s:?}"),
            Err(e) => println!("  {tag:?}: <error: {e}>"),
        }
    }

    println!("\n== attributes  (GET_ATTRIBUTES_VALUES 0x83 — full set) ==");
    match device.get_attributes() {
        Ok(attrs) => {
            for (tag, value) in attrs {
                match ATTRIBUTES.iter().find(|&&(_, id)| id == tag).map(|&(name, _)| name) {
                    Some(name) => println!("  {name} ({tag}) = {value:#010x} ({value})"),
                    None => println!("  tag {tag} = {value:#010x} ({value})"),
                }
            }
        }
        Err(e) => println!("  <error: {e}>"),
    }

    println!("\n== settings  (GET_SETTINGS_VALUES 0x89) ==");
    let ids: Vec<u8> = WANT_SETTINGS.iter().map(|&(_, id)| id).collect();
    match device.get_settings(&ids) {
        Ok(vals) => {
            for &(name, id) in &WANT_SETTINGS {
                match vals.iter().find(|&&(i, _)| i == id).map(|&(_, v)| v) {
                    Some(v) => println!("  {name} ({id}) = {v}"),
                    None => println!("  {name} ({id}) = <not reported>"),
                }
            }
        }
        Err(e) => println!("  <error: {e}>"),
    }

    Ok(())
}
