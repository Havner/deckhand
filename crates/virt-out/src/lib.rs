//! `virt-out` — the output HAL (PLAN §2). Emits virtual mouse/keyboard/gamepad input
//! to the OS. Platform code sits behind an internal backend trait; the public API is a
//! platform-agnostic sink plus the output vocabulary. Sync (no async).
//!
//! Status: Phase A scaffold (Linux uinput backend). See PLAN §2.1.

#![cfg(target_os = "linux")]
