//! `mapper` - the pure mapping core (PLAN 4.1/4.2). Evaluates a compiled [`Program`] over per-frame
//! controller input ([`LogicalFrame`]) into output events, deterministically: an injected [`Tick`]
//! is its only notion of time, and it links no HAL and no OS, so it is golden-testable in isolation.
//! The engine compiles config into a `Program` and drives the [`Mapper`]; the network sink drives
//! the same core.
//!
//! Modules: `mapper` (the `Mapper` + per-tick pass), plus the `program` (the compiled IR) and
//! `logical` (the per-frame input view) it consumes - all re-exported flat below.

mod activator;
mod behavior;
mod command;
mod gyro;
mod layers;
mod logical;
mod mapper;
mod program;
mod reconcile;
mod smooth;

pub use logical::{Dir, LogicalFrame};
pub use mapper::{HapticReq, Mapper, Tick};
pub use program::{
    CompiledAction, CompiledBinding, CompiledCommand, CompiledLayer, CompiledSet, LayerId, Program,
    ProgramMeta, Role, SetId, SourceMap, empty_program,
};
