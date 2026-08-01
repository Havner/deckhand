//! The local-socket transport (PLAN §4.4): a thin [`Client`] and [`Server`] over `interprocess`
//! local sockets — Unix domain sockets on Unix, named pipes on Windows. The *only* platform-
//! specific bit is the socket **name**; everything above it (framing, messages) is shared.

use std::io;
#[cfg(unix)]
use std::path::{Path, PathBuf};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, Listener, ListenerOptions, Stream};
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;

use crate::codec::{read_msg, write_msg};
use crate::{Event, Request, Response};

/// Default control-socket path on Unix. `$DECKHAND_SOCKET` overrides it outright (handy for tests
/// / non-default layouts); otherwise `$XDG_RUNTIME_DIR/deckhand.sock` (fallback `/tmp` when the
/// runtime dir is unset — e.g. outside a login session). Shared by the daemon (bind) and clients
/// (connect) so they always agree.
#[cfg(unix)]
pub fn default_socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DECKHAND_SOCKET") {
        return PathBuf::from(p);
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("deckhand.sock")
}

/// Default control-pipe name on Windows (namespaced → `\\.\pipe\deckhand.sock`).
#[cfg(windows)]
pub const DEFAULT_PIPE_NAME: &str = "deckhand.sock";

/// A connected client end of the control socket.
pub struct Client {
    stream: Stream,
}

impl Client {
    /// Connect to the daemon at the platform-default control socket.
    pub fn connect_default() -> io::Result<Self> {
        #[cfg(unix)]
        {
            Self::connect_path(&default_socket_path())
        }
        #[cfg(windows)]
        {
            let name = DEFAULT_PIPE_NAME.to_ns_name::<GenericNamespaced>()?;
            Ok(Client { stream: Stream::connect(name)? })
        }
    }

    /// Connect to a control socket at an explicit filesystem path (Unix).
    #[cfg(unix)]
    pub fn connect_path(path: &Path) -> io::Result<Self> {
        let name = path.to_fs_name::<GenericFilePath>()?;
        Ok(Client { stream: Stream::connect(name)? })
    }

    /// Send a request and read the single reply.
    pub fn call(&mut self, req: &Request) -> io::Result<Response> {
        write_msg(&mut self.stream, req)?;
        read_msg(&mut self.stream)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "daemon closed the connection")
        })
    }

    /// Send a [`Request::Subscribe`] and switch this connection to the event stream: the daemon
    /// then pushes [`Event`]s, read with [`next_event`](Client::next_event). No reply is sent.
    pub fn subscribe(&mut self) -> io::Result<()> {
        write_msg(&mut self.stream, &Request::Subscribe)
    }

    /// Read the next pushed event (after [`subscribe`](Client::subscribe)); `Ok(None)` on a clean
    /// close.
    pub fn next_event(&mut self) -> io::Result<Option<Event>> {
        read_msg(&mut self.stream)
    }
}

/// A bound control-socket listener.
pub struct Server {
    listener: Listener,
}

impl Server {
    /// Bind the platform-default control socket.
    pub fn bind_default() -> io::Result<Self> {
        #[cfg(unix)]
        {
            Self::bind_path(&default_socket_path())
        }
        #[cfg(windows)]
        {
            let name = DEFAULT_PIPE_NAME.to_ns_name::<GenericNamespaced>()?;
            Ok(Server { listener: ListenerOptions::new().name(name).create_sync()? })
        }
    }

    /// Bind a control socket at an explicit filesystem path (Unix). The caller owns the
    /// stale-socket / single-instance policy (PLAN §4.4 — that dance lives in the daemon).
    #[cfg(unix)]
    pub fn bind_path(path: &Path) -> io::Result<Self> {
        let name = path.to_fs_name::<GenericFilePath>()?;
        Ok(Server { listener: ListenerOptions::new().name(name).create_sync()? })
    }

    /// Iterate accepted client connections. Each item is one [`Conn`].
    pub fn incoming(&self) -> impl Iterator<Item = io::Result<Conn>> + '_ {
        self.listener.incoming().map(|r| r.map(|stream| Conn { stream }))
    }
}

/// One accepted client connection on the server side. Read [`Request`]s and write [`Response`]s /
/// [`Event`]s over it.
pub struct Conn {
    stream: Stream,
}

impl Conn {
    /// Read the next request; `Ok(None)` when the client closed the connection.
    pub fn recv(&mut self) -> io::Result<Option<Request>> {
        read_msg(&mut self.stream)
    }

    /// Write a reply.
    pub fn reply(&mut self, resp: &Response) -> io::Result<()> {
        write_msg(&mut self.stream, resp)
    }

    /// Push an event (to a subscribed connection).
    pub fn send_event(&mut self, ev: &Event) -> io::Result<()> {
        write_msg(&mut self.stream, ev)
    }
}
