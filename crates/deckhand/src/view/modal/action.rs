//! The output-Action picker modal — a tabbed chooser that returns one [`config::Action`].
//!
//! Tabs mirror the categories a binding's action can target: Gamepad (`GamepadButton`), Mouse
//! (`MouseButton`), Keyboard / Numpad (`Key`, split like Steam), and Action Sets (the engine
//! mode actions, each parameterised by a set/layer picked from the loaded profile). Clicking a tile
//! or selecting a combobox value confirms immediately (Steam-style) — there is no OK button. Steam's
//! SYSTEM/CAMERA tabs are dropped (no vocab); `Action::None` is intentionally not offered (a future
//! gear "Unbind" falls the input back to `<unbound>` instead).

use std::collections::HashSet;

use iced::widget::{Space, button, column, container, pick_list, row, scrollable, text};
use iced::{Center, Element, Fill, Theme};

use config::{Action, ActionSetRef, LayerRef};
use vocab::{GamepadButton, Key, MouseButton};

use super::ActionTab;
use crate::editor::EditorMessage;
use crate::{App, Message, style};

/// The base key-cell width unit (px); wider keys scale it. (Named `KW`, not `U`, because `Key::U`
/// exists and a `use Key::*` would shadow a bare `U`.)
const KW: f32 = 42.0;

/// Fixed card footprint — sized to the largest tab (gamepad) so switching tabs doesn't resize the
/// modal. Deliberately a little roomier than any one tab needs; taller content (numpad extras)
/// scrolls within it.
const CARD_W: f32 = 840.0;
const CARD_H: f32 = 520.0;

/// The whole Action-picker card: a tab bar + the selected tab's content, at a fixed size with the
/// content centred horizontally.
pub(super) fn card(app: &App, tab: ActionTab) -> Element<'static, Message> {
    let content = match tab {
        ActionTab::Gamepad => gamepad(),
        ActionTab::Mouse => mouse(),
        ActionTab::Keyboard => keyboard(),
        ActionTab::Numpad => numpad(),
        ActionTab::ActionSets => action_sets(app),
    };
    // `width(Fill)` makes the body span the card so `align_x(Center)` centres the tab bar and the
    // content left↔right (they're otherwise shrink-width and would hug the left edge).
    let body = column![tab_bar(tab), content].spacing(20.0).align_x(Center).width(Fill);
    container(scrollable(body).width(Fill).height(Fill))
        .padding(20.0)
        .width(CARD_W)
        .height(CARD_H)
        .style(style::modal_card)
        .into()
}

/// The row of tab buttons; the active one is highlighted.
fn tab_bar(active: ActionTab) -> Element<'static, Message> {
    let tab = |label, t: ActionTab| -> Element<'static, Message> {
        let b = button(text(label).size(13.0)).padding(8.0);
        let b = if t == active { b.style(button::primary) } else { b.style(button::text) };
        b.on_press(Message::Editor(EditorMessage::ActionPickerTab(t))).into()
    };
    row![
        tab("Gamepad", ActionTab::Gamepad),
        tab("Mouse", ActionTab::Mouse),
        tab("Keyboard", ActionTab::Keyboard),
        tab("Numpad", ActionTab::Numpad),
        tab("Action Sets", ActionTab::ActionSets),
    ]
    .spacing(6.0)
    .into()
}

// --- gamepad ---------------------------------------------------------------------------------

/// An active gamepad-button tile.
fn gbtn(label: &'static str, gb: GamepadButton) -> Element<'static, Message> {
    button(text(label).size(13.0).center())
        .width(50.0)
        .height(40.0)
        .style(style::option_button)
        .on_press(Message::Editor(EditorMessage::ActionPicked(Action::GamepadButton(gb))))
        .into()
}

/// A disabled placeholder tile (no output in vocab yet — e.g. LT/RT, stick directions). No
/// `on_press` → renders in the option button's disabled (faded) state.
fn gbtn_off(label: &'static str) -> Element<'static, Message> {
    button(text(label).size(13.0).center()).width(50.0).height(40.0).style(style::option_button).into()
}

/// An active gamepad-button tile with a custom (coloured) style — the A/B/X/Y face buttons keep
/// their Xbox glyph colours rather than the uniform option style.
fn gbtn_styled(
    label: &'static str,
    gb: GamepadButton,
    sty: fn(&Theme, button::Status) -> button::Style,
) -> Element<'static, Message> {
    button(text(label).size(13.0).center())
        .width(50.0)
        .height(40.0)
        .style(sty)
        .on_press(Message::Editor(EditorMessage::ActionPicked(Action::GamepadButton(gb))))
        .into()
}

/// up / left+center+right / down, stacked — a stick cluster.
fn cross(
    up: Element<'static, Message>,
    left: Element<'static, Message>,
    center: Element<'static, Message>,
    right: Element<'static, Message>,
    down: Element<'static, Message>,
) -> Element<'static, Message> {
    column![up, row![left, center, right].spacing(6.0).align_y(Center), down]
        .spacing(6.0)
        .align_x(Center)
        .into()
}

/// up / left+right / down — a dpad or face diamond.
fn diamond(
    up: Element<'static, Message>,
    left: Element<'static, Message>,
    right: Element<'static, Message>,
    down: Element<'static, Message>,
) -> Element<'static, Message> {
    column![up, row![left, Space::new().width(50.0), right].spacing(6.0), down]
        .spacing(6.0)
        .align_x(Center)
        .into()
}

fn gamepad() -> Element<'static, Message> {
    use GamepadButton::*;
    // Bumpers active; triggers disabled (a gamepad trigger output is an axis, not in vocab yet).
    let shoulders = row![
        gbtn_off("LT"),
        gbtn("LB", LeftBumper),
        Space::new().width(60.0),
        gbtn("RB", RightBumper),
        gbtn_off("RT"),
    ]
    .spacing(8.0)
    .align_y(Center);

    // Sticks: centre click active; the 4 direction arrows disabled (no stick-direction output).
    let lstick = cross(gbtn_off("↑"), gbtn_off("←"), gbtn("L", LeftStick), gbtn_off("→"), gbtn_off("↓"));
    let rstick = cross(gbtn_off("↑"), gbtn_off("←"), gbtn("R", RightStick), gbtn_off("→"), gbtn_off("↓"));
    let dpad = diamond(gbtn("↑", DpadUp), gbtn("←", DpadLeft), gbtn("→", DpadRight), gbtn("↓", DpadDown));
    let face = diamond(
        gbtn_styled("Y", Y, button::warning),
        gbtn_styled("X", X, button::primary),
        gbtn_styled("B", B, button::danger),
        gbtn_styled("A", A, button::success),
    );
    let system = row![gbtn("Back", Back), gbtn("Guide", Guide), gbtn("Start", Start)].spacing(8.0);

    let left =
        column![text("Left Stick").size(12.0), lstick, text("D-Pad").size(12.0), dpad].spacing(10.0).align_x(Center);
    let right =
        column![text("Face").size(12.0), face, text("Right Stick").size(12.0), rstick].spacing(10.0).align_x(Center);

    column![
        shoulders,
        row![left, Space::new().width(40.0), system, Space::new().width(40.0), right].spacing(8.0),
    ]
    .spacing(20.0)
    .align_x(Center)
    .into()
}

// --- mouse -----------------------------------------------------------------------------------

fn mouse() -> Element<'static, Message> {
    use MouseButton::*;
    let mb = |label, b: MouseButton| -> Element<'static, Message> {
        button(text(label))
            .width(190.0)
            .style(style::option_button)
            .on_press(Message::Editor(EditorMessage::ActionPicked(Action::MouseButton(b))))
            .into()
    };
    let clicks = column![
        mb("Left Click", Left),
        mb("Middle Click", Middle),
        mb("Right Click", Right),
        mb("Back (Mouse 4)", Back),
        mb("Forward (Mouse 5)", Forward),
    ]
    .spacing(8.0);
    let scroll = column![
        mb("Scroll Up", ScrollUp),
        mb("Scroll Down", ScrollDown),
        mb("Scroll Left", ScrollLeft),
        mb("Scroll Right", ScrollRight),
    ]
    .spacing(8.0);
    row![clicks, scroll].spacing(24.0).into()
}

// --- keyboard / numpad -----------------------------------------------------------------------

/// A single key tile (fixed height, variable width), confirming `Action::Key` on press.
fn key(k: Key, w: f32) -> Element<'static, Message> {
    let label = key_label(&k);
    button(text(label).size(11.0).center())
        .width(w)
        .height(30.0)
        .padding(2.0)
        .style(style::option_button)
        .on_press(Message::Editor(EditorMessage::ActionPicked(Action::Key(k))))
        .into()
}

/// A row of default-width key tiles.
fn krow(keys: &[Key]) -> Element<'static, Message> {
    let mut r = row![].spacing(4.0);
    for k in keys {
        r = r.push(key(k.clone(), key_width(k)));
    }
    r.into()
}

/// Extra horizontal units for the wider keys, so a row roughly lines up. Keys are spelled with the
/// `Key::` prefix (no glob) so `Key::Space`/`Key::U` don't shadow the iced `Space` widget / `KW`.
fn key_width(k: &Key) -> f32 {
    use Key::*;
    match k {
        Backspace | Enter => 1.9 * KW,
        Tab | CapsLock | LeftShift | RightShift => 1.7 * KW,
        Space => 6.0 * KW,
        LeftCtrl | RightCtrl | LeftAlt | RightAlt | LeftMeta | RightMeta | Menu => 1.3 * KW,
        _ => KW,
    }
}

/// Main keyboard rows (function → number → QWERTY → home → shift → bottom).
const KB_ROWS: &[&[Key]] = {
    use Key::*;
    &[
        &[Esc, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12],
        &[Grave, D1, D2, D3, D4, D5, D6, D7, D8, D9, D0, Minus, Equal, Backspace],
        &[Tab, Q, W, E, R, T, Y, U, I, O, P, LeftBrace, RightBrace, Backslash],
        &[CapsLock, A, S, D, F, G, H, J, K, L, Semicolon, Apostrophe, Enter],
        &[LeftShift, Z, X, C, V, B, N, M, Comma, Dot, Slash, RightShift],
        &[LeftCtrl, LeftMeta, LeftAlt, Space, RightAlt, RightMeta, Menu, RightCtrl],
    ]
};

const NP_NAV: &[Key] =
    { use Key::*; &[Insert, Home, PageUp, Delete, End, PageDown] };
const NP_ARROWS: &[Key] = { use Key::*; &[Up, Left, Down, Right] };
const NP_MEDIA: &[Key] =
    { use Key::*; &[PlayPause, StopCd, PreviousSong, NextSong, Mute, VolumeDown, VolumeUp] };
const NP_KEYPAD: &[Key] = {
    use Key::*;
    &[
        NumLock, KpSlash, KpAsterisk, KpMinus, Kp7, Kp8, Kp9, KpPlus, Kp4, Kp5, Kp6, Kp1, Kp2, Kp3,
        KpEnter, Kp0, KpDot,
    ]
};

fn keyboard() -> Element<'static, Message> {
    let mut col = column![].spacing(4.0);
    for r in KB_ROWS {
        col = col.push(krow(r));
    }
    col.align_x(Center).into()
}

/// A row of default-width tiles for the numpad/nav/media clusters.
fn row_of(keys: &[Key]) -> Element<'static, Message> {
    let mut r = row![].spacing(4.0);
    for k in keys {
        r = r.push(key(k.clone(), KW));
    }
    r.into()
}

fn numpad() -> Element<'static, Message> {
    use Key::*;
    let nav = column![
        row_of(&[Insert, Home, PageUp]),
        row_of(&[Delete, End, PageDown]),
        iced::widget::Space::new().height(8.0),
        row_of(&[Up]),
        row_of(&[Left, Down, Right]),
    ]
    .spacing(4.0)
    .align_x(Center);
    let media = column![
        row_of(&[PlayPause, StopCd]),
        row_of(&[PreviousSong, NextSong]),
        iced::widget::Space::new().height(8.0),
        row_of(&[Mute]),
        row_of(&[VolumeDown, VolumeUp]),
    ]
    .spacing(4.0);
    let keypad = column![
        row_of(&[NumLock, KpSlash, KpAsterisk, KpMinus]),
        row_of(&[Kp7, Kp8, Kp9, KpPlus]),
        row_of(&[Kp4, Kp5, Kp6]),
        row_of(&[Kp1, Kp2, Kp3, KpEnter]),
        row_of(&[Kp0, KpDot]),
    ]
    .spacing(4.0);
    let top = row![nav, media, keypad].spacing(28.0);
    column![top, text("Other keys").size(14.0), extras_grid()].spacing(16.0).align_x(Center).into()
}

/// Every `Key` not placed in the keyboard/numpad layouts — so all 137 stay bindable (decision F),
/// even ones with no home on a drawn layout. Auto-complements the placed set: add a key to vocab
/// and it simply appears here until someone gives it a spot.
fn extras_grid() -> Element<'static, Message> {
    let mut placed = HashSet::new();
    for r in KB_ROWS {
        placed.extend(r.iter().cloned());
    }
    for list in [NP_NAV, NP_ARROWS, NP_MEDIA, NP_KEYPAD] {
        placed.extend(list.iter().cloned());
    }
    let extras: Vec<Key> = Key::ALL.iter().filter(|k| !placed.contains(*k)).cloned().collect();

    let mut col = column![].spacing(4.0);
    for chunk in extras.chunks(7) {
        let mut r = row![].spacing(4.0);
        for k in chunk {
            r = r.push(key(k.clone(), 1.7 * KW));
        }
        col = col.push(r);
    }
    col.into()
}

// --- action sets -----------------------------------------------------------------------------

fn action_sets(app: &App) -> Element<'static, Message> {
    // Sets: all of them; layers: only those of the set the input pages currently edit
    // (`EditTarget.set`) — layer refs resolve within a single action set (compile.rs: per-set).
    let sets: Vec<String> = app
        .editing
        .as_ref()
        .map(|e| e.doc.action_sets.iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default();
    let layers: Vec<String> = app
        .editing
        .as_ref()
        .and_then(|e| {
            e.doc.action_sets.iter().find(|s| s.name == e.target.set).map(|s| {
                s.layers.iter().map(|l| l.name.clone()).collect()
            })
        })
        .unwrap_or_default();

    let change: Element<'static, Message> = pick_list(None::<String>, sets, |s: &String| s.clone())
        .placeholder("Change Action Set")
        .style(style::labeled_pick)
        .on_select(|name| {
            Message::Editor(EditorMessage::ActionPicked(Action::ChangeActionSet(ActionSetRef(name))))
        })
        .width(280.0)
        .into();

    column![
        change,
        layer_pick("Hold Layer", layers.clone(), Action::HoldLayer),
        layer_pick("Add Layer", layers.clone(), Action::AddLayer),
        layer_pick("Remove Layer", layers, Action::RemoveLayer),
    ]
    .spacing(12.0)
    .align_x(Center)
    .into()
}

/// A layer combobox: placeholder names the action, click reveals the current set's layers, and
/// selecting one confirms `make(layer)` immediately.
fn layer_pick(
    placeholder: &'static str,
    layers: Vec<String>,
    make: fn(LayerRef) -> Action,
) -> Element<'static, Message> {
    pick_list(None::<String>, layers, |s: &String| s.clone())
        .placeholder(placeholder)
        .style(style::labeled_pick)
        .on_select(move |name| Message::Editor(EditorMessage::ActionPicked(make(LayerRef(name)))))
        .width(280.0)
        .into()
}

/// A display label for a key (UI-owned — vocab stays presentation-free).
fn key_label(k: &Key) -> &'static str {
    use Key::*;
    match k {
        LeftShift => "L Shift",
        RightShift => "R Shift",
        LeftCtrl => "L Ctrl",
        RightCtrl => "R Ctrl",
        LeftAlt => "L Alt",
        RightAlt => "R Alt",
        LeftMeta => "L Win",
        RightMeta => "R Win",
        Esc => "Esc",
        Tab => "Tab",
        CapsLock => "Caps",
        Backspace => "Bksp",
        Enter => "Enter",
        Space => "Space",
        Compose => "Compose",
        Menu => "Menu",
        Up => "↑",
        Down => "↓",
        Left => "←",
        Right => "→",
        Insert => "Ins",
        Delete => "Del",
        Home => "Home",
        End => "End",
        PageUp => "PgUp",
        PageDown => "PgDn",
        Grave => "`",
        K102nd => "<>",
        Minus => "-",
        Equal => "=",
        LeftBrace => "[",
        RightBrace => "]",
        Backslash => "\\",
        Semicolon => ";",
        Apostrophe => "'",
        Comma => ",",
        Dot => ".",
        Slash => "/",
        D1 => "1",
        D2 => "2",
        D3 => "3",
        D4 => "4",
        D5 => "5",
        D6 => "6",
        D7 => "7",
        D8 => "8",
        D9 => "9",
        D0 => "0",
        Q => "Q",
        W => "W",
        E => "E",
        R => "R",
        T => "T",
        Y => "Y",
        U => "U",
        I => "I",
        O => "O",
        P => "P",
        A => "A",
        S => "S",
        D => "D",
        F => "F",
        G => "G",
        H => "H",
        J => "J",
        K => "K",
        L => "L",
        Z => "Z",
        X => "X",
        C => "C",
        V => "V",
        B => "B",
        N => "N",
        M => "M",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        Print => "PrtSc",
        SysRq => "SysRq",
        ScrollLock => "ScrLk",
        Pause => "Pause",
        NumLock => "Num",
        KpSlash => "KP /",
        KpAsterisk => "KP *",
        KpMinus => "KP -",
        KpPlus => "KP +",
        KpEnter => "KP Ent",
        Kp7 => "KP 7",
        Kp8 => "KP 8",
        Kp9 => "KP 9",
        Kp4 => "KP 4",
        Kp5 => "KP 5",
        Kp6 => "KP 6",
        Kp1 => "KP 1",
        Kp2 => "KP 2",
        Kp3 => "KP 3",
        Kp0 => "KP 0",
        KpDot => "KP .",
        KpComma => "KP ,",
        KpEqual => "KP =",
        KpPlusMinus => "KP ±",
        KpLeftParen => "KP (",
        KpRightParen => "KP )",
        Mute => "Mute",
        VolumeDown => "Vol -",
        VolumeUp => "Vol +",
        MicMute => "Mic Mute",
        PlayPause => "Play/Pause",
        Play => "Play",
        PreviousSong => "Prev",
        NextSong => "Next",
        Rewind => "Rewind",
        FastForward => "Fast Fwd",
        StopCd => "Stop",
        PlayCd => "Play CD",
        PauseCd => "Pause CD",
        CloseCd => "Close CD",
        EjectCd => "Eject",
        EjectCloseCd => "Eject/Close",
        Back => "Browser Back",
        Forward => "Browser Fwd",
        BrightnessDown => "Bright -",
        BrightnessUp => "Bright +",
        BrightnessCycle => "Bright Cycle",
        BrightnessAuto => "Bright Auto",
        KbdIllumToggle => "Illum",
        KbdIllumDown => "Illum -",
        KbdIllumUp => "Illum +",
    }
}
