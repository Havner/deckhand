//! Engine error type (PLAN §4). Covers compile failures, hardware/IO from the HALs, and
//! control-API misuse. Kept small and `thiserror`-based; grows as the steps land.

use config::Diagnostic;

/// The engine's result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Anything the engine can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A [`config::ConfigDoc`] failed to compile — carries the collected diagnostics
    /// (PLAN §4.2 S2; `compile()` bails on any `Error`-severity diagnostic).
    #[error("config did not compile: {} diagnostic(s)", .0.len())]
    Compile(Vec<Diagnostic>),

    /// Input HAL failure (device open/read/command).
    #[error("input device: {0}")]
    Input(#[from] steam_hid::Error),

    /// Output HAL failure (virtual device / sink).
    #[error("output sink: {0}")]
    Output(#[from] virt_out::Error),

    /// The control API was used out of order (e.g. `start()` with no program applied, or no
    /// matching input device found).
    #[error("engine not ready: {0}")]
    NotReady(&'static str),
}
