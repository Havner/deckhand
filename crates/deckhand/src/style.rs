//! All custom colors and styles for the UI, in one place.
//!
//! **Rule: every color we draw comes from the active theme's palette** — never a hardcoded
//! `Color::from_rgb`. Even when we bend a color out of its usual meaning (using `warning` as the
//! "Y button" yellow, `primary` as the "X button" blue) or modify it (darkening a panel), the
//! source is a palette role, so the whole window recolors coherently when the theme changes. iced
//! ships styled variants for some widgets (`button::success`, `container::secondary`, …); the
//! helpers here fill the gaps where only a raw palette color is exposed (text color, glyph color).
//!
//! The text helpers are `fn(&Theme) -> text::Style` so they can be handed straight to
//! `text(...).style(...)` and re-resolve against whatever theme iced passes — no need to thread the
//! theme through the view functions.

use iced::widget::{container, text};
use iced::{Background, Color, Theme};

/// Text in the palette's **success** color (green-ish) — e.g. the "connected" dot.
pub fn success_text(theme: &Theme) -> text::Style {
    text::Style { color: Some(theme.palette().success.base.color) }
}

/// Text in the palette's **danger** color (red-ish) — e.g. the "disconnected" dot, error messages.
pub fn danger_text(theme: &Theme) -> text::Style {
    text::Style { color: Some(theme.palette().danger.base.color) }
}

/// Text in the palette's **primary** color — the "X button" glyph (nominally blue).
pub fn primary_text(theme: &Theme) -> text::Style {
    text::Style { color: Some(theme.palette().primary.base.color) }
}

/// Text in the palette's **warning** color — the "Y button" glyph (nominally yellow).
pub fn warning_text(theme: &Theme) -> text::Style {
    text::Style { color: Some(theme.palette().warning.base.color) }
}

/// Muted/"inactive" text — a strong background tone rather than the foreground text color, so it
/// reads as subtle against surrounding text (e.g. the status-bar separators). Tracks the theme.
pub fn muted_text(theme: &Theme) -> text::Style {
    text::Style { color: Some(theme.palette().background.strong.color) }
}

/// A dimmed, theme-tinted backdrop for the modal overlay: the theme's background darkened toward
/// black and made translucent, so the content behind reads as greyed out.
pub fn scrim(theme: &Theme) -> container::Style {
    let base = theme.palette().background.base.color;
    let c = Color { r: base.r * 0.3, g: base.g * 0.3, b: base.b * 0.3, a: 0.7 };
    container::Style {
        background: Some(Background::Color(c)),
        ..container::transparent(theme)
    }
}

/// On a **dark** theme, each RGB channel of the `secondary` background is divided by this to darken
/// a panel while keeping the theme's tint. Light themes keep the plain `secondary` preset.
const DARK_PANEL_DIVISOR: f32 = 5.0;

/// Shared panel background (sidebar + the Buttons cards): the theme's `secondary` container style,
/// but darkened on dark themes (the palette's `is_dark` flag — the same signal that drives the
/// window decorations). Keeps the tint so each theme's panels still read as *its* color, just
/// darker.
pub fn panel(theme: &Theme) -> container::Style {
    let mut style = container::secondary(theme);
    // Light themes: keep the preset (its secondary text is already readable).
    if !theme.palette().is_dark {
        return style;
    }
    // Dark themes: darken the secondary background, and force the theme's primary (light) text
    // color — the `secondary` preset picks dark text for its light base, which is unreadable once
    // the background is darkened.
    if let Some(Background::Color(c)) = style.background {
        style.background = Some(Background::Color(Color::from_rgb(
            c.r / DARK_PANEL_DIVISOR,
            c.g / DARK_PANEL_DIVISOR,
            c.b / DARK_PANEL_DIVISOR,
        )));
    }
    style.text_color = Some(theme.palette().background.base.text);
    style
}
