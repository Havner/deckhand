//! End-to-end: spawn the real `deckhandd` binary on a temp control socket and drive it with the
//! `ipc` client. No hardware needed — exercises the socket serving, request/reply
//! dispatch, spec validation, and clean shutdown (Unix).

#![cfg(unix)]

use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use ipc::{Client, Request, Response, RunState};

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

    // Fresh daemon: idle, nothing loaded, defaults reported for input/output.
    match client.call(&Request::Status).expect("status") {
        Response::Status(s) => {
            assert_eq!(s.state, RunState::Idle);
            assert!(s.main.is_none() && s.fallback.is_none());
            assert_eq!(s.input, "auto");
            assert_eq!(s.output, "local");
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
        Response::Status(s) => assert_eq!(s.input, "dongle"),
        other => panic!("status: {other:?}"),
    }

    // Shutdown → Ok; the daemon then exits and removes its socket.
    assert!(matches!(client.call(&Request::Shutdown).unwrap(), Response::Ok));
    drop(client);

    let status = wait_child(&mut child, Duration::from_secs(5)).expect("daemon should exit");
    assert!(status.success(), "daemon exited with {status:?}");
    assert!(!sock.exists(), "socket file was not cleaned up");
}

/// Regression (PLAN §4.4, thread-per-connection): a client that connects and keeps its connection
/// **open and idle** — as the daemon-mode UI does with its persistent command connection — must not
/// wedge the accept loop. A second client has to be served promptly while the first still holds on.
#[test]
fn concurrent_clients_are_served() {
    let sock =
        std::env::temp_dir().join(format!("deckhandd-concurrent-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let mut child = Command::new(env!("CARGO_BIN_EXE_deckhandd"))
        .env("DECKHAND_SOCKET", &sock)
        .env("RUST_LOG", "warn")
        .spawn()
        .expect("spawn deckhandd");

    assert!(wait_for(|| sock.exists(), Duration::from_secs(5)), "daemon never bound the socket");

    // First client: connect, issue a request, then keep the connection open and idle.
    let mut held = Client::connect_path(&sock).expect("connect held");
    assert!(matches!(held.call(&Request::Status).unwrap(), Response::Status(_)));

    // Second client, on another thread: it must be served even though `held` is still open. Run it
    // off-thread with a timeout so a regression (the accept loop wedged) fails the test instead of
    // hanging it.
    let (tx, rx) = std::sync::mpsc::channel();
    let sock2 = sock.clone();
    std::thread::spawn(move || {
        let served = Client::connect_path(&sock2)
            .and_then(|mut c| c.call(&Request::Status))
            .map(|r| matches!(r, Response::Status(_)))
            .unwrap_or(false);
        let _ = tx.send(served);
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok(true),
        "second client was not served while the first held its connection open",
    );

    // Cleanup: the held connection can still drive shutdown.
    assert!(matches!(held.call(&Request::Shutdown).unwrap(), Response::Ok));
    drop(held);
    let status = wait_child(&mut child, Duration::from_secs(5)).expect("daemon should exit");
    assert!(status.success(), "daemon exited with {status:?}");
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
