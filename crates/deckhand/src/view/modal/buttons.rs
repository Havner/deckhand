//! The button (gater/global) picker modal — a controller-shaped chooser that returns one
//! `SourceKind::Button` [`InputSource`](config::InputSource). Its consumers are `Activation.gaters`
//! and `GlobalChord.buttons` (both `Vec<InputSource>`), so this is a single-select: click one button
//! to append it to a list managed elsewhere. Only the 18 standalone hardware buttons appear — face
//! buttons (A/B/X/Y) and D-Pad directions aren't standalone `InputSource`s, so they can't be used
//! here yet (a note on the card says so).

use iced::widget::{Space, button, column, container, row, text};
use iced::{Center, Element};

use config::InputSource;
use config::InputSource::*;

use crate::view::editor::input_label;
use crate::{Message, style};

/// One selectable hardware button.
fn tile(src: InputSource) -> Element<'static, Message> {
    let label = input_label(&src);
    button(text(label).size(12.0).center())
        .width(150.0)
        .height(40.0)
        .style(style::option_button)
        .on_press(Message::ButtonPicked(src))
        .into()
}

/// The whole button-picker card: the 18 standalone buttons in a rough controller arrangement.
pub(super) fn card() -> Element<'static, Message> {
    let gap = || Space::new().width(30.0);
    let shoulders =
        row![tile(LeftTriggerFull), tile(LeftBumper), gap(), tile(RightBumper), tile(RightTriggerFull)]
            .spacing(8.0);
    let grips =
        row![tile(LeftGrip), tile(LeftGrip2), gap(), tile(RightGrip2), tile(RightGrip)].spacing(8.0);
    let system = row![tile(View), tile(Steam), tile(Menu), tile(QuickAccess)].spacing(8.0);
    let clicks = row![
        tile(LeftStickClick),
        tile(LeftPadClick),
        gap(),
        tile(RightPadClick),
        tile(RightStickClick),
    ]
    .spacing(8.0);
    let touches = row![tile(LeftPadTouch), gap(), tile(RightPadTouch)].spacing(8.0);

    let note = text(
        "A/B/X/Y and D-Pad directions can't be used here yet — the current model only exposes \
         standalone hardware buttons.",
    )
    .size(12.0);

    let col = column![
        text("Select a button").size(18.0),
        shoulders,
        grips,
        system,
        clicks,
        touches,
        Space::new().height(4.0),
        note,
    ]
    .spacing(14.0)
    .align_x(Center);

    container(col).padding(20.0).width(720.0).style(style::modal_card).into()
}
