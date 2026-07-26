//! Validation — **collect-all** diagnostics over a [`ConfigDoc`] / [`GlobalConfig`]
//! (PLAN §3). Not fail-fast: the UI surfaces every problem at once. This is where the
//! (deferred) `compile()` will hook in; today it stops at diagnostics.
//!
//! It does **not** check device presence — profiles are device-independent, so a binding
//! for an input a device lacks is not an error (the engine ignores it; the UI may warn via
//! `Shape`).

use std::collections::{BTreeMap, HashSet};

use crate::action::Action;
use crate::binding::SourceBinding;
use crate::global::GlobalConfig;
use crate::input::{InputSource, SourceKind};
use crate::profile::ConfigDoc;

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A validation finding. `Error` = the config is malformed; `Warning` = advisory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
}

impl ConfigDoc {
    /// Validate the profile, collecting all diagnostics. No `Error`s ⇒ well-formed.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();

        if self.action_sets.is_empty() {
            error(&mut out, "profile has no action sets");
        }

        let set_names: HashSet<&str> = self.action_sets.iter().map(|a| a.name.as_str()).collect();
        dupes(self.action_sets.iter().map(|a| a.name.as_str()), "action set", &mut out);

        for a in &self.action_sets {
            let layer_names: HashSet<&str> = a.layers.iter().map(|l| l.name.as_str()).collect();
            dupes(
                a.layers.iter().map(|l| l.name.as_str()),
                &format!("action set {:?}: layer", a.name),
                &mut out,
            );

            check_bindings(&a.bindings, &format!("set {:?}", a.name), &set_names, &layer_names, &mut out);
            for l in &a.layers {
                let ctx = format!("set {:?} / layer {:?}", a.name, l.name);
                check_bindings(&l.bindings, &ctx, &set_names, &layer_names, &mut out);
            }
        }
        out
    }
}

impl GlobalConfig {
    /// Validate the globals: chords must have physical-button members, master rumble ≤ 100.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if self.master_rumble > 100 {
            warning(&mut out, format!("master_rumble is {} (> 100%)", self.master_rumble));
        }
        for (i, chord) in self.chords.iter().enumerate() {
            if chord.buttons.is_empty() {
                error(&mut out, format!("chord #{i} has no buttons"));
            }
            for b in &chord.buttons {
                if b.kind() != SourceKind::Button {
                    error(&mut out, format!("chord #{i}: {b:?} is not a physical button"));
                }
            }
        }
        out
    }
}

fn check_bindings(
    bindings: &BTreeMap<InputSource, SourceBinding>,
    ctx: &str,
    set_names: &HashSet<&str>,
    layer_names: &HashSet<&str>,
    out: &mut Vec<Diagnostic>,
) {
    for (input, binding) in bindings {
        if !binding.is_valid_for(&input.kind()) {
            error(out, format!("{ctx}: binding not valid for {input:?} (kind {:?})", input.kind()));
        }
        // Activation gaters must be physical buttons.
        if let Some(act) = binding.activation() {
            for g in &act.gaters {
                if g.kind() != SourceKind::Button {
                    error(out, format!("{ctx}: {input:?} gater {g:?} is not a physical button"));
                }
            }
        }
        for cmd in binding.commands() {
            if cmd.actions.is_empty() {
                warning(out, format!("{ctx}: {input:?} has a command with no actions"));
            }
            for action in &cmd.actions {
                check_action(action, set_names, layer_names, ctx, out);
            }
        }
    }
}

fn check_action(
    action: &Action,
    set_names: &HashSet<&str>,
    layer_names: &HashSet<&str>,
    ctx: &str,
    out: &mut Vec<Diagnostic>,
) {
    match action {
        Action::ChangeActionSet(r) if !set_names.contains(r.0.as_str()) => {
            error(out, format!("{ctx}: ChangeActionSet references unknown action set {:?}", r.0));
        }
        Action::HoldLayer(r) | Action::AddLayer(r) | Action::RemoveLayer(r)
            if !layer_names.contains(r.0.as_str()) =>
        {
            error(out, format!("{ctx}: layer action references unknown layer {:?}", r.0));
        }
        _ => {}
    }
}

/// Report duplicate names in an iterator.
fn dupes<'a>(names: impl Iterator<Item = &'a str>, what: &str, out: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    for n in names {
        if !seen.insert(n) {
            error(out, format!("duplicate {what} name {n:?}"));
        }
    }
}

fn error(out: &mut Vec<Diagnostic>, m: impl Into<String>) {
    out.push(Diagnostic { severity: Severity::Error, message: m.into() });
}
fn warning(out: &mut Vec<Diagnostic>, m: impl Into<String>) {
    out.push(Diagnostic { severity: Severity::Warning, message: m.into() });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, LayerRef};
    use crate::command::{Activator, Command};
    use crate::profile::{ActionSet, Layer};
    use crate::settings::{Activation, ActivationMode, GyroToMouseSettings};

    fn cmd(action: Action) -> Command {
        Command { activator: Activator::Regular, actions: vec![action], settings: Default::default() }
    }

    fn errors(ds: &[Diagnostic]) -> usize {
        ds.iter().filter(|d| d.severity == Severity::Error).count()
    }

    #[test]
    fn clean_profile_has_no_errors() {
        let mut bindings = BTreeMap::new();
        bindings.insert(
            InputSource::LeftBumper,
            SourceBinding::Button { commands: vec![cmd(Action::HoldLayer(LayerRef("aim".into())))] },
        );
        let doc = ConfigDoc {
            version: 0,
            name: "p".into(),
            action_sets: vec![ActionSet {
                name: "Game".into(),
                bindings,
                layers: vec![Layer { name: "aim".into(), bindings: BTreeMap::new() }],
            }],
            rumble: Default::default(),
        };
        assert_eq!(errors(&doc.validate()), 0);
    }

    #[test]
    fn catches_dangling_ref_kind_mismatch_and_bad_gater() {
        let mut bindings = BTreeMap::new();
        // dangling layer ref
        bindings.insert(
            InputSource::LeftBumper,
            SourceBinding::Button { commands: vec![cmd(Action::HoldLayer(LayerRef("nope".into())))] },
        );
        // kind mismatch: AsMouse on a Button
        bindings.insert(InputSource::RightBumper, SourceBinding::AsMouse { settings: Default::default() });
        // bad gater: a Pad used as a gyro gater
        bindings.insert(
            InputSource::Gyro,
            SourceBinding::GyroToMouse {
                settings: GyroToMouseSettings {
                    activation: Activation {
                        mode: ActivationMode::HoldToEnable,
                        gaters: vec![InputSource::LeftPad],
                    },
                    ..Default::default()
                },
            },
        );
        let doc = ConfigDoc {
            version: 0,
            name: "p".into(),
            action_sets: vec![ActionSet { name: "Game".into(), bindings, layers: vec![] }],
            rumble: Default::default(),
        };
        // ≥ 3 errors: dangling layer, kind mismatch, bad gater.
        assert!(errors(&doc.validate()) >= 3);
    }
}
