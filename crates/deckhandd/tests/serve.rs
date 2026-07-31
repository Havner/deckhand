//! End-to-end: spawn the real `deckhandd` binary on a temp control socket and drive it with the
//! `deckhand-ipc` client. No hardware needed — exercises the socket serving, request/reply
//! dispatch, spec validation, and clean shutdown (Unix).

#![cfg(unix)]

use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use deckhand_ipc::{Client, Request, Response, RunState};

#[test]
fn daemon_serves_control_requests() {
    let sock = std::env::temp_dir().join(format!("deckhandd-test-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let mut child = Command::new(env!("CARGO_BIN_EXE_deckhandd"))
        .env("DECKHAND_SOCKET", &sock)
        .env("RUST_LOG", "warn")
        .spawn()
        .expect("spawn deckhandd");

    assert!(wait_for(|| sock.exists(), Duration::from_secs(5)), "daemon never bound the socket");

    let mut client = Client::connect_path(&sock).expect("connect");

    // Fresh daemon: idle, nothing loaded, nothing staged.
    match client.call(&Request::Status).expect("status") {
        Response::Status(s) => {
            assert_eq!(s.state, RunState::Idle);
            assert!(!s.has_main && !s.has_fallback);
            assert_eq!(s.input, None);
        }
        other => panic!("status: {other:?}"),
    }

    // A valid input spec is accepted; a bogus one is rejected (not fatal).
    assert!(matches!(client.call(&Request::SetInput("dongle".into())).unwrap(), Response::Ok));
    assert!(matches!(client.call(&Request::SetInput("bogus".into())).unwrap(), Response::Error(_)));

    // ListDevices returns Devices (contents depend on attached HW) — or an Error if HID init is
    // unavailable in this environment; either proves the request/reply path works.
    assert!(matches!(
        client.call(&Request::ListDevices).unwrap(),
        Response::Devices(_) | Response::Error(_)
    ));

    // The accepted input spec is now reflected in status.
    match client.call(&Request::Status).unwrap() {
        Response::Status(s) => assert_eq!(s.input.as_deref(), Some("dongle")),
        other => panic!("status: {other:?}"),
    }

    // Shutdown → Ok; the daemon then exits and removes its socket.
    assert!(matches!(client.call(&Request::Shutdown).unwrap(), Response::Ok));
    drop(client);

    let status = wait_child(&mut child, Duration::from_secs(5)).expect("daemon should exit");
    assert!(status.success(), "daemon exited with {status:?}");
    assert!(!sock.exists(), "socket file was not cleaned up");
}

fn wait_for(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn wait_child(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(s) = child.try_wait().expect("try_wait") {
            return Some(s);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
