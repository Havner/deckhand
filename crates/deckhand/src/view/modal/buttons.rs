//! The button (gater/chord) picker modal — a chooser that returns one raw controller
//! [`Button`](vocab_hid::Button). Its consumers are `Activation.gaters` and `GlobalChord.buttons`
//! (both `Vec<vocab_hid::Button>`), so this is a single-select: click one button to append it to a
//! list managed elsewhere. Every hardware button bit is offered — including the face buttons
//! (A/B/X/Y), the dpad directions, and the pad/stick touches — grouped into static rows.

use iced::widget::{button, column, container, row, text};
use iced::{Center, Element};

use vocab_hid::Button;

use crate::view::button_label;
use crate::{Message, style};

/// One selectable button tile (fixed size), confirming `ButtonPicked` on press.
fn tile(b: Button) -> Element<'static, Message> {
    button(text(button_label(&b)).size(12.0).center())
        .width(150.0)
        .height(40.0)
        .style(style::option_button)
        .on_press(Message::ButtonPicked(b))
        .into()
}

/// A row of button tiles.
fn row_of(buttons: &[Button]) -> Element<'static, Message> {
    let mut r = row![].spacing(8.0);
    for b in buttons {
        r = r.push(tile(b.clone()));
    }
    r.into()
}

/// The whole button-picker card: every hardware button, grouped into static rows.
pub(super) fn card() -> Element<'static, Message> {
    use vocab_hid::Button::*;
    let col = column![
        text("Select a button").size(18.0),
        row_of(&[A, B, X, Y]),
        row_of(&[DpadUp, DpadDown, DpadLeft, DpadRight]),
        row_of(&[LB, RB, LT, RT]),
        row_of(&[LGrip, RGrip, LGrip2, RGrip2]),
        row_of(&[View, Menu, Steam, QuickAccess]),
        row_of(&[LStickPress, RStickPress, LPadPress, RPadPress]),
        row_of(&[LPadTouch, RPadTouch, LStickTouch, RStickTouch]),
    ]
    .spacing(10.0)
    .align_x(Center);
    container(col).padding(20.0).width(720.0).style(style::modal_card).into()
}
