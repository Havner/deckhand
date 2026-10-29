//! Force feedback for the virtual gamepad, answered on its own thread.
//!
//! A game's effect upload/erase (`EVIOCSFF`/`EVIOCRMFF`) on a uinput device is a synchronous
//! handshake: the kernel blocks the calling thread until the device's owner answers with
//! `UI_BEGIN_FF_UPLOAD`/`UI_END_FF_UPLOAD` (or the `_ERASE` pair). Games (SDL, Wine's winebus)
//! re-upload the effect on every strength change, so whoever answers sits on the game's hot path.
//! Answered from the mapping loop (once per pass, after the mapping work) each upload stalled the
//! game for up to ~10 ms - enough to make a game that drives rumble from its audio thread crackle.
//! So this thread owns the gamepad `VirtualDevice`, blocks in `poll()` on it, answers each request
//! at once, and publishes the effective rumble for `Sink::poll_rumble` to read. Playback (`EV_FF`
//! play/stop) never blocks a game; it is handled here only because it rides the same fd.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{EventSummary, FFEffectKind, InputEvent, UInputCode};

use crate::gamepad::Rumble;

/// Effect slots the virtual gamepad advertises (`with_ff_effects_max`).
pub(super) const FF_MAX_EFFECTS: u32 = 16;

/// The running force-feedback thread. Dropping it stops and joins the thread, which drops the
/// gamepad `VirtualDevice` it owns.
pub(super) struct FfThread {
    // Latest effective rumble, packed `strong << 16 | weak` so the pair is read consistently.
    rumble: Arc<AtomicU32>,
    // eventfd in the thread's poll set; a write wakes it to exit.
    stop: File,
    handle: Option<JoinHandle<io::Result<()>>>,
}

impl FfThread {
    /// Start serving force feedback for `pad` (its fd must be non-blocking).
    pub(super) fn spawn(pad: VirtualDevice) -> io::Result<Self> {
        let stop = eventfd()?;
        let stop_rx = stop.try_clone()?;
        let rumble = Arc::new(AtomicU32::new(0));
        let published = rumble.clone();
        let handle = std::thread::Builder::new()
            .name("deckhand-ff".into())
            .spawn(move || run(pad, &stop_rx, &published))?;
        Ok(Self { rumble, stop, handle: Some(handle) })
    }

    /// The currently-commanded rumble, or the error that ended the thread.
    pub(super) fn rumble(&mut self) -> io::Result<Rumble> {
        if self.handle.as_ref().is_none_or(|h| h.is_finished()) {
            return Err(match self.handle.take().map(|h| h.join()) {
                Some(Ok(Err(e))) => e,
                _ => io::Error::other("force-feedback thread exited"),
            });
        }
        let v = self.rumble.load(Ordering::Relaxed);
        Ok(Rumble { strong: (v >> 16) as u16, weak: v as u16 })
    }
}

impl Drop for FfThread {
    fn drop(&mut self) {
        let _ = (&self.stop).write_all(&1u64.to_ne_bytes());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn eventfd() -> io::Result<File> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor we exclusively own.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// The thread body: sleep until FF traffic, a stop, or the playing effect's auto-stop deadline;
/// answer/apply what arrived; publish the effective rumble.
fn run(mut pad: VirtualDevice, stop: &File, rumble: &AtomicU32) -> io::Result<()> {
    let mut ff = FfState::new();
    loop {
        // Round the deadline up so we don't wake just short of it and spin.
        let timeout = ff.until.map_or(-1, |t| {
            let left = t.saturating_duration_since(Instant::now());
            left.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32
        });
        let mut fds = [
            libc::pollfd { fd: pad.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: stop.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) } < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if fds[1].revents != 0 {
            return Ok(());
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("virtual gamepad fd failed"));
        }
        if fds[0].revents & libc::POLLIN != 0 {
            handle_events(&mut pad, &mut ff)?;
        }
        ff.expire(Instant::now());
        let r = ff.current();
        rumble.store((r.strong as u32) << 16 | r.weak as u32, Ordering::Relaxed);
    }
}

/// Read the queued FF traffic and answer/apply it. An upload/erase is answered when its
/// `FFUploadEvent`/`FFEraseEvent` drops (`UI_END_FF_*`), which releases the waiting game.
fn handle_events(pad: &mut VirtualDevice, ff: &mut FfState) -> io::Result<()> {
    // The fd is non-blocking, so a spurious wake reads `WouldBlock`; anything left over after this
    // batch wakes the next `poll()` straight away.
    let events: Vec<InputEvent> = match pad.fetch_events() {
        Ok(iter) => iter.collect(),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
        Err(e) => return Err(e),
    };
    for event in events {
        match event.destructure() {
            // `FFUploadEvent` owns a dup of the fd, so it doesn't borrow `pad`.
            EventSummary::UInput(ev, UInputCode::UI_FF_UPLOAD, _) => {
                let mut up = pad.process_ff_upload(ev)?;
                let data = up.effect();
                let effect = match data.kind {
                    FFEffectKind::Rumble { strong_magnitude, weak_magnitude } => Some(FfEffect {
                        rumble: Rumble { strong: strong_magnitude, weak: weak_magnitude },
                        length_ms: data.replay.length as u32,
                        delay_ms: data.replay.delay as u32,
                    }),
                    _ => None,
                };
                match ff.upload(up.effect_id(), effect) {
                    Some(id) => {
                        up.set_effect_id(id);
                        up.set_retval(0);
                    }
                    None => up.set_retval(-1), // pool exhausted
                }
            }
            EventSummary::UInput(ev, UInputCode::UI_FF_ERASE, _) => {
                let id = pad.process_ff_erase(ev)?.effect_id() as i16;
                ff.erase(id);
            }
            EventSummary::ForceFeedback(_, effect, value) => {
                ff.play(effect.0 as i16, value, Instant::now());
            }
            _ => {}
        }
    }
    Ok(())
}

/// A stored force-feedback effect: its rumble magnitudes plus the replay timing we must honor
/// ourselves (the uinput-userspace FF model makes us stop playback when `length` elapses - the
/// kernel doesn't do it for us).
struct FfEffect {
    rumble: Rumble,
    length_ms: u32, // 0 = play until an explicit stop
    delay_ms: u32,
}

/// The FF state machine, kept free of the fd so it's unit-testable: a small pool of effect ids,
/// the effects uploaded for each, the one currently playing, and when it should auto-stop.
struct FfState {
    free_ids: Vec<i16>,
    effects: HashMap<i16, FfEffect>,
    playing: Option<i16>,
    until: Option<Instant>,
}

impl FfState {
    fn new() -> Self {
        Self {
            free_ids: (0..FF_MAX_EFFECTS as i16).rev().collect(),
            effects: HashMap::new(),
            playing: None,
            until: None,
        }
    }

    /// An upload. New effects (id `-1`) get a pooled id; updates (a game changing an effect in
    /// place - e.g. dropping rumble to 0) keep theirs. Returns the id, `None` if the pool is
    /// exhausted. A non-rumble effect (`None`) holds its id but plays nothing.
    fn upload(&mut self, id: i16, effect: Option<FfEffect>) -> Option<i16> {
        let id = if id >= 0 { id } else { self.free_ids.pop()? };
        match effect {
            Some(e) => self.effects.insert(id, e),
            None => self.effects.remove(&id),
        };
        Some(id)
    }

    /// An effect is erased - reclaim its id.
    fn erase(&mut self, id: i16) {
        self.effects.remove(&id);
        if self.playing == Some(id) {
            self.playing = None;
            self.until = None;
        }
        self.free_ids.push(id);
    }

    /// Play (`value` = repeat count >= 1) / stop (0) of an effect id. On play we honor the effect's
    /// replay `length` by scheduling an auto-stop; `length == 0` plays until an explicit stop.
    /// (Single-slot: last-played effect wins.)
    fn play(&mut self, id: i16, value: i32, now: Instant) {
        if value != 0 {
            self.playing = Some(id);
            let (length, delay) =
                self.effects.get(&id).map(|e| (e.length_ms, e.delay_ms)).unwrap_or((0, 0));
            self.until = (length > 0).then(|| {
                let total = delay + length * value.max(1) as u32;
                now + Duration::from_millis(total as u64)
            });
        } else if self.playing == Some(id) {
            self.playing = None;
            self.until = None;
        }
    }

    /// Auto-stop once the replay length has elapsed.
    fn expire(&mut self, now: Instant) {
        if self.until.is_some_and(|t| now >= t) {
            self.playing = None;
            self.until = None;
        }
    }

    /// The currently-commanded rumble (zero if nothing is playing).
    fn current(&self) -> Rumble {
        let effect = self.playing.and_then(|id| self.effects.get(&id));
        effect.map(|e| e.rumble.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rumble(strong: u16, weak: u16, length_ms: u32) -> Option<FfEffect> {
        Some(FfEffect { rumble: Rumble { strong, weak }, length_ms, delay_ms: 0 })
    }

    #[test]
    fn upload_pools_ids_and_updates_in_place() {
        let mut ff = FfState::new();
        let a = ff.upload(-1, rumble(1, 2, 0)).unwrap();
        let b = ff.upload(-1, rumble(3, 4, 0)).unwrap();
        assert_ne!(a, b);
        ff.play(a, 1, Instant::now());
        assert_eq!(ff.current(), Rumble { strong: 1, weak: 2 });
        // An in-place update keeps the id and takes effect while playing.
        assert_eq!(ff.upload(a, rumble(5, 6, 0)), Some(a));
        assert_eq!(ff.current(), Rumble { strong: 5, weak: 6 });
    }

    #[test]
    fn pool_exhausts_and_erase_reclaims() {
        let mut ff = FfState::new();
        let ids: Vec<_> =
            (0..FF_MAX_EFFECTS).map(|_| ff.upload(-1, rumble(1, 1, 0)).unwrap()).collect();
        assert_eq!(ff.upload(-1, rumble(1, 1, 0)), None);
        ff.play(ids[3], 1, Instant::now());
        ff.erase(ids[3]);
        assert!(ff.current().is_zero());
        assert_eq!(ff.upload(-1, rumble(1, 1, 0)), Some(ids[3]));
    }

    #[test]
    fn stop_only_affects_the_playing_effect() {
        let mut ff = FfState::new();
        let a = ff.upload(-1, rumble(1, 1, 0)).unwrap();
        let b = ff.upload(-1, rumble(2, 2, 0)).unwrap();
        ff.play(a, 1, Instant::now());
        ff.play(b, 0, Instant::now());
        assert_eq!(ff.current(), Rumble { strong: 1, weak: 1 });
        ff.play(a, 0, Instant::now());
        assert!(ff.current().is_zero());
    }

    #[test]
    fn replay_length_auto_stops() {
        let mut ff = FfState::new();
        let t0 = Instant::now();
        let long =
            FfEffect { rumble: Rumble { strong: 9, weak: 9 }, length_ms: 100, delay_ms: 20 };
        let id = ff.upload(-1, Some(long)).unwrap();
        ff.play(id, 2, t0); // delay + 2 x length = 220 ms
        ff.expire(t0 + Duration::from_millis(219));
        assert!(!ff.current().is_zero());
        ff.expire(t0 + Duration::from_millis(220));
        assert!(ff.current().is_zero());
        // length 0 plays until an explicit stop.
        let inf = ff.upload(-1, rumble(1, 1, 0)).unwrap();
        ff.play(inf, 1, t0);
        assert_eq!(ff.until, None);
    }
}
