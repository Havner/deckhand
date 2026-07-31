//! End-to-end: run the real `deckhandctl` binary against an in-process fake daemon (a
//! `deckhand-ipc::Server`), checking that each subcommand sends the right request and renders the
//! reply. No real daemon / hardware needed (Unix).

#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};
use std::thread;

use deckhand_ipc::{Request, Response, RunState, Server, StatusInfo};

#[test]
fn ctl_drives_a_fake_daemon() {
    let sock = std::env::temp_dir().join(format!("deckhandctl-test-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    // Fake daemon: one request per client connection (deckhandctl is one-shot).
    let server = Server::bind_path(&sock).expect("bind");
    let handle = thread::spawn(move || {
        for conn in server.incoming() {
            let mut conn = conn.expect("accept");
            if let Some(req) = conn.recv().expect("recv") {
                let resp = match req {
                    Request::Status => Response::Status(StatusInfo {
                        state: RunState::Running,
                        input: Some("dongle".into()),
                        output: Some("local".into()),
                        has_main: true,
                        has_fallback: false,
                    }),
                    Request::SetInput(spec) if spec == "gordon:dongle:1:" => Response::Ok,
                    Request::SetInput(_) => Response::Error("bad spec".into()),
                    Request::Shutdown => {
                        conn.reply(&Response::Ok).expect("reply");
                        return; // stop the fake daemon
                    }
                    other => Response::Error(format!("unhandled: {other:?}")),
                };
                conn.reply(&resp).expect("reply");
            }
        }
    });

    // `status` renders the StatusInfo fields.
    let out = run_ctl(&sock, &["status"]);
    assert!(out.status.success(), "status exit: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Running"), "status stdout: {stdout}");
    assert!(stdout.contains("dongle"), "status stdout: {stdout}");

    // `select-device <id>` → SetInput → ok.
    let out = run_ctl(&sock, &["select-device", "gordon:dongle:1:"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));

    // A rejected spec → non-zero exit, error on stderr.
    let out = run_ctl(&sock, &["select-device", "nope"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("bad spec"));

    // `shutdown` → ok, and the fake daemon returns.
    let out = run_ctl(&sock, &["shutdown"]);
    assert!(out.status.success());

    handle.join().expect("server thread");
    let _ = std::fs::remove_file(&sock);
}

fn run_ctl(sock: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deckhandctl"))
        .env("DECKHAND_SOCKET", sock)
        .args(args)
        .output()
        .expect("run deckhandctl")
}
