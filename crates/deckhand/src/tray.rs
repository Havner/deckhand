//! System-tray icon + menu.
//!
//! Linux uses **ksni** - a pure-Rust StatusNotifierItem over D-Bus (zbus), **no gtk**. (tray-icon's
//! Linux backend pulls gtk3/libappindicator, which shares process-global display state with
//! iced/wgpu; ksni sidesteps that and drops the gtk system deps entirely.) ksni runs its own D-Bus
//! service thread; menu clicks arrive through a process-global channel the app drains from a
//! subscription (see [`menu_receiver`]). The menu is a state-dependent **Show/Hide** plus **Quit**.
//!
//! Windows uses **tray-icon**'s native win32 backend (no gtk, zero system deps). It needs a thread
//! with a running Win32 message loop, so we give it a dedicated one that owns the (`!Send`) icon +
//! menu; menu/icon clicks arrive through tray-icon's global event handlers, which we forward to the
//! same process-global channel the Linux backend uses. The app talks *to* that thread (label update,
//! shutdown) with `PostThreadMessageW` control messages the loop handles inline.
//!
//! macOS still gets the stub - its tray must live on the main thread (tray-icon's Cocoa backend), a
//! follow-up.

use std::sync::OnceLock;

use crossbeam_channel::{Receiver, Sender, unbounded};

/// What a tray menu click means to the app.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MenuAction {
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
pub(crate) fn menu_receiver() -> Receiver<MenuAction> {
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
        /// Primary activation (left-click on the icon - on most SNI hosts a double-click, since a
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
    /// handle only holds a weak ref, so dropping it would *not* stop the service - shutdown must be
    /// explicit).
    pub(crate) struct Tray {
        handle: Handle<DeckhandTray>,
    }

    impl Tray {
        /// Show the tray icon, with the Show/Hide label reflecting `hidden`. Blocks briefly while the
        /// D-Bus service registers.
        pub(crate) fn enable(hidden: bool) -> Result<Tray, String> {
            let handle = DeckhandTray { hidden }.spawn().map_err(|e| e.to_string())?;
            Ok(Tray { handle })
        }

        /// Update the Show/Hide label for the current window state.
        pub(crate) fn set_label(&self, hidden: bool) {
            self.handle.update(|t| t.hidden = hidden);
        }

        /// Remove the tray icon and stop its D-Bus service.
        pub(crate) fn disable(self) {
            self.handle.shutdown();
        }
    }

    /// Decode the bundled tray PNG to a ksni [`Icon`](ksni::Icon). Returns `None` (tray shows without
    /// an icon) rather than failing if the image is missing/unsupported.
    fn load_icon() -> Option<ksni::Icon> {
        static PNG: &[u8] = include_bytes!("../assets/tray-icon.png");
        let (rgba, w, h) = crate::decode_png_rgba(PNG)?;
        // ksni wants ARGB per pixel (network byte order); our source is straight RGBA.
        let mut data = Vec::with_capacity(rgba.len());
        for c in rgba.as_chunks::<4>().0 {
            data.extend_from_slice(&[c[3], c[0], c[1], c[2]]);
        }
        Some(ksni::Icon { width: w as i32, height: h as i32, data })
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::Once;
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_APP,
    };
    use windows::core::{PCSTR, w};

    use super::{MenuAction, channel, toggle_label};

    /// Stable menu-item ids so the (process-global) menu-event handler can tell the two items apart
    /// without carrying muda's auto-generated ids off the tray thread.
    const ID_TOGGLE: &str = "toggle";
    const ID_QUIT: &str = "quit";

    /// Control messages we post to the tray thread's message loop. `WM_APP`-based so they never
    /// collide with the shell/menu window messages tray-icon's hidden window dispatches. `WM_TRAY_LABEL`
    /// carries the window's hidden-state in `wParam` (!=0 => hidden => show "Show").
    const WM_TRAY_LABEL: u32 = WM_APP + 1;
    const WM_TRAY_QUIT: u32 = WM_APP + 2;

    /// Process-once init: install the menu + icon event handlers, and opt the process into the system
    /// menu theme. tray-icon/muda store the handlers in a `OnceCell`, so only the first
    /// `set_event_handler` wins (later calls - including re-enabling the tray - are silently ignored)
    /// and clearing them is impossible. That's fine: our closures capture nothing but the
    /// process-global [`channel`], so a single permanent install correctly serves every tray the app
    /// creates over its lifetime, and teardown just drops the icon (no handler to remove).
    static INIT: Once = Once::new();

    fn init_once() {
        INIT.call_once(|| {
            follow_system_menu_theme();
            MenuEvent::set_event_handler(Some(|ev: MenuEvent| {
                let action = match ev.id.0.as_str() {
                    ID_QUIT => MenuAction::Quit,
                    _ => MenuAction::ToggleWindow, // ID_TOGGLE
                };
                let _ = channel().0.send(action);
            }));
            TrayIconEvent::set_event_handler(Some(|ev: TrayIconEvent| {
                // Double-clicking the icon toggles the window (the classic Windows tray gesture); the
                // right-click menu covers Show/Hide + Quit explicitly.
                if let TrayIconEvent::DoubleClick { .. } = ev {
                    let _ = channel().0.send(MenuAction::ToggleWindow);
                }
            }));
        });
    }

    /// Make tray-icon's **context menu** follow the Windows dark/light setting. That menu is a native
    /// `TrackPopupMenu` popup, which muda's `MenuTheme` does NOT reach (it themes only menu *bars*), so
    /// its colors come from the process's preferred app mode - light by default. We flip that with the
    /// undocumented uxtheme exports `SetPreferredAppMode` (ordinal 135) + `FlushMenuThemes` (136), the
    /// standard recipe for dark Win32 menus on Windows 10 1903+/11. `AllowDark` = follow the system
    /// (dark only when the user's theme is dark). All best-effort: on older Windows the ordinals are
    /// absent and the menu simply stays light.
    fn follow_system_menu_theme() {
        /// `PreferredAppMode::AllowDark` - honor the system dark/light setting.
        const ALLOW_DARK: i32 = 1;
        unsafe {
            let Ok(uxtheme) = LoadLibraryW(w!("uxtheme.dll")) else {
                return;
            };
            // Both exports are ordinal-only (unnamed): #135 SetPreferredAppMode, #136 FlushMenuThemes.
            if let Some(p) = GetProcAddress(uxtheme, PCSTR(135 as *const u8)) {
                let set_preferred_app_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(p);
                set_preferred_app_mode(ALLOW_DARK);
            }
            if let Some(p) = GetProcAddress(uxtheme, PCSTR(136 as *const u8)) {
                let flush_menu_themes: extern "system" fn() = std::mem::transmute(p);
                flush_menu_themes();
            }
        }
    }

    /// Build the menu + tray icon on the current (message-loop) thread. Returns the live icon (drop =
    /// remove) and a handle to the Show/Hide item (for live label updates on this thread).
    fn build(hidden: bool) -> Result<(TrayIcon, MenuItem), String> {
        let toggle = MenuItem::with_id(ID_TOGGLE, toggle_label(hidden), true, None);
        let quit = MenuItem::with_id(ID_QUIT, "Quit", true, None);
        let menu = Menu::new();
        menu.append(&toggle).map_err(|e| e.to_string())?;
        menu.append(&quit).map_err(|e| e.to_string())?;

        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("deckhand")
            // Leave left-click free for the double-click toggle; the menu opens on right-click.
            .with_menu_on_left_click(false);
        if let Some(icon) = load_icon() {
            builder = builder.with_icon(icon);
        }
        let tray = builder.build().map_err(|e| e.to_string())?;
        // `toggle` is an Rc handle to the same item now owned by `menu`/`tray`; keeping it lets us
        // retitle the item after the menu was boxed into the icon.
        Ok((tray, toggle))
    }

    /// The tray thread: install handlers, build the icon, report readiness (thread id) or the build
    /// error back to [`Tray::enable`], then pump the Win32 message loop until `WM_TRAY_QUIT`. The
    /// `TrayIcon` drops on the way out, removing the icon from the tray.
    fn run(hidden: bool, ready: mpsc::Sender<Result<u32, String>>) {
        init_once();
        // `_tray` is the RAII guard: it must stay bound for the loop's lifetime - dropping it removes
        // the icon from the tray.
        let (_tray, toggle) = match build(hidden) {
            Ok(pair) => pair,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        // Building the icon created this thread's hidden window (and thus its message queue), so the
        // id is now safe for `PostThreadMessageW`.
        let thread_id = unsafe { GetCurrentThreadId() };
        if ready.send(Ok(thread_id)).is_err() {
            return; // enable() gave up; `_tray` drops here and the icon is removed.
        }

        let mut msg = MSG::default();
        loop {
            // Blocks until a message arrives (window messages for the icon/menu, or our thread
            // control messages). `> 0` = message, `0` = WM_QUIT, `-1` = error.
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if ret.0 <= 0 {
                break;
            }
            // Thread messages (posted via PostThreadMessageW) have a null hwnd - our control channel.
            if msg.hwnd.0.is_null() {
                match msg.message {
                    WM_TRAY_LABEL => {
                        toggle.set_text(toggle_label(msg.wParam.0 != 0));
                        continue;
                    }
                    WM_TRAY_QUIT => break,
                    _ => {}
                }
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // `_tray` drops here -> icon removed. Handlers stay installed (see `HANDLERS`); with no icon
        // alive they simply never fire.
    }

    /// A live system tray, backed by a dedicated message-loop thread. The app drives it with
    /// `PostThreadMessageW` control messages (label update / shutdown).
    pub(crate) struct Tray {
        /// The tray thread's Win32 id, for `PostThreadMessageW`.
        thread_id: u32,
        /// Join handle for the message-loop thread (joined on [`Tray::disable`]).
        join: Option<JoinHandle<()>>,
    }

    impl Tray {
        /// Show the tray icon with the Show/Hide label reflecting `hidden`. Spawns the message-loop
        /// thread and blocks until it has built the icon (or failed to).
        pub(crate) fn enable(hidden: bool) -> Result<Tray, String> {
            // The icon + menu are `!Send` (Rc-based), so they must be created and dropped on the loop
            // thread; only the thread id + a build result travel back here.
            let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
            let join = std::thread::Builder::new()
                .name("deckhand-tray".into())
                .spawn(move || run(hidden, ready_tx))
                .map_err(|e| e.to_string())?;
            match ready_rx.recv() {
                Ok(Ok(thread_id)) => Ok(Tray { thread_id, join: Some(join) }),
                Ok(Err(e)) => {
                    let _ = join.join();
                    Err(e)
                }
                Err(_) => {
                    let _ = join.join();
                    Err("tray thread exited before signalling readiness".into())
                }
            }
        }

        /// Update the Show/Hide label for the current window state (marshalled onto the tray thread,
        /// which owns the `!Send` menu item).
        pub(crate) fn set_label(&self, hidden: bool) {
            self.post(WM_TRAY_LABEL, WPARAM(hidden as usize));
        }

        /// Remove the tray icon and stop its message loop, joining the thread.
        pub(crate) fn disable(mut self) {
            self.post(WM_TRAY_QUIT, WPARAM(0));
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }

        /// Post a control message to the tray thread. Best-effort: if the loop has already exited the
        /// post just fails and is ignored.
        fn post(&self, msg: u32, wparam: WPARAM) {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, msg, wparam, LPARAM(0));
            }
        }
    }

    /// Decode the bundled tray PNG to a tray-icon [`Icon`] (straight RGBA8, as `from_rgba` wants).
    /// Returns `None` (tray shows without an icon) rather than failing if the image is missing.
    fn load_icon() -> Option<Icon> {
        static PNG: &[u8] = include_bytes!("../assets/tray-icon.png");
        let (rgba, w, h) = crate::decode_png_rgba(PNG)?;
        Icon::from_rgba(rgba, w, h).ok()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod imp {
    /// Stub tray for platforms without an implementation yet (macOS tray = a follow-up).
    pub(crate) struct Tray;

    impl Tray {
        pub(crate) fn enable(_hidden: bool) -> Result<Tray, String> {
            Err("system tray is not implemented on this platform yet".into())
        }
        pub(crate) fn set_label(&self, _hidden: bool) {}
        pub(crate) fn disable(self) {}
    }
}

pub(crate) use imp::Tray;
