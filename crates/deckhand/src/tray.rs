//! System-tray icon + menu.
//!
//! Linux uses **ksni** — a pure-Rust StatusNotifierItem over D-Bus (zbus), **no gtk**. (tray-icon's
//! Linux backend pulls gtk3/libappindicator, which shares process-global display state with
//! iced/wgpu; ksni sidesteps that and drops the gtk system deps entirely.) ksni runs its own D-Bus
//! service thread; menu clicks arrive through a process-global channel the app drains from a
//! subscription (see [`menu_receiver`]). The menu is a state-dependent **Show/Hide** plus **Quit**.
//!
//! Other platforms get a stub for now — the Windows/macOS tray (tray-icon's native win32/Cocoa
//! backends, no gtk) is a follow-up.

use std::sync::OnceLock;

use crossbeam_channel::{Receiver, Sender, unbounded};

/// What a tray menu click means to the app.
#[derive(Debug, Clone, Copy)]
pub enum MenuAction {
    /// Show the window if hidden, hide it if shown.
    ToggleWindow,
    /// Quit the application.
    Quit,
}

/// The process-global menu-action channel: the tray's menu callbacks send, the app's subscription
/// receives. Created once; crossbeam receivers clone, so the subscription just clones its end.
static CHANNEL: OnceLock<(Sender<MenuAction>, Receiver<MenuAction>)> = OnceLock::new();

fn channel() -> &'static (Sender<MenuAction>, Receiver<MenuAction>) {
    CHANNEL.get_or_init(unbounded)
}

/// A clone of the menu-action receiver for the app's subscription to drain.
pub fn menu_receiver() -> Receiver<MenuAction> {
    channel().1.clone()
}

/// The Show/Hide label for the current window state.
fn toggle_label(hidden: bool) -> &'static str {
    if hidden { "Show" } else { "Hide" }
}

#[cfg(target_os = "linux")]
mod imp {
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::StandardItem;

    use super::{MenuAction, channel, toggle_label};

    /// The ksni tray model: just the current window state (drives the Show/Hide label).
    struct DeckhandTray {
        hidden: bool,
    }

    impl ksni::Tray for DeckhandTray {
        fn id(&self) -> String {
            "deckhand".into()
        }
        fn title(&self) -> String {
            "deckhand".into()
        }
        /// Primary activation (left-click on the icon — on most SNI hosts a double-click, since a
        /// single click opens the menu): toggle the window like the Show/Hide item.
        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = channel().0.send(MenuAction::ToggleWindow);
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            load_icon().into_iter().collect()
        }
        fn menu(&self) -> Vec<ksni::menu::MenuItem<Self>> {
            vec![
                StandardItem {
                    label: toggle_label(self.hidden).into(),
                    activate: Box::new(|_| {
                        let _ = channel().0.send(MenuAction::ToggleWindow);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|_| {
                        let _ = channel().0.send(MenuAction::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    /// A live system tray. Holds the ksni service handle; [`Tray::disable`] removes the icon (the
    /// handle only holds a weak ref, so dropping it would *not* stop the service — shutdown must be
    /// explicit).
    pub struct Tray {
        handle: Handle<DeckhandTray>,
    }

    impl Tray {
        /// Show the tray icon, with the Show/Hide label reflecting `hidden`. Blocks briefly while the
        /// D-Bus service registers.
        pub fn enable(hidden: bool) -> Result<Tray, String> {
            let handle = DeckhandTray { hidden }.spawn().map_err(|e| e.to_string())?;
            Ok(Tray { handle })
        }

        /// Update the Show/Hide label for the current window state.
        pub fn set_label(&self, hidden: bool) {
            self.handle.update(|t| t.hidden = hidden);
        }

        /// Remove the tray icon and stop its D-Bus service.
        pub fn disable(self) {
            self.handle.shutdown();
        }
    }

    /// Decode the bundled tray PNG to a ksni [`Icon`](ksni::Icon) (ARGB32, network byte order).
    /// Returns `None` (tray shows without an icon) rather than failing if the image is
    /// missing/unsupported.
    fn load_icon() -> Option<ksni::Icon> {
        static PNG: &[u8] = include_bytes!("../assets/tray-icon.png");
        let mut reader = png::Decoder::new(PNG).read_info().ok()?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).ok()?;
        if info.bit_depth != png::BitDepth::Eight {
            return None;
        }
        let px = &buf[..info.buffer_size()];
        // Source is RGB(A)8; ksni wants ARGB per pixel (network byte order).
        let mut data = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        match info.color_type {
            png::ColorType::Rgba => {
                for c in px.chunks_exact(4) {
                    data.extend_from_slice(&[c[3], c[0], c[1], c[2]]);
                }
            }
            png::ColorType::Rgb => {
                for c in px.chunks_exact(3) {
                    data.extend_from_slice(&[255, c[0], c[1], c[2]]);
                }
            }
            _ => return None,
        }
        Some(ksni::Icon { width: info.width as i32, height: info.height as i32, data })
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    /// Stub tray for platforms without an implementation yet (Windows/macOS tray = a follow-up).
    pub struct Tray;

    impl Tray {
        pub fn enable(_hidden: bool) -> Result<Tray, String> {
            Err("system tray is not implemented on this platform yet".into())
        }
        pub fn set_label(&self, _hidden: bool) {}
        pub fn disable(self) {}
    }
}

pub use imp::Tray;
