//! The left-sidebar navigation model — one enum for every screen the content pane can show.
//!
//! Three bands, top to bottom:
//! - **Profiles** — profile *management* (load-for-edit, send-to-daemon); always available.
//! - a separator, then the **profile-editor** categories (Action Sets … Gyro) — these edit the
//!   currently-loaded profile, so they're disabled until one is loaded (see `App::editing`).
//! - the **application-level** entries pinned at the bottom (Globals, Settings) — always available.
//!
//! The editor categories are still stub/mockup screens; Profiles, Settings, and Globals are wired.

/// A sidebar entry / content screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    // Profile management (top band).
    Profiles,
    // Profile-editor categories (middle band, below the separator; stub screens for now).
    ActionSets,
    Buttons,
    Triggers,
    Joysticks,
    Trackpads,
    Gyro,
    // Application-level (bottom band).
    Globals,
    Settings,
}

impl Category {
    /// The profile-editor categories, shown below the sidebar separator. These operate on the
    /// loaded profile, so they're greyed out until a profile is loaded for editing.
    pub const EDITOR: &'static [Category] = &[
        Category::ActionSets,
        Category::Buttons,
        Category::Triggers,
        Category::Joysticks,
        Category::Trackpads,
        Category::Gyro,
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
            Category::ActionSets => "Action Sets",
            Category::Buttons => "Buttons",
            Category::Triggers => "Triggers",
            Category::Joysticks => "Joysticks",
            Category::Trackpads => "Trackpads",
            Category::Gyro => "Gyro",
            Category::Globals => "Globals",
            Category::Settings => "Settings",
        }
    }
}
