//! `vocab-out` - the hardware-independent **output vocabulary**, shared by `config` (which names
//! the target of a binding) and `virt-out` (which realizes it). The output counterpart of
//! `vocab-hid` (inputs). PLAN 2.1 / 3.
//!
//! Split into two modules purely for clarity about who needs what (everything is re-exported flat
//! at the crate root, so consumers see one namespace):
//! - [`core`] - the leaf target enums (`Key` / `MouseButton` / `GamepadButton` / `GamepadAxis`) a
//!   binding can name. **What `config` needs.**
//! - [`event`] - the [`OutputEvent`] batch item built from them. **What the mapper additionally
//!   needs** (and hands `virt-out`'s `Sink::emit`).

mod core;
mod event;

// core: the config-facing vocabulary.
pub use core::{GamepadAxis, GamepadButton, Key, MouseButton, SCROLL_HI_RES_PER_DETENT};
// event: the mapper-facing emit item.
pub use event::OutputEvent;
