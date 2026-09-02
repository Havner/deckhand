//! `ipc` - the control-plane wire protocol + local-socket client/server shared by the
//! daemon (`deckhandd`) and its clients (`deckhandctl`, the daemon-mode UI). PLAN 4.4.
//!
//! Depends on `config` (messages carry `ConfigDoc`), **never** on `engine` - the daemon owns the
//! engine, clients stay thin. Sync, no async. Transport is `interprocess` local sockets (Unix
//! domain sockets / Windows named pipes) behind [`Client`]/[`Server`]; the framing ([`write_msg`]/
//! [`read_msg`]) is a plain length-prefixed `postcard` codec over any `Read`/`Write`.

mod codec;
mod message;
mod net;

pub use codec::{read_msg, write_msg};
pub use message::{BoundDevice, Event, ProfileRole, Request, Response, RunState, StatusSnapshot};
pub use net::{Client, Conn, Server};
#[cfg(unix)]
pub use net::default_socket_path;
#[cfg(windows)]
pub use net::DEFAULT_PIPE_NAME;
