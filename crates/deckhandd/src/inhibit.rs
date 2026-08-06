//! `--prevent-sleep`: keep the session awake while the daemon forwards controller input.
//!
//! When a controller's input is consumed here (and forwarded over the network), the compositor sees
//! no local activity and eventually blanks the screen / auto-suspends. We hold a freedesktop
//! **ScreenSaver** inhibitor on the **session** D-Bus for the daemon's lifetime — the same mechanism
//! browsers, media players, and game launchers (Heroic/Electron) use. It is **unprivileged** (session
//! bus, no root), **DE-provided** (KDE/GNOME/XFCE/… all implement it), and a plain **bus call** (no
//! X/Wayland connection), so the daemon stays headless. Linux only.

use zbus::blocking::Connection;

#[zbus::proxy(
    interface = "org.freedesktop.ScreenSaver",
    default_service = "org.freedesktop.ScreenSaver",
    default_path = "/org/freedesktop/ScreenSaver"
)]
trait ScreenSaver {
    fn inhibit(&self, application_name: &str, reason_for_inhibit: &str) -> zbus::Result<u32>;
    fn un_inhibit(&self, cookie: u32) -> zbus::Result<()>;
}

/// Holds a ScreenSaver inhibition while alive; releases it on drop (and, as a backstop, the service
/// releases it automatically when we disconnect from the bus on exit).
pub struct SleepInhibitor {
    conn: Connection,
    cookie: u32,
}

impl SleepInhibitor {
    /// Take the inhibition. **Best-effort**: if the session bus or the ScreenSaver service is
    /// unavailable (e.g. a headless login), log a warning and return `None` rather than failing the
    /// daemon.
    pub fn acquire() -> Option<SleepInhibitor> {
        let conn = match Connection::session() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("prevent-sleep: no session bus ({e}) — sleep NOT inhibited");
                return None;
            }
        };
        let proxy = match ScreenSaverProxyBlocking::new(&conn) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("prevent-sleep: ScreenSaver proxy unavailable ({e}) — sleep NOT inhibited");
                return None;
            }
        };
        match proxy.inhibit("deckhand", "forwarding controller input") {
            Ok(cookie) => {
                log::info!(
                    "prevent-sleep: idle inhibited via org.freedesktop.ScreenSaver (cookie {cookie})"
                );
                Some(SleepInhibitor { conn, cookie })
            }
            Err(e) => {
                log::warn!("prevent-sleep: Inhibit failed ({e}) — sleep NOT inhibited");
                None
            }
        }
    }
}

impl Drop for SleepInhibitor {
    fn drop(&mut self) {
        if let Ok(proxy) = ScreenSaverProxyBlocking::new(&self.conn)
            && let Err(e) = proxy.un_inhibit(self.cookie)
        {
            log::warn!("prevent-sleep: UnInhibit failed ({e})");
        }
    }
}
