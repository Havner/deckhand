//! The left-sidebar navigation model — one enum for every screen the content pane can show.
//!
//! Three bands, top to bottom:
//! - **Profiles** — profile *management* (load-for-edit, send-to-daemon); always available.
//! - a separator, then the **profile-editor** categories (Profile … Gyro) — these edit the
//!   currently-loaded profile, so they're disabled until one is loaded (see `App::editing`).
//! - the **application-level** entries pinned at the bottom (Globals, Settings) — always available.
//!
//! The editor categories are still stub/mockup screens; Profiles, Settings, and Globals are wired.

use config::InputSource;

/// A sidebar entry / content screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    // Profile management (top band).
    Profiles,
    // Profile-editor categories (middle band, below the separator; stub screens for now).
    Profile,
    Buttons,
    Triggers,
    Joysticks,
    Trackpads,
    Gyro,
    Rumble,
    // Application-level (bottom band).
    Globals,
    Settings,
}

impl Category {
    /// The profile-editor categories, shown below the sidebar separator. These operate on the
    /// loaded profile, so they're greyed out until a profile is loaded for editing.
    pub const EDITOR: &'static [Category] = &[
        Category::Profile,
        Category::Buttons,
        Category::Triggers,
        Category::Joysticks,
        Category::Trackpads,
        Category::Gyro,
        Category::Rumble,
    ];

    /// The application-level categories pinned at the bottom of the sidebar (Settings at the very
    /// bottom, Globals above it).
    pub const APP: &'static [Category] = &[Category::Globals, Category::Settings];

    /// Whether this category is part of the profile editor (disabled when no profile is loaded).
    pub fn is_editor(self) -> bool {
        Self::EDITOR.contains(&self)
    }

    /// The sidebar label.
    pub fn label(self) -> &'static str {
        match self {
            Category::Profiles => "Profiles",
            Category::Profile => "Profile",
            Category::Buttons => "Buttons",
            Category::Triggers => "Triggers",
            Category::Joysticks => "Joysticks",
            Category::Trackpads => "Trackpads",
            Category::Gyro => "Gyro",
            Category::Rumble => "Rumble",
            Category::Globals => "Globals",
            Category::Settings => "Settings",
        }
    }

    /// The input groups shown on this category's editor page, in display order. Empty for the
    /// non-input categories (Profile / Rumble / Globals / Settings / Profiles) — those render
    /// bespoke screens (action sets + name, rumble feel, app settings). See [`InputGroup`] for the
    /// header + primary + sub-button layout.
    ///
    /// This is the static, device-independent superset (Gordon lacks a handful — greyed at render
    /// time via [`config::Shape`], not filtered here). Every [`InputSource`] appears in exactly one
    /// group across all categories (checked by tests).
    pub fn groups(self) -> &'static [InputGroup] {
        match self {
            Category::Buttons => BUTTONS_GROUPS,
            Category::Triggers => TRIGGER_GROUPS,
            Category::Joysticks => STICK_GROUPS,
            Category::Trackpads => PAD_GROUPS,
            Category::Gyro => GYRO_GROUPS,
            _ => &[],
        }
    }
}

/// One labelled group on an editor page. Two shapes share it:
///
/// - a **button cluster** — a header over several standalone buttons (`Bumpers`: Left/Right Bumper);
///   `primary` holds the buttons, `sub` is empty.
/// - a **rich source** — the header *is* the source (`Left Stick`); `primary` is the single rich
///   input (its behaviour selector), and `sub` lists the buttons that live on it (its click/touch),
///   shown below with a small gap and no header of their own.
pub struct InputGroup {
    /// Group header (a cluster name like "Bumpers", or a rich source like "Left Stick").
    pub header: &'static str,
    /// The primary inputs: the cluster's buttons, or the single rich source.
    pub primary: &'static [InputSource],
    /// Sub-buttons attached to a rich source (its click/touch), shown under `primary` without their
    /// own header. Empty for button clusters.
    pub sub: &'static [InputSource],
}

use InputSource as I;

/// Buttons page — the standalone buttons + the two 4-button clusters, in labelled groups.
const BUTTONS_GROUPS: &[InputGroup] = &[
    InputGroup { header: "Face Buttons", primary: &[I::FaceButtons], sub: &[] },
    InputGroup { header: "D-Pad", primary: &[I::DPad], sub: &[] },
    InputGroup { header: "Bumpers", primary: &[I::LeftBumper, I::RightBumper], sub: &[] },
    InputGroup {
        header: "Grips",
        primary: &[I::LeftGrip, I::RightGrip, I::LeftGrip2, I::RightGrip2],
        sub: &[],
    },
    InputGroup { header: "Menu Buttons", primary: &[I::View, I::Menu, I::Steam, I::QuickAccess], sub: &[] },
];

/// Triggers page — each analog trigger with its full-pull hardware bit nested under it.
const TRIGGER_GROUPS: &[InputGroup] = &[
    InputGroup { header: "Left Trigger", primary: &[I::LeftTrigger], sub: &[I::LeftTriggerFull] },
    InputGroup { header: "Right Trigger", primary: &[I::RightTrigger], sub: &[I::RightTriggerFull] },
];

/// Joysticks page — each stick with its click nested under it.
const STICK_GROUPS: &[InputGroup] = &[
    InputGroup { header: "Left Stick", primary: &[I::LeftStick], sub: &[I::LeftStickClick] },
    InputGroup { header: "Right Stick", primary: &[I::RightStick], sub: &[I::RightStickClick] },
];

/// Trackpads page — each pad with its click + touch nested under it.
const PAD_GROUPS: &[InputGroup] = &[
    InputGroup { header: "Left Trackpad", primary: &[I::LeftPad], sub: &[I::LeftPadClick, I::LeftPadTouch] },
    InputGroup { header: "Right Trackpad", primary: &[I::RightPad], sub: &[I::RightPadClick, I::RightPadTouch] },
];

/// Gyro page — the single motion source.
const GYRO_GROUPS: &[InputGroup] = &[InputGroup { header: "Gyro", primary: &[I::Gyro], sub: &[] }];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every logical input appears exactly once across all category groups (primary or sub) — no
    /// input is dropped from the editor, and none is listed on two pages. Guards the mapping against
    /// drift when `InputSource` grows.
    #[test]
    fn every_input_is_mapped_exactly_once() {
        let mut seen: Vec<InputSource> = Vec::new();
        for &cat in Category::EDITOR {
            for group in cat.groups() {
                seen.extend(group.primary.iter().chain(group.sub).cloned());
            }
        }
        // No duplicates.
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "an input is mapped on more than one page");
        // Complete: exactly the full superset.
        assert_eq!(sorted.len(), InputSource::ALL.len(), "not every input is mapped");
        for input in InputSource::ALL {
            assert!(seen.contains(input), "{input:?} is not on any editor page");
        }
    }

    /// The non-input categories carry no groups (they render bespoke screens).
    #[test]
    fn non_input_categories_have_no_groups() {
        for cat in [Category::Profile, Category::Rumble, Category::Globals, Category::Settings, Category::Profiles] {
            assert!(cat.groups().is_empty(), "{:?} should have no input groups", cat.label());
        }
    }
}
