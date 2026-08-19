//! End-to-end client ↔ server over a real local socket (Unix). Binds a unique temp socket, runs a
//! tiny echo-ish server in a thread, and drives it through the `Client` API.

#![cfg(unix)]

use std::thread;

use ipc::{Client, ProfileRole, Request, Response, RunState, Server, StatusSnapshot};

#[test]
fn client_server_round_trip() {
    let path = std::env::temp_dir().join(format!("ipc-test-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path); // clear any stale socket from a crashed prior run

    // Bind before spawning so the client can't connect before the listener exists.
    let server = Server::bind_path(&path).expect("bind");
    let handle = thread::spawn(move || {
        for conn in server.incoming() {
            let mut conn = conn.expect("accept");
            while let Some(req) = conn.recv().expect("recv") {
                let resp = match req {
                    Request::Start => Response::Ok,
                    Request::Status => Response::Status(StatusSnapshot {
                        state: RunState::Running,
                        output: "local".into(),
                        input: "dongle".into(),
                        bound: None,
                        controller: Some(true),
                        battery: Some(72),
                        device_config: Default::default(),
                        main: Some("game".into()),
                        fallback: None,
                        active: Some(ProfileRole::Main),
                        chords: None,
                    }),
                    Request::ListDevices => Response::Devices(vec!["gordon:dongle:1:".into()]),
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
            assert_eq!(s.input, "dongle");
            assert_eq!(s.main.as_deref(), Some("game"));
            assert!(s.fallback.is_none());
        }
        other => panic!("expected Status, got {other:?}"),
    }

    match client.call(&Request::ListDevices).expect("list") {
        Response::Devices(d) => {
            assert_eq!(d.len(), 1);
            assert_eq!(d[0], "gordon:dongle:1:");
        }
        other => panic!("expected Devices, got {other:?}"),
    }

    assert!(matches!(client.call(&Request::Shutdown).expect("shutdown"), Response::Ok));

    drop(client);
    handle.join().expect("server thread");
    let _ = std::fs::remove_file(&path);
}
