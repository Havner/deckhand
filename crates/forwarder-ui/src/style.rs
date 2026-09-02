//! The handful of custom styles the forwarder draws (a trimmed copy of the main UI's `style.rs`).
//!
//! Rule, unchanged: every color comes from the active theme's palette, so the window recolors
//! coherently. The text helpers are `fn(&Theme) -> text::Style` so they hand straight to
//! `text(...).style(...)`.

use iced::Theme;
use iced::widget::overlay::menu;
use iced::widget::text;

/// Text in the palette's **success** color (green-ish) - the "connected"/controller-present dot.
pub(crate) fn success_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.palette().success.base.color),
    }
}

/// Text in the palette's **danger** color (red-ish) - the "disconnected" dot, error messages.
pub(crate) fn danger_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.palette().danger.base.color),
    }
}

/// Muted/"inactive" text - a strong background tone, so it reads as subtle against surrounding text
/// (the status-bar separators).
pub(crate) fn muted_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.palette().background.strong.color),
    }
}

/// The dropdown menu frame for the input combobox - recolor to `primary.strong` so the open field
/// and its list read as one highlighted unit.
pub(crate) fn combo_menu(theme: &Theme) -> menu::Style {
    let mut style = menu::default(theme);
    style.border.color = theme.palette().primary.strong.color;
    style
}
