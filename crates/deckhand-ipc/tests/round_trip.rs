//! End-to-end client ↔ server over a real local socket (Unix). Binds a unique temp socket, runs a
//! tiny echo-ish server in a thread, and drives it through the `Client` API.

#![cfg(unix)]

use std::thread;

use deckhand_ipc::{Client, DeviceEntry, Request, Response, RunState, Server, StatusInfo};

#[test]
fn client_server_round_trip() {
    let path = std::env::temp_dir().join(format!("deckhand-ipc-test-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path); // clear any stale socket from a crashed prior run

    // Bind before spawning so the client can't connect before the listener exists.
    let server = Server::bind_path(&path).expect("bind");
    let handle = thread::spawn(move || {
        for conn in server.incoming() {
            let mut conn = conn.expect("accept");
            while let Some(req) = conn.recv().expect("recv") {
                let resp = match req {
                    Request::Start => Response::Ok,
                    Request::Status => Response::Status(StatusInfo {
                        state: RunState::Running,
                        input: Some("dongle".into()),
                        output: Some("local".into()),
                        has_main: true,
                        has_fallback: false,
                    }),
                    Request::ListDevices => Response::Devices(vec![DeviceEntry {
                        id: "gordon:dongle:1:".into(),
                        kind: "Gordon".into(),
                        transport: "UsbDongle".into(),
                        interface: 1,
                    }]),
                    Request::Shutdown => {
                        conn.reply(&Response::Ok).expect("reply");
                        return; // end the server thread
                    }
                    other => Response::Error(format!("unhandled: {other:?}")),
                };
                conn.reply(&resp).expect("reply");
            }
        }
    });

    let mut client = Client::connect_path(&path).expect("connect");

    assert!(matches!(client.call(&Request::Start).expect("start"), Response::Ok));

    match client.call(&Request::Status).expect("status") {
        Response::Status(s) => {
            assert_eq!(s.state, RunState::Running);
            assert_eq!(s.input.as_deref(), Some("dongle"));
            assert!(s.has_main && !s.has_fallback);
        }
        other => panic!("expected Status, got {other:?}"),
    }

    match client.call(&Request::ListDevices).expect("list") {
        Response::Devices(d) => {
            assert_eq!(d.len(), 1);
            assert_eq!(d[0].id, "gordon:dongle:1:");
        }
        other => panic!("expected Devices, got {other:?}"),
    }

    assert!(matches!(client.call(&Request::Shutdown).expect("shutdown"), Response::Ok));

    drop(client);
    handle.join().expect("server thread");
    let _ = std::fs::remove_file(&path);
}
