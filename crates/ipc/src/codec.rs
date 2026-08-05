//! Length-prefixed `postcard` framing over any byte stream (PLAN §4.4). Each message is a 4-byte
//! little-endian length followed by that many `postcard` bytes. Sync; works over the local socket
//! or any `Read`/`Write` (so it is unit-testable without a socket).

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Upper bound on a single framed message (a fat `ConfigDoc` is a few KB; this is slack + a guard
/// against a bogus length triggering a huge allocation).
const MAX_MSG_LEN: u32 = 16 * 1024 * 1024;

/// Serialize `msg` and write it as one length-prefixed frame, flushing.
pub fn write_msg<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let bytes = postcard::to_stdvec(msg).map_err(invalid_data)?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| invalid_data("message longer than u32::MAX"))?;
    if len > MAX_MSG_LEN {
        return Err(invalid_data("message exceeds MAX_MSG_LEN"));
    }
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Read one length-prefixed frame and deserialize it. Returns `Ok(None)` on a **clean** EOF at a
/// frame boundary (the peer closed the connection), so a read loop can end gracefully.
pub fn read_msg<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<Option<T>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_MSG_LEN {
        return Err(invalid_data("framed length exceeds MAX_MSG_LEN"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    let msg = postcard::from_bytes(&buf).map_err(invalid_data)?;
    Ok(Some(msg))
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
    use crate::{ProfileRole, Request, Response, RunState, StatusSnapshot};

    #[test]
    fn round_trips_over_a_buffer() {
        let reqs = vec![
            Request::Start,
            Request::SetInput("dongle".into()),
            Request::Status,
            Request::Subscribe,
        ];
        let mut buf = Vec::new();
        for r in &reqs {
            write_msg(&mut buf, r).unwrap();
        }
        // A trailing status reply, to prove mixed types frame independently.
        write_msg(
            &mut buf,
            &Response::Status(StatusSnapshot {
                state: RunState::Idle,
                input: "dongle".into(),
                output: "local".into(),
                main: None,
                fallback: None,
                bound: None,
                globals: Default::default(),
            }),
        )
        .unwrap();

        let mut cur = std::io::Cursor::new(buf);
        for expected in &reqs {
            let got: Request = read_msg(&mut cur).unwrap().unwrap();
            assert_eq!(format!("{got:?}"), format!("{expected:?}"));
        }
        let reply: Response = read_msg(&mut cur).unwrap().unwrap();
        assert!(matches!(reply, Response::Status(s) if s.input == "dongle"));
        // Clean EOF at a frame boundary → None.
        assert!(read_msg::<Request>(&mut cur).unwrap().is_none());
    }

    #[test]
    fn profile_role_is_carried() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &ProfileRole::Fallback).unwrap();
        let mut cur = std::io::Cursor::new(buf);
        let got: ProfileRole = read_msg(&mut cur).unwrap().unwrap();
        assert_eq!(got, ProfileRole::Fallback);
    }
}
