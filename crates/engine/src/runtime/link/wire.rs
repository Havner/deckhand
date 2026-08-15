//! The §6 network wire vocabulary + codec (PLAN §6.1/§6.2 slice 2). Pure data + (de)serialization
//! — **no sockets** here (the bridge threads that use these land in slice 3), so it is fully
//! unit-testable over in-memory buffers.
//!
//! Two transports, split by idempotency (PLAN §6.1):
//! - **UDP** (unreliable, latest-wins): controller snapshots client→server ([`FramePacket`]) and the
//!   rumble/click back-channel server→client ([`Downlink`]). Datagrams are message-bounded, so these
//!   are bare postcard blobs — no length prefix ([`encode`]/[`decode`]).
//! - **TCP** (reliable, ordered): the config uplink + device lifecycle events client→server
//!   ([`Uplink`]). A byte stream needs framing, so these are length-prefixed ([`write_frame`]/
//!   [`read_frame`], mirroring `ipc`'s codec — engine sits below the daemon so it doesn't pull the
//!   `ipc` crate).

// The wire vocab + codec are consumed by the network bridge threads in slice 3; until then only the
// unit tests exercise them, so a non-test build sees them as dead. Remove when slice 3 lands.
#![allow(dead_code)]

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use config::GlobalConfig;
use steam_hid::{ControllerState, Report};

use crate::program::{Program, Role};

use super::super::{Click, RumbleCmd};

/// Client→server over **UDP**: a raw controller snapshot. Latest-wins — `state.seq` (a `u32` from
/// the device) drops stale/out-of-order datagrams. The sink-side Mapper turns it into outputs
/// (report-side transmission — snapshots are idempotent and self-heal on loss; PLAN §6).
pub(super) type FramePacket = ControllerState;

/// Client→server over **TCP** (reliable, ordered): the config uplink + device lifecycle. The client
/// owns config and compiles `ConfigDoc→Program` before the wire (decision B), so the server never
/// compiles. `Report::State` never travels here — snapshots go over UDP as [`FramePacket`].
/// The wire protocol version — bumped on any incompatible change to the message vocab. The client
/// sends it first ([`Uplink::Hello`]); the server closes the connection on a mismatch.
pub(super) const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) enum Uplink {
    /// The handshake — always the first frame. The server validates `version` and drops the
    /// connection on a mismatch. Carries no config (config is ordinary `Apply`/`SetGlobals`).
    Hello { version: u16 },
    /// A keep-alive so the client detects a dead server over the otherwise-idle TCP link (`State`
    /// frames ride UDP). The server no-ops it.
    Ping,
    /// Apply a compiled program to a role (main↔fallback), or clear it (`program: None`), like the
    /// local `Control::Apply`.
    Apply { program: Option<Program>, role: Role },
    /// Replace the global config (master rumble, chords, ...).
    SetGlobals(GlobalConfig),
    /// A device lifecycle event — `Connected` / `Disconnected` / `Battery` (never `State`). Merged
    /// into the server's frame stream so the mapper (release-on-`Disconnected`) and the synthesized
    /// event surface see it.
    Event(Report),
}

/// Server→client over **UDP** (rare loss tolerable): the rumble/click back-channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) enum Downlink {
    /// Sustained rumble level — latest-wins, so a dropped datagram self-heals on the next update.
    Rumble(RumbleCmd),
    /// One-shot command-haptic click — fire-and-forget; a rare miss is imperceptible.
    Click(Click),
}

/// Upper bound on a single TCP frame (a fat `Program` is a few KB; slack + a guard against a bogus
/// length triggering a huge allocation). Mirrors `ipc::codec`.
const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Encode a message as a single UDP datagram payload (bare postcard, no length prefix).
pub(super) fn encode<T: Serialize>(msg: &T) -> postcard::Result<Vec<u8>> {
    postcard::to_stdvec(msg)
}

/// Decode a UDP datagram payload.
pub(super) fn decode<T: DeserializeOwned>(buf: &[u8]) -> postcard::Result<T> {
    postcard::from_bytes(buf)
}

/// Write `msg` to a TCP stream as one length-prefixed frame (4-byte LE length + postcard), flushing.
pub(super) fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let bytes = postcard::to_stdvec(msg).map_err(invalid_data)?;
    let len =
        u32::try_from(bytes.len()).map_err(|_| invalid_data("frame longer than u32::MAX"))?;
    if len > MAX_FRAME_LEN {
        return Err(invalid_data("frame exceeds MAX_FRAME_LEN"));
    }
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Read one length-prefixed frame from a TCP stream. `Ok(None)` on a **clean** EOF at a frame
/// boundary (peer closed) so a read loop can end gracefully.
pub(super) fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<Option<T>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(invalid_data("framed length exceeds MAX_FRAME_LEN"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    decode(&buf).map(Some).map_err(invalid_data)
}

fn invalid_data<E>(e: E) -> io::Error
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::InvalidData, e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{
        CompiledAction, CompiledBinding, CompiledCommand, CompiledLayer, CompiledSet, LayerId,
        ProgramMeta, SetId, SourceMap,
    };
    use config::{Activator, HapticStrength, InputSource, Side};

    /// A non-trivial program exercising nested IR serde (sets, layers, a bound command, ids).
    fn sample_program() -> Program {
        Program {
            meta: ProgramMeta { name: "Cyberpunk 2077".into(), role: Role::Main },
            default_set: SetId::new(0),
            rumble: Default::default(),
            sets: vec![CompiledSet {
                name: "Game".into(),
                base: SourceMap::from_iter([(
                    InputSource::LeftBumper,
                    CompiledBinding::Button {
                        commands: vec![CompiledCommand {
                            activator: Activator::Regular { interruptible: false },
                            actions: vec![CompiledAction::HoldLayer(LayerId::new(0))],
                            settings: Default::default(),
                        }],
                    },
                )]),
                layers: vec![CompiledLayer { name: "aim".into(), bindings: SourceMap::new() }],
            }],
        }
    }

    #[test]
    fn program_survives_the_tcp_frame_codec() {
        // The decisive slice-1/2 proof: a full compiled Program round-trips over the wire.
        let msg = Uplink::Apply { program: Some(sample_program()), role: Role::Main };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        let back: Option<Uplink> = read_frame(&mut buf.as_slice()).unwrap();
        assert_eq!(back, Some(msg));
    }

    #[test]
    fn clear_role_survives_the_tcp_frame_codec() {
        // A clear (`program: None`) round-trips too, so clearing a role propagates to the server.
        let msg = Uplink::Apply { program: None, role: Role::Main };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        assert_eq!(read_frame::<Uplink>(&mut buf.as_slice()).unwrap(), Some(msg));
    }

    #[test]
    fn uplink_globals_and_events_round_trip() {
        for msg in [
            Uplink::SetGlobals(GlobalConfig::default()),
            Uplink::Event(Report::Connected),
            Uplink::Event(Report::Disconnected),
        ] {
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).unwrap();
            assert_eq!(read_frame::<Uplink>(&mut buf.as_slice()).unwrap(), Some(msg));
        }
    }

    #[test]
    fn multiple_frames_decode_independently() {
        let msgs = [
            Uplink::Event(Report::Connected),
            Uplink::Apply { program: Some(sample_program()), role: Role::Fallback },
            Uplink::SetGlobals(GlobalConfig::default()),
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_frame(&mut buf, m).unwrap();
        }
        let mut r = buf.as_slice();
        for m in &msgs {
            assert_eq!(read_frame::<Uplink>(&mut r).unwrap().as_ref(), Some(m));
        }
        // Clean EOF at the final frame boundary → None (peer closed).
        assert_eq!(read_frame::<Uplink>(&mut r).unwrap(), None);
    }

    #[test]
    fn frame_and_downlink_round_trip_as_datagrams() {
        // UDP frame: a controller snapshot (bare postcard, no length prefix).
        let state = ControllerState { seq: 42, left_trigger: 0.5, ..Default::default() };
        let bytes = encode::<FramePacket>(&state).unwrap();
        assert_eq!(decode::<FramePacket>(&bytes).unwrap(), state);

        // UDP back-channel: rumble level + one-shot click.
        for msg in [
            Downlink::Rumble(RumbleCmd { strong: 30000, weak: 12000, hz: 80 }),
            Downlink::Click(Click { side: Side::Left, strength: HapticStrength::Medium }),
        ] {
            let bytes = encode(&msg).unwrap();
            assert_eq!(decode::<Downlink>(&bytes).unwrap(), msg);
        }
    }
}
