//! `compile()` - [`config::ConfigDoc`] -> [`Program`] (PLAN 4.2 S2).
//!
//! The bridge between authoring data and the runtime IR. It:
//! 1. runs [`ConfigDoc::validate`] and **bails on any `Error`-severity diagnostic**
//!    (warnings don't block - the UI surfaces those itself);
//! 2. resolves `ActionSetRef`/`LayerRef` **names -> ids** ([`SetId`]/[`LayerId`]); layer ids
//!    are per-set and *are* the declared-order precedence;
//! 3. mirrors each [`config::SourceBinding`] into a [`CompiledBinding`], reusing settings
//!    verbatim and rewriting only the ref-carrying actions.
//!
//! Post-validation the refs are guaranteed to resolve, so resolution uses `expect` on that
//! invariant - a failure means `validate()` and `compile()` disagree (a bug), not bad input.
//! The `InputSource -> ControllerState` mapping is **not** here - it's the device-specific
//! runtime table (S3); `compile` stays device-independent, like `config`.

use std::collections::{BTreeMap, HashMap};

use config::{
    Action, ActionSet, Command, ConfigDoc, Diagnostic, InputSource, Layer, Severity, SourceBinding,
};

use crate::program::{
    CompiledAction, CompiledBinding, CompiledCommand, CompiledLayer, CompiledSet, LayerId, Program,
    ProgramMeta, Role, SetId, SourceMap,
};

/// Name-resolution context for one action set: set names are program-global, layer names are
/// scoped to the set being compiled (PLAN 4 - layers are per-set).
struct Names<'a> {
    sets: &'a HashMap<&'a str, SetId>,
    layers: HashMap<&'a str, LayerId>,
}

/// Compile a profile into the runtime [`Program`]. `Err` carries **all** diagnostics (so the
/// caller/UI can show them) when any is an `Error`. The result is tagged [`Role::Main`];
/// `Engine::apply` re-tags it to the slot it lands in (PLAN 4.1).
pub fn compile(doc: &ConfigDoc) -> Result<Program, Vec<Diagnostic>> {
    let diags = doc.validate();
    if diags.iter().any(|d| d.severity == Severity::Error) {
        return Err(diags);
    }

    let set_ids: HashMap<&str, SetId> = doc
        .action_sets
        .iter()
        .enumerate()
        .map(|(i, a)| (a.name.as_str(), SetId::new(i)))
        .collect();

    let sets = doc.action_sets.iter().map(|a| compile_set(a, &set_ids)).collect();

    Ok(Program {
        meta: ProgramMeta { name: doc.name.clone(), role: Role::Main },
        sets,
        default_set: SetId::new(0),
        rumble: doc.rumble.clone(),
    })
}

fn compile_set(set: &ActionSet, set_ids: &HashMap<&str, SetId>) -> CompiledSet {
    let layer_ids: HashMap<&str, LayerId> = set
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| (l.name.as_str(), LayerId::new(i)))
        .collect();
    let names = Names { sets: set_ids, layers: layer_ids };

    CompiledSet {
        name: set.name.clone(),
        base: compile_bindings(&set.bindings, &names),
        layers: set.layers.iter().map(|l| compile_layer(l, &names)).collect(),
    }
}

fn compile_layer(layer: &Layer, names: &Names) -> CompiledLayer {
    CompiledLayer { name: layer.name.clone(), bindings: compile_bindings(&layer.bindings, names) }
}

fn compile_bindings(
    bindings: &BTreeMap<InputSource, SourceBinding>,
    names: &Names,
) -> SourceMap<CompiledBinding> {
    bindings.iter().map(|(src, b)| (src.clone(), compile_binding(b, names))).collect()
}

fn compile_binding(binding: &SourceBinding, names: &Names) -> CompiledBinding {
    let cc = |cmds: &[Command]| compile_commands(cmds, names);
    match binding {
        SourceBinding::Button { commands } => CompiledBinding::Button { commands: cc(commands) },
        SourceBinding::ButtonPad { up, down, left, right } => CompiledBinding::ButtonPad {
            up: cc(up),
            down: cc(down),
            left: cc(left),
            right: cc(right),
        },
        SourceBinding::Joystick { settings, outer_ring } => CompiledBinding::Joystick {
            settings: settings.clone(),
            outer_ring: cc(outer_ring),
        },
        SourceBinding::DirectionalPad { settings, up, down, left, right, outer_ring } => {
            CompiledBinding::DirectionalPad {
                settings: settings.clone(),
                up: cc(up),
                down: cc(down),
                left: cc(left),
                right: cc(right),
                outer_ring: cc(outer_ring),
            }
        }
        SourceBinding::AsMouse { settings } => {
            CompiledBinding::AsMouse { settings: settings.clone() }
        }
        SourceBinding::JoystickMouse { settings } => {
            CompiledBinding::JoystickMouse { settings: settings.clone() }
        }
        SourceBinding::GyroToMouse { settings } => {
            CompiledBinding::GyroToMouse { settings: settings.clone() }
        }
        SourceBinding::Trigger { settings, soft_pull } => CompiledBinding::Trigger {
            settings: settings.clone(),
            soft_pull: cc(soft_pull),
        },
        SourceBinding::None => CompiledBinding::None,
    }
}

fn compile_commands(commands: &[Command], names: &Names) -> Vec<CompiledCommand> {
    commands
        .iter()
        .map(|c| CompiledCommand {
            activator: c.activator.clone(),
            actions: c.actions.iter().map(|a| compile_action(a, names)).collect(),
            settings: c.settings.clone(),
        })
        .collect()
}

fn compile_action(action: &Action, names: &Names) -> CompiledAction {
    match action {
        Action::None => CompiledAction::None,
        Action::Key(k) => CompiledAction::Key(k.clone()),
        Action::MouseButton(b) => CompiledAction::MouseButton(b.clone()),
        Action::GamepadButton(b) => CompiledAction::GamepadButton(b.clone()),
        Action::ChangeActionSet(r) => CompiledAction::ChangeActionSet(resolve_set(names, &r.0)),
        Action::HoldLayer(r) => CompiledAction::HoldLayer(resolve_layer(names, &r.0)),
        Action::AddLayer(r) => CompiledAction::AddLayer(resolve_layer(names, &r.0)),
        Action::RemoveLayer(r) => CompiledAction::RemoveLayer(resolve_layer(names, &r.0)),
    }
}

fn resolve_set(names: &Names, name: &str) -> SetId {
    names.sets.get(name).cloned().expect("validate() guarantees the action-set ref resolves")
}

fn resolve_layer(names: &Names, name: &str) -> LayerId {
    names.layers.get(name).cloned().expect("validate() guarantees the layer ref resolves")
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{Activator, LayerRef};
    use vocab_out::{GamepadButton, Key};

    fn press(action: Action) -> Command {
        Command { activator: Activator::Regular { interruptible: false }, actions: vec![action], settings: Default::default() }
    }

    /// A two-set profile with a layer and cross-references, to exercise id resolution.
    fn sample() -> ConfigDoc {
        let mut game = BTreeMap::new();
        game.insert(
            InputSource::LeftBumper,
            SourceBinding::Button { commands: vec![press(Action::HoldLayer(LayerRef("aim".into())))] },
        );
        game.insert(
            InputSource::Menu,
            SourceBinding::Button {
                commands: vec![press(Action::ChangeActionSet(config::ActionSetRef("Drive".into())))],
            },
        );
        game.insert(
            InputSource::FaceButtons,
            SourceBinding::ButtonPad {
                up: vec![press(Action::GamepadButton(GamepadButton::Y))],
                down: vec![press(Action::Key(Key::Space))],
                left: vec![],
                right: vec![],
            },
        );

        let aim = Layer {
            name: "aim".into(),
            bindings: BTreeMap::from([(
                InputSource::RightPad,
                SourceBinding::AsMouse { settings: Default::default() },
            )]),
        };

        ConfigDoc {
            version: 0,
            name: "Sample".into(),
            action_sets: vec![
                ActionSet { name: "Game".into(), bindings: game, layers: vec![aim] },
                ActionSet { name: "Drive".into(), bindings: BTreeMap::new(), layers: vec![] },
            ],
            rumble: Default::default(),
        }
    }

    #[test]
    fn resolves_names_to_ids() {
        let prog = compile(&sample()).expect("valid");
        assert_eq!(prog.meta.name, "Sample");
        assert_eq!(prog.meta.role, Role::Main);
        assert_eq!(prog.default_set.index(), 0);
        assert_eq!(prog.sets.len(), 2);

        let game = prog.set(&SetId::new(0));
        assert_eq!(game.name, "Game");
        assert_eq!(game.layers.len(), 1);

        // HoldLayer("aim") -> LayerId(0) within Game.
        let lb = game.base.get(&InputSource::LeftBumper).unwrap();
        match lb {
            CompiledBinding::Button { commands } => {
                assert_eq!(commands[0].actions, vec![CompiledAction::HoldLayer(LayerId::new(0))]);
            }
            _ => panic!("expected Button"),
        }

        // ChangeActionSet("Drive") -> SetId(1).
        let menu = game.base.get(&InputSource::Menu).unwrap();
        match menu {
            CompiledBinding::Button { commands } => {
                assert_eq!(
                    commands[0].actions,
                    vec![CompiledAction::ChangeActionSet(SetId::new(1))]
                );
            }
            _ => panic!("expected Button"),
        }

        // Output leaves pass through unchanged.
        let face = game.base.get(&InputSource::FaceButtons).unwrap();
        assert!(matches!(face, CompiledBinding::ButtonPad { .. }));
    }

    #[test]
    fn dangling_ref_fails_compile() {
        let mut doc = sample();
        // Point the bumper at a layer that doesn't exist.
        doc.action_sets[0].bindings.insert(
            InputSource::LeftBumper,
            SourceBinding::Button { commands: vec![press(Action::HoldLayer(LayerRef("nope".into())))] },
        );
        let err = compile(&doc).unwrap_err();
        assert!(err.iter().any(|d| d.severity == Severity::Error));
    }
}
