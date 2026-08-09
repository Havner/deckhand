//! The left-sidebar navigation model — one enum for every screen the content pane can show.
//!
//! Mirrors the Steam Deck configurator's layout (per-input-type categories up top), plus the two
//! **application-level** entries pinned at the bottom (Settings, Globals). The profile-edit
//! categories are still stubs; Settings and Globals are both fully wired.

/// A sidebar entry / content screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    // Profile-edit categories (top group; stub screens in the test).
    Buttons,
    Triggers,
    Joysticks,
    Trackpads,
    Gyro,
    // Application-level (bottom group).
    Settings,
    Globals,
}

impl Category {
    /// The profile-edit categories shown at the top of the sidebar.
    pub const PROFILE: &'static [Category] =
        &[Category::Buttons, Category::Triggers, Category::Joysticks, Category::Trackpads, Category::Gyro];

    /// The application-level categories pinned at the bottom of the sidebar (Settings sits at the
    /// very bottom, Globals above it).
    pub const APP: &'static [Category] = &[Category::Globals, Category::Settings];

    /// The sidebar label.
    pub fn label(self) -> &'static str {
        match self {
            Category::Buttons => "Buttons",
            Category::Triggers => "Triggers",
            Category::Joysticks => "Joysticks",
            Category::Trackpads => "Trackpads",
            Category::Gyro => "Gyro",
            Category::Settings => "Settings",
            Category::Globals => "Globals",
        }
    }
}
