//! The left-sidebar navigation model — one enum for every screen the content pane can show.
//!
//! Two sections, split by a flex spacer (declared top-to-bottom):
//! - **Top** — the **profile-editor** bands ([`Category::EDITOR_BANDS`]: Profile / per-input pages),
//!   which edit the loaded profile, so they're disabled until one is loaded (see
//!   `App::editing`).
//! - **Bottom** ([`Category::BOTTOM`]) — profile *management* (Profiles) plus the app-level pages
//!   (Device, Settings); always available.
//!
//! Profiles, Settings, and Device are wired; the editor pages are still mock screens.

use config::InputSource;

/// A sidebar entry / content screen. Declared in sidebar order, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Category {
    // Profile-editor categories (top section; mock screens for now).
    Profile,
    Buttons,
    Triggers,
    Joysticks,
    Trackpads,
    Gyro,
    // Bottom section: profile management + app-level pages.
    Profiles,
    Chords,
    Device,
    Settings,
}

impl Category {
    /// The profile-editor categories, split into the sidebar's two bands (rendered with a rule
    /// between them): the profile-level **Profile** page and the per-input pages. They operate on the
    /// loaded profile, so they're greyed until one is loaded ([`Self::is_editor`]).
    pub(crate) const EDITOR_BANDS: &'static [&'static [Category]] = &[
        &[Category::Profile],
        &[
            Category::Buttons,
            Category::Triggers,
            Category::Joysticks,
            Category::Trackpads,
            Category::Gyro,
        ],
    ];

    /// The bottom-section pages, in order: profile management (Profiles) then the app-level pages
    /// (Device, Settings). Rendered flat (no separators), pinned below the flex spacer.
    pub(crate) const BOTTOM: &'static [Category] =
        &[Category::Profiles, Category::Chords, Category::Device, Category::Settings];

    /// Whether this category is part of the profile editor (disabled when no profile is loaded).
    pub(crate) fn is_editor(self) -> bool {
        Self::EDITOR_BANDS.iter().any(|band| band.contains(&self))
    }

    /// The sidebar label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Category::Profile => "Profile",
            Category::Buttons => "Buttons",
            Category::Triggers => "Triggers",
            Category::Joysticks => "Joysticks",
            Category::Trackpads => "Trackpads",
            Category::Gyro => "Gyro",
            Category::Profiles => "Profiles",
            Category::Chords => "Chords",
            Category::Device => "Device",
            Category::Settings => "Settings",
        }
    }

    /// The input groups shown on this category's editor page, in display order. Empty for the
    /// non-input categories (Profile / Device / Settings / Profiles) — those render bespoke screens
    /// (action sets + name + rumble feel, app settings). See [`InputGroup`] for the header + primary
    /// + sub-button layout.
    ///
    /// This is the static, device-independent superset (Gordon lacks a handful — greyed at render
    /// time via [`config::Shape`], not filtered here). Every [`InputSource`] appears in exactly one
    /// group across all categories (checked by tests).
    pub(crate) fn groups(self) -> &'static [InputGroup] {
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
pub(crate) struct InputGroup {
    /// Group header (a cluster name like "Bumpers", or a rich source like "Left Stick").
    pub(crate) header: &'static str,
    /// The primary inputs: the cluster's buttons, or the single rich source.
    pub(crate) primary: &'static [InputSource],
    /// Sub-buttons attached to a rich source (its click/touch), shown under `primary` without their
    /// own header. Empty for button clusters.
    pub(crate) sub: &'static [InputSource],
}

use InputSource as I;

/// Buttons page — the standalone buttons + the two 4-button clusters, in labelled groups.
const BUTTONS_GROUPS: &[InputGroup] = &[
    InputGroup { header: "Face Buttons", primary: &[I::FaceButtons], sub: &[] },
    InputGroup { header: "D-Pad", primary: &[I::DPad], sub: &[] },
    InputGroup { header: "Bumpers", primary: &[I::LeftBumper, I::RightBumper], sub: &[] },
    InputGroup {
        header: "Grips",
        // Grip-touch (capacitive handle sensors) is Triton-only — the shape filter drops it on
        // Gordon/Neptune, so it appears here only when a Triton is shown.
        primary: &[
            I::LeftGrip,
            I::RightGrip,
            I::LeftGrip2,
            I::RightGrip2,
            I::LeftGripTouch,
            I::RightGripTouch,
        ],
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
    InputGroup { header: "Left Stick", primary: &[I::LeftStick], sub: &[I::LeftStickClick, I::LeftStickTouch] },
    InputGroup { header: "Right Stick", primary: &[I::RightStick], sub: &[I::RightStickClick, I::RightStickTouch] },
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
        for band in Category::EDITOR_BANDS {
            for &cat in *band {
                for group in cat.groups() {
                    seen.extend(group.primary.iter().chain(group.sub).cloned());
                }
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
        for cat in [Category::Profile, Category::Chords, Category::Device, Category::Settings, Category::Profiles] {
            assert!(cat.groups().is_empty(), "{:?} should have no input groups", cat.label());
        }
    }
}
