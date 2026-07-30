//! The runtime IR — a compiled, name-resolved mapping (PLAN §4.1/§4.2 S1).
//!
//! [`Program`] is the engine's **input contract**: the flattened, index-based form the
//! mapper runs each tick. `compile()` (S2) turns a [`config::ConfigDoc`] into one —
//! resolving `ActionSetRef`/`LayerRef` names to [`SetId`]/[`LayerId`], and mirroring each
//! [`config::SourceBinding`] into a [`CompiledBinding`]. Settings are **reused from
//! `config`** (they are already runtime-ready `f32`s); the only thing that changes shape is
//! the ref-carrying [`config::Action`] → id-carrying [`CompiledAction`].
//!
//! Layers are **per-set**, and a layer's index in [`CompiledSet::layers`] *is* its
//! declared-order precedence (PLAN §4). Not serde — this is runtime state, never persisted.

use std::collections::BTreeMap;

use config::{
    AsMouseSettings, Activator, CommandSettings, DirectionalPadSettings, GyroToMouseSettings,
    InputSource, JoystickMouseSettings, JoystickSettings, RumbleSettings, TriggerSettings,
};
use vocab::{GamepadButton, Key, MouseButton};

/// Which of the engine's two live slots a [`Program`] occupies (PLAN §4.1). The engine
/// self-switches `Main`↔`Fallback` via a global chord. (`Main` was `Active`, renamed since the
/// fallback is what's "active" while it runs — the name was backwards.)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Role {
    Main,
    Fallback,
}

/// Index of an action set within a [`Program`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SetId(u16);

impl SetId {
    pub fn new(index: usize) -> Self {
        SetId(index as u16)
    }
    pub fn index(&self) -> usize {
        self.0 as usize
    }
}

/// Index of a layer within its [`CompiledSet`]. Layers are per-set, and this index **is**
/// the layer's declared-order precedence (PLAN §4: higher index = higher precedence).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerId(u16);

impl LayerId {
    pub fn new(index: usize) -> Self {
        LayerId(index as u16)
    }
    pub fn index(&self) -> usize {
        self.0 as usize
    }
}

/// Metadata carried alongside a compiled program (name for UI/status, slot role).
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramMeta {
    pub name: String,
    pub role: Role,
}

/// A compiled, name-resolved mapping — the engine's runtime input contract (PLAN §4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub meta: ProgramMeta,
    pub sets: Vec<CompiledSet>,
    /// The set active on load (the profile's first action set).
    pub default_set: SetId,
    /// Per-profile rumble feel (pulse Hz, strength %, curve) — the manager applies these to the
    /// game→controller haptics for whichever program is the active role (PLAN §3 Round E).
    pub rumble: RumbleSettings,
}

impl Program {
    /// The action set at `id` (panics on an out-of-range id — ids only come from `compile`).
    pub fn set(&self, id: &SetId) -> &CompiledSet {
        &self.sets[id.index()]
    }
}

/// One action set: base bindings + its layers (index = declared-order precedence).
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledSet {
    pub name: String,
    pub base: SourceMap<CompiledBinding>,
    pub layers: Vec<CompiledLayer>,
}

impl CompiledSet {
    /// The layer at `id`.
    pub fn layer(&self, id: &LayerId) -> &CompiledLayer {
        &self.layers[id.index()]
    }
}

/// A layer: the subset of inputs it overrides (PLAN §4 — silent inputs fall through).
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledLayer {
    pub name: String,
    pub bindings: SourceMap<CompiledBinding>,
}

/// A per-[`InputSource`] lookup. **Opaque** so the representation (a `BTreeMap` now, a dense
/// array later) can change without touching the mapper. Iteration is deterministic
/// (`InputSource` order).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMap<T>(BTreeMap<InputSource, T>);

impl<T> SourceMap<T> {
    pub fn new() -> Self {
        SourceMap(BTreeMap::new())
    }
    pub fn get(&self, source: &InputSource) -> Option<&T> {
        self.0.get(source)
    }
    pub fn insert(&mut self, source: InputSource, value: T) -> Option<T> {
        self.0.insert(source, value)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&InputSource, &T)> {
        self.0.iter()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<T> Default for SourceMap<T> {
    fn default() -> Self {
        SourceMap::new()
    }
}

impl<T> FromIterator<(InputSource, T)> for SourceMap<T> {
    fn from_iter<I: IntoIterator<Item = (InputSource, T)>>(iter: I) -> Self {
        SourceMap(iter.into_iter().collect())
    }
}

/// Runtime form of a [`config::SourceBinding`]: settings reused from `config` (already
/// runtime `f32`s), commands with their action refs resolved to ids (PLAN §4.1).
#[derive(Debug, Clone, PartialEq)]
pub enum CompiledBinding {
    /// A standalone button: its multi-activator commands.
    Button { commands: Vec<CompiledCommand> },
    /// A button group in `ButtonPad` mode: its four member buttons.
    ButtonPad {
        up: Vec<CompiledCommand>,
        down: Vec<CompiledCommand>,
        left: Vec<CompiledCommand>,
        right: Vec<CompiledCommand>,
    },
    /// Pad/Stick → gamepad stick, plus an outer-ring virtual button.
    Joystick { settings: JoystickSettings, outer_ring: Vec<CompiledCommand> },
    /// Pad/Stick → four direction + outer-ring virtual buttons.
    DirectionalPad {
        settings: DirectionalPadSettings,
        up: Vec<CompiledCommand>,
        down: Vec<CompiledCommand>,
        left: Vec<CompiledCommand>,
        right: Vec<CompiledCommand>,
        outer_ring: Vec<CompiledCommand>,
    },
    /// Pad → cursor/scroll (no virtual buttons).
    AsMouse { settings: AsMouseSettings },
    /// Stick → cursor/scroll (no virtual buttons).
    JoystickMouse { settings: JoystickMouseSettings },
    /// Gyro → cursor/scroll (no virtual buttons).
    GyroToMouse { settings: GyroToMouseSettings },
    /// Trigger → gamepad trigger, plus a soft-pull virtual button.
    Trigger { settings: TriggerSettings, soft_pull: Vec<CompiledCommand> },
    /// Explicitly unbound — produces no output (overrides a base binding when used in a layer).
    None,
}

/// A command with its action refs resolved. `activator`/`settings` are reused from `config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledCommand {
    pub activator: Activator,
    pub actions: Vec<CompiledAction>,
    pub settings: CommandSettings,
}

/// Runtime form of a [`config::Action`]: output leaves unchanged (they name `vocab`
/// targets `virt-out` realizes), mode refs resolved to ids (PLAN §4.1 / Round D).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompiledAction {
    None,
    Key(Key),
    MouseButton(MouseButton),
    GamepadButton(GamepadButton),
    ChangeActionSet(SetId),
    HoldLayer(LayerId),
    AddLayer(LayerId),
    RemoveLayer(LayerId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_index() {
        assert_eq!(SetId::new(3).index(), 3);
        assert_eq!(LayerId::new(0).index(), 0);
    }

    #[test]
    fn source_map_lookup_and_default() {
        let mut m: SourceMap<CompiledBinding> = SourceMap::default();
        assert!(m.is_empty());
        m.insert(InputSource::LeftBumper, CompiledBinding::Button { commands: vec![] });
        assert_eq!(m.len(), 1);
        assert!(m.get(&InputSource::LeftBumper).is_some());
        assert!(m.get(&InputSource::RightBumper).is_none());
    }

    #[test]
    fn program_set_and_layer_access() {
        let prog = Program {
            meta: ProgramMeta { name: "p".into(), role: Role::Main },
            default_set: SetId::new(0),
            rumble: Default::default(),
            sets: vec![CompiledSet {
                name: "Game".into(),
                base: SourceMap::from_iter([(
                    InputSource::LeftBumper,
                    CompiledBinding::Button {
                        commands: vec![CompiledCommand {
                            activator: Activator::Regular,
                            actions: vec![CompiledAction::HoldLayer(LayerId::new(0))],
                            settings: Default::default(),
                        }],
                    },
                )]),
                layers: vec![CompiledLayer { name: "aim".into(), bindings: SourceMap::new() }],
            }],
        };
        assert_eq!(prog.set(&prog.default_set).name, "Game");
        assert_eq!(prog.set(&SetId::new(0)).layer(&LayerId::new(0)).name, "aim");
    }
}
