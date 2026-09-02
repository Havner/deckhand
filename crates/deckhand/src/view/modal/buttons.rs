//! The button (gater/chord) picker modal - a **controller-shaped** chooser that returns one raw
//! controller [`Button`](vocab_hid::Button). Its consumers are `Activation.gaters` and
//! `Chord.buttons` (both `Vec<vocab_hid::Button>`), so this is a single-select: click one to
//! append it. Every hardware bit is offered, laid out ~like the Action picker's Gamepad tab (each
//! tile roughly where the button sits on the pad). Tiles carry short labels here (the chips + command
//! bars use the full `button_label`), same split as the Gamepad tab.

use iced::widget::{Space, button, column, container, row, text};
use iced::{Center, Element, Theme};

use vocab_hid::Button;

use crate::{Message, style};

/// Tile geometry - a touch wider than the Gamepad tab's (50) so the word labels fit.
const W: f32 = 60.0;
const H: f32 = 40.0;

/// An active button tile, confirming `ButtonPicked` on press.
fn btn(label: &'static str, b: Button) -> Element<'static, Message> {
    button(text(label).size(12.0).center())
        .width(W)
        .height(H)
        .style(style::option_button)
        .on_press(Message::ButtonPicked(b))
        .into()
}

/// A tile with a custom (coloured) style - the A/B/X/Y face buttons keep their Xbox glyph colours.
fn btn_styled(label: &'static str, b: Button, sty: fn(&Theme, button::Status) -> button::Style) -> Element<'static, Message> {
    button(text(label).size(12.0).center())
        .width(W)
        .height(H)
        .style(sty)
        .on_press(Message::ButtonPicked(b))
        .into()
}

/// up / left+right / down - a dpad or face diamond (middle gap = one tile wide).
fn diamond(
    up: Element<'static, Message>,
    left: Element<'static, Message>,
    right: Element<'static, Message>,
    down: Element<'static, Message>,
) -> Element<'static, Message> {
    column![up, row![left, Space::new().width(W), right].spacing(6.0), down].spacing(6.0).align_x(Center).into()
}

/// A small labelled cluster (header over its content).
fn cluster(label: &'static str, content: impl Into<Element<'static, Message>>) -> Element<'static, Message> {
    column![text(label).size(12.0), content.into()].spacing(8.0).align_x(Center).into()
}

/// The whole button-picker card, controller-shaped and centred left<->right.
pub(super) fn card() -> Element<'static, Message> {
    use vocab_hid::Button::*;
    let gap = || Space::new().width(40.0);

    // Bumpers + full-trigger pulls, mirrored (outer trigger, inner bumper).
    let shoulders = row![btn("LT", LT), btn("LB", LB), Space::new().width(60.0), btn("RB", RB), btn("RT", RT)]
        .spacing(8.0)
        .align_y(Center);

    // Left half: the D-pad diamond over the left stick/pad clicks.
    let dpad = diamond(btn("↑", DpadUp), btn("←", DpadLeft), btn("→", DpadRight), btn("↓", DpadDown));
    let left = cluster(
        "D-Pad",
        column![dpad, row![btn("LS", LStickPress), btn("L Pad", LPadPress)].spacing(6.0)].spacing(10.0).align_x(Center),
    );

    // Right half: the face diamond over the right stick/pad clicks.
    let face = diamond(
        btn_styled("Y", Y, button::warning),
        btn_styled("X", X, button::primary),
        btn_styled("B", B, button::danger),
        btn_styled("A", A, button::success),
    );
    let right = cluster(
        "Face",
        column![face, row![btn("R Pad", RPadPress), btn("RS", RStickPress)].spacing(6.0)].spacing(10.0).align_x(Center),
    );

    // Centre: the four system buttons in their 2x2 cluster (View/Menu over Steam/Quick).
    let center = cluster(
        "System",
        column![
            row![btn("View", View), btn("Menu", Menu)].spacing(6.0),
            row![btn("Steam", Steam), btn("Quick", QuickAccess)].spacing(6.0),
        ]
        .spacing(6.0)
        .align_x(Center),
    );

    // Back grips, mirrored (outer grip, inner grip2).
    let grips = row![btn("LGrip", LGrip), btn("LGrip2", LGrip2), Space::new().width(60.0), btn("RGrip2", RGrip2), btn("RGrip", RGrip)]
        .spacing(8.0)
        .align_y(Center);

    // The capacitive touch bits - the odd ones out, kept in their own labelled row at the bottom.
    // Grip touch (Triton-only) sits with them; like every other tile here it's offered regardless of
    // shape (the picker builds device-independent gater/chord sets).
    let touch = cluster(
        "Touch",
        row![
            btn("L Stick", LStickTouch),
            btn("L Pad", LPadTouch),
            btn("L Grip", LGripTouch),
            btn("R Grip", RGripTouch),
            btn("R Pad", RPadTouch),
            btn("R Stick", RStickTouch),
        ]
        .spacing(6.0),
    );

    let body = column![
        text("Select a button").size(18.0),
        shoulders,
        row![left, gap(), center, gap(), right].spacing(8.0).align_y(Center),
        grips,
        touch,
    ]
    .spacing(16.0)
    .align_x(Center)
    .width(iced::Fill);
    container(body).padding(20.0).width(720.0).style(style::modal_card).into()
}
