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

use iced::widget::{button, container, pick_list, slider, text};
use iced::{Background, Border, Color, Theme};

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

/// A muted, **inactive-looking** slider — used for the LED-brightness bar when its checkbox is off,
/// so the control keeps the exact same geometry (rail + handle) as the live slider but reads as
/// disabled. Recolors iced's default slider from `primary` to a flat `background.strong` tone; the
/// incoming status is ignored (there's no meaningful hover/drag on an inert control).
pub fn disabled_slider(theme: &Theme, _status: slider::Status) -> slider::Style {
    let muted = theme.palette().background.strong.color;
    let mut style = slider::default(theme, slider::Status::Active);
    style.rail.backgrounds = (muted.into(), muted.into());
    style.handle.background = muted.into();
    style
}

/// A button styled to match the default combobox (`pick_list`) sitting on our cards: the palette's
/// `background.weak` fill + text and a `background.strong` hairline border — instead of the lighter
/// `secondary.base` tone iced's `button::secondary` uses, which reads too pale on the darkened cards.
/// Unifies the gear / "Add command" buttons with the behaviour combobox next to them. Hover
/// highlights the border like an opened combobox; pressed dips the fill a shade.
pub fn combo_button(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.palette();
    let base = button::Style {
        background: Some(Background::Color(palette.background.weak.color)),
        text_color: palette.background.weak.text,
        border: Border { radius: 2.0.into(), width: 1.0, color: palette.background.strong.color },
        ..button::Style::default()
    };
    match status {
        button::Status::Active => base,
        button::Status::Hovered => {
            button::Style { border: Border { color: palette.primary.strong.color, ..base.border }, ..base }
        }
        button::Status::Pressed => {
            button::Style { background: Some(Background::Color(palette.background.weaker.color)), ..base }
        }
        // Reads as inactive — faded fill + text (e.g. the selector arrows at the list ends).
        button::Status::Disabled => button::Style {
            background: Some(Background::Color(palette.background.weaker.color)),
            text_color: palette.background.strong.color,
            ..base
        },
    }
}

/// On a **dark** theme, each RGB channel of the `secondary` background is divided by this to darken
/// a panel while keeping the theme's tint. Light themes keep the plain `secondary` preset.
const DARK_PANEL_DIVISOR: f32 = 5.0;

/// The dark-theme "bar" background: the `secondary` container color with each channel divided by
/// [`DARK_PANEL_DIVISOR`]. One source of truth for the tone the sidebar/cards use, so the picker
/// option buttons ([`option_button`]) can sit on the very same color.
fn dark_bar_color(theme: &Theme) -> Color {
    let c = match container::secondary(theme).background {
        Some(Background::Color(c)) => c,
        _ => theme.palette().background.weak.color,
    };
    Color::from_rgb(c.r / DARK_PANEL_DIVISOR, c.g / DARK_PANEL_DIVISOR, c.b / DARK_PANEL_DIVISOR)
}

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
    style.background = Some(Background::Color(dark_bar_color(theme)));
    style.text_color = Some(theme.palette().background.base.text);
    style
}

/// A `pick_list` whose placeholder reads as **normal** text, not the muted default. The Action
/// picker uses a combobox's placeholder as its permanent label ("Hold Layer", …) rather than a
/// hint for an empty value, so the default `secondary` placeholder tone made those labels look
/// disabled. Everything else matches `pick_list::default`.
pub fn labeled_pick(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let mut style = pick_list::default(theme, status);
    style.placeholder_color = style.text_color;
    style
}

/// A modal card's surface — like `container::rounded_box`, but filled with the window's **base**
/// background (`background.base`) instead of the lighter `background.weak`, so a modal reads as the
/// same tone as the big content pane behind it (which has no fill of its own → the base background).
/// `rounded_box`'s `weak` fill looked right on light themes but too light on dark ones.
pub fn modal_card(theme: &Theme) -> container::Style {
    let palette = theme.palette();
    container::Style {
        background: Some(Background::Color(palette.background.base.color)),
        text_color: Some(palette.background.base.text),
        ..container::rounded_box(theme)
    }
}

/// The picker / menu **option** button — one style shared by the Action picker, the Button picker,
/// and the gear context menu, so every clickable option reads the same.
///
/// **Light themes:** essentially `button::secondary`, but hovering only *outlines* it with the
/// combobox's hairline border (no fill change); the fill shifts to `secondary`'s hover tone on
/// press instead. **Dark themes:** the [`combo_button`] look (gear / `<unbound>`) but filled with
/// the darkened [`dark_bar_color`] the bars use, so options sit on the same tone as the cards
/// behind them (hover highlights the border like an opened combobox; press dips the fill).
pub fn option_button(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.palette();
    if palette.is_dark {
        let bar = dark_bar_color(theme);
        let base = button::Style {
            background: Some(Background::Color(bar)),
            text_color: palette.background.base.text,
            border: Border { radius: 2.0.into(), width: 1.0, color: palette.background.strong.color },
            ..button::Style::default()
        };
        match status {
            button::Status::Active => base,
            button::Status::Hovered => button::Style {
                border: Border { color: palette.primary.strong.color, ..base.border },
                ..base
            },
            button::Status::Pressed => button::Style {
                background: Some(Background::Color(Color::from_rgb(
                    bar.r * 0.7,
                    bar.g * 0.7,
                    bar.b * 0.7,
                ))),
                ..base
            },
            button::Status::Disabled => {
                button::Style { text_color: palette.background.strong.color, ..base }
            }
        }
    } else {
        // Light: base is plain `secondary`; hover only adds a border (fill unchanged), and press
        // adopts `secondary`'s hover fill. The border matches `combo_button`'s hover highlight
        // (`primary.strong`) so hovering an option reads like hovering an opened combobox.
        let base = button::secondary(theme, button::Status::Active);
        let combo_border =
            Border { radius: 2.0.into(), width: 1.0, color: palette.primary.strong.color };
        match status {
            button::Status::Active => base,
            button::Status::Hovered => button::Style { border: combo_border, ..base },
            button::Status::Pressed => button::Style {
                border: combo_border,
                ..button::secondary(theme, button::Status::Hovered)
            },
            button::Status::Disabled => button::secondary(theme, button::Status::Disabled),
        }
    }
}
