//! `--prevent-sleep [MODE]`: keep the machine from auto-suspending while the daemon forwards
//! controller input.
//!
//! When a controller's input is consumed here (and forwarded over the network), the compositor sees
//! no local activity and eventually auto-suspends the machine (and/or blanks the screen). We hold an
//! idle/sleep inhibitor on the D-Bus for the daemon's lifetime. Which one depends on `MODE`
//! (`PreventSleep`), because there is no single interface every environment implements:
//!
//! - **`screensaver`** - `org.freedesktop.ScreenSaver` (session bus). Inhibits *idle*, so it stops
//!   auto-suspend **and** screen blanking. Broadly implemented (KDE and others). Cookie-based.
//! - **`powermanagement`** - `org.freedesktop.PowerManagement.Inhibit` (session bus). Inhibits
//!   *suspend only* - the screen still blanks. KDE/XFCE/MATE. Cookie-based. **Not on GNOME.**
//! - **`gnome`** - `org.gnome.SessionManager.Inhibit` with `flags=4` (session bus). Suspend only,
//!   screen still blanks. GNOME's equivalent of `powermanagement`. Cookie-based.
//! - **`login1`** - `org.freedesktop.login1.Manager.Inhibit("sleep", ..., "block")` (**system** bus).
//!   Suspend only, screen still blanks; the lock is held by an open fd, released when it closes.
//!   Works on any systemd host (incl. no-DE), but a *block* sleep lock may need polkit auth on
//!   locked-down policies (e.g. SteamOS), and it also blocks *manual* suspend.
//! - **`auto`** (default) - try `powermanagement`, then `gnome`, then `login1`; first that answers
//!   wins. Skips `screensaver` so the screen stays free to blank.
//!
//! All modes are **unprivileged** where available (system bus for `login1`) and **best-effort**: if
//! nothing answers, we log a warning and run without an inhibitor rather than failing the daemon.
//! Linux only.

use zbus::blocking::Connection;
use zbus::zvariant::OwnedFd;

use crate::PreventSleep;

/// App name / reason reported to the inhibitor services (shown in DE "an app is preventing sleep" UI).
const APP: &str = "deckhand";
const REASON: &str = "forwarding controller input";
/// `org.gnome.SessionManager` inhibit flag bit 4 = "inhibit suspending the session or computer"
/// (bit 8 would be idle/blank - deliberately not set, so the screen still blanks).
const GNOME_INHIBIT_SUSPEND: u32 = 4;

#[zbus::proxy(
    interface = "org.freedesktop.ScreenSaver",
    default_service = "org.freedesktop.ScreenSaver",
    default_path = "/org/freedesktop/ScreenSaver"
)]
trait ScreenSaver {
    fn inhibit(&self, application_name: &str, reason_for_inhibit: &str) -> zbus::Result<u32>;
    fn un_inhibit(&self, cookie: u32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.PowerManagement.Inhibit",
    default_service = "org.freedesktop.PowerManagement.Inhibit",
    default_path = "/org/freedesktop/PowerManagement/Inhibit"
)]
trait PowerManagement {
    fn inhibit(&self, application_name: &str, reason_for_inhibit: &str) -> zbus::Result<u32>;
    fn un_inhibit(&self, cookie: u32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.gnome.SessionManager",
    default_service = "org.gnome.SessionManager",
    default_path = "/org/gnome/SessionManager"
)]
trait GnomeSession {
    fn inhibit(&self, app_id: &str, toplevel_xid: u32, reason: &str, flags: u32)
    -> zbus::Result<u32>;
    fn uninhibit(&self, cookie: u32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;
}

/// What we're holding, and how to release it. Cookie backends re-open a proxy on the retained
/// connection to un-inhibit; `login1` releases simply by dropping the fd.
enum Held {
    ScreenSaver { conn: Connection, cookie: u32 },
    PowerManagement { conn: Connection, cookie: u32 },
    Gnome { conn: Connection, cookie: u32 },
    Login1 { _fd: OwnedFd },
}

/// Holds a sleep/idle inhibition while alive; releases it on drop (and, as a backstop, the service
/// releases it automatically when we disconnect from the bus / close the fd on exit).
pub struct SleepInhibitor(Held);

impl SleepInhibitor {
    /// Take the inhibition for `mode`. **Best-effort**: on any failure log a warning and return
    /// `None` rather than failing the daemon. `Auto` tries the screen-friendly backends in turn and
    /// warns only if none work; an explicitly-named mode warns naming that backend.
    pub fn acquire(mode: PreventSleep) -> Option<SleepInhibitor> {
        match mode {
            PreventSleep::Auto => {
                Self::powermanagement()
                    .or_else(Self::gnome)
                    .or_else(Self::login1)
                    .or_else(|| {
                        log::warn!(
                            "prevent-sleep: auto found no working backend \
                             (tried powermanagement, gnome, login1) - sleep NOT inhibited"
                        );
                        None
                    })
            }
            PreventSleep::Screensaver => Self::warn_if_none("screensaver", Self::screensaver()),
            PreventSleep::Powermanagement => {
                Self::warn_if_none("powermanagement", Self::powermanagement())
            }
            PreventSleep::Gnome => Self::warn_if_none("gnome", Self::gnome()),
            PreventSleep::Login1 => Self::warn_if_none("login1", Self::login1()),
        }
    }

    fn warn_if_none(name: &str, held: Option<SleepInhibitor>) -> Option<SleepInhibitor> {
        if held.is_none() {
            log::warn!("prevent-sleep: {name} backend unavailable - sleep NOT inhibited");
        }
        held
    }

    fn screensaver() -> Option<SleepInhibitor> {
        let conn = session_bus()?;
        let proxy = ScreenSaverProxyBlocking::new(&conn)
            .map_err(|e| log::debug!("prevent-sleep: ScreenSaver proxy: {e}"))
            .ok()?;
        match proxy.inhibit(APP, REASON) {
            Ok(cookie) => {
                log::info!("prevent-sleep: held via org.freedesktop.ScreenSaver (cookie {cookie})");
                Some(SleepInhibitor(Held::ScreenSaver { conn, cookie }))
            }
            Err(e) => {
                log::debug!("prevent-sleep: ScreenSaver.Inhibit: {e}");
                None
            }
        }
    }

    fn powermanagement() -> Option<SleepInhibitor> {
        let conn = session_bus()?;
        let proxy = PowerManagementProxyBlocking::new(&conn)
            .map_err(|e| log::debug!("prevent-sleep: PowerManagement proxy: {e}"))
            .ok()?;
        match proxy.inhibit(APP, REASON) {
            Ok(cookie) => {
                log::info!(
                    "prevent-sleep: held via org.freedesktop.PowerManagement.Inhibit (cookie {cookie})"
                );
                Some(SleepInhibitor(Held::PowerManagement { conn, cookie }))
            }
            Err(e) => {
                log::debug!("prevent-sleep: PowerManagement.Inhibit: {e}");
                None
            }
        }
    }

    fn gnome() -> Option<SleepInhibitor> {
        let conn = session_bus()?;
        let proxy = GnomeSessionProxyBlocking::new(&conn)
            .map_err(|e| log::debug!("prevent-sleep: GNOME SessionManager proxy: {e}"))
            .ok()?;
        match proxy.inhibit(APP, 0, REASON, GNOME_INHIBIT_SUSPEND) {
            Ok(cookie) => {
                log::info!("prevent-sleep: held via org.gnome.SessionManager (cookie {cookie})");
                Some(SleepInhibitor(Held::Gnome { conn, cookie }))
            }
            Err(e) => {
                log::debug!("prevent-sleep: GNOME SessionManager.Inhibit: {e}");
                None
            }
        }
    }

    fn login1() -> Option<SleepInhibitor> {
        let conn = Connection::system()
            .map_err(|e| log::debug!("prevent-sleep: no system bus: {e}"))
            .ok()?;
        let proxy = Login1ManagerProxyBlocking::new(&conn)
            .map_err(|e| log::debug!("prevent-sleep: login1 proxy: {e}"))
            .ok()?;
        match proxy.inhibit("sleep", APP, REASON, "block") {
            Ok(fd) => {
                log::info!("prevent-sleep: held via org.freedesktop.login1 (block sleep)");
                Some(SleepInhibitor(Held::Login1 { _fd: fd }))
            }
            Err(e) => {
                log::debug!("prevent-sleep: login1 Inhibit(sleep, block): {e}");
                None
            }
        }
    }
}

impl Drop for SleepInhibitor {
    fn drop(&mut self) {
        let released = match &self.0 {
            Held::ScreenSaver { conn, cookie } => {
                ScreenSaverProxyBlocking::new(conn).and_then(|p| p.un_inhibit(*cookie))
            }
            Held::PowerManagement { conn, cookie } => {
                PowerManagementProxyBlocking::new(conn).and_then(|p| p.un_inhibit(*cookie))
            }
            Held::Gnome { conn, cookie } => {
                GnomeSessionProxyBlocking::new(conn).and_then(|p| p.uninhibit(*cookie))
            }
            // The lock is the open fd; dropping `self.0` closes it and releases the inhibitor.
            Held::Login1 { .. } => Ok(()),
        };
        if let Err(e) = released {
            log::warn!("prevent-sleep: releasing inhibitor failed ({e})");
        }
    }
}

/// Connect to the session bus, logging (debug) and returning `None` on failure - e.g. a headless
/// login with no session bus.
fn session_bus() -> Option<Connection> {
    Connection::session()
        .map_err(|e| log::debug!("prevent-sleep: no session bus: {e}"))
        .ok()
}
