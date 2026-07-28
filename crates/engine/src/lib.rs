//! `engine` — the deckhand headless runtime / manager (PLAN §4).
//!
//! The central place the mapper runs, with no UI. It owns and drives the hardware layers
//! below it: opens devices via [`steam_hid`], runs a compiled [`Program`](program::Program)
//! over their input, and emits the result through [`virt_out`]. It holds the live runtime
//! state (active action set/layer, activator timers, gyro integration, accumulators).
//!
//! **Not a pure core** — it does real I/O. Purity is kept *inside*, at the mapping step:
//! a pure `Mapper::tick(input, clock) → outputs` that the manager shell drives each tick,
//! so the mapping logic is golden-testable from recorded traces without hardware
//! (PLAN §4.1). The crate is strictly **headless**: no X/Wayland/DE linkage.
//!
//! Built in steps (PLAN §4.2). This is **S0 — scaffold**: the crate, its error type, and
//! the module map. Each subsequent step adds its module and re-exports:
//! - `program`  — the runtime IR (`Program`) — S1
//! - `compile`  — `compile(&ConfigDoc) -> Result<Program, Vec<Diagnostic>>` — S2
//! - `logical`  — `ControllerState -> LogicalFrame` (per `DeviceKind`) — S3
//! - `mapper`   — the pure per-tick `Mapper` (`reconcile`/`resolve`/`behavior`/`command`) — S4–S8
//! - `runtime`  — reader thread(s) + central `select!` loop (threads/channels) — S9
//! - `handle`   — the `Engine` control API — S10

mod error;

pub use error::{Error, Result};
