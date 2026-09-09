#![cfg(unix)]

use crate::extras::status_signals::{BlockedReason, RunState, StatusSignals};
use std::io::Read;
use std::os::unix::net::UnixListener;

fn temp_socket_path(name: &str) -> (std::path::PathBuf, UnixListener) {
    let dir = std::env::temp_dir().join(format!("zs_status_test_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket_path = dir.join("status.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    (socket_path, listener)
}

fn cleanup(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

#[test]
fn send_start_writes_expected_message() {
    let (socket_path, listener) = temp_socket_path("start");
    let ss = StatusSignals::new(socket_path.to_string_lossy().to_string());
    ss.send_start();

    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "start\n");
    cleanup(&socket_path);
}

#[test]
fn send_stop_writes_expected_message() {
    let (socket_path, listener) = temp_socket_path("stop");
    let ss = StatusSignals::new(socket_path.to_string_lossy().to_string());
    ss.send_stop();

    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "stop\n");
    cleanup(&socket_path);
}

#[test]
fn nonexistent_socket_does_not_panic() {
    let ss = StatusSignals::new("/tmp/definitely_nonexistent_status_socket_12345".to_string());
    ss.send_start();
    ss.send_stop();
    ss.send_git_conflict();
    ss.send_blocked(BlockedReason::Permission);
    ss.send_state(RunState::Working);
    let guard = ss.blocked_scope(BlockedReason::Permission);
    drop(guard);
}

#[test]
fn send_git_conflict_writes_expected_message() {
    let (socket_path, listener) = temp_socket_path("conflict");
    let ss = StatusSignals::new(socket_path.to_string_lossy().to_string());
    ss.send_git_conflict();

    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "git-conflict\n");
    cleanup(&socket_path);
}

#[test]
fn send_blocked_writes_expected_message() {
    let (socket_path, listener) = temp_socket_path("blocked");
    let ss = StatusSignals::new(socket_path.to_string_lossy().to_string());
    ss.send_blocked(BlockedReason::Permission);

    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "blocked:permission\n");
    cleanup(&socket_path);
}

#[test]
fn send_state_writes_expected_message() {
    let (socket_path, listener) = temp_socket_path("state");
    let ss = StatusSignals::new(socket_path.to_string_lossy().to_string());
    ss.send_state(RunState::Working);

    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "state:working\n");
    cleanup(&socket_path);
}

#[test]
fn blocked_scope_sends_pair_in_order() {
    let (socket_path, listener) = temp_socket_path("scope_order");
    let ss = StatusSignals::new(socket_path.to_string_lossy().to_string());

    let guard = ss.blocked_scope(BlockedReason::Permission);
    drop(guard);

    let mut received = Vec::new();
    for _ in 0..2 {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = String::new();
        stream.read_to_string(&mut buf).unwrap();
        received.push(buf.trim_end().to_string());
    }
    assert_eq!(received, vec!["blocked:permission", "state:working"]);
    cleanup(&socket_path);
}

#[test]
fn blocked_scope_releases_on_error_path() {
    let (socket_path, listener) = temp_socket_path("scope_error");
    let ss = StatusSignals::new(socket_path.to_string_lossy().to_string());

    let result: Result<(), ()> = (|| {
        let _guard = ss.blocked_scope(BlockedReason::Permission);
        Err(())?;
        Ok(())
    })();
    assert_eq!(result, Err(()));

    let mut received = Vec::new();
    for _ in 0..2 {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = String::new();
        stream.read_to_string(&mut buf).unwrap();
        received.push(buf.trim_end().to_string());
    }
    assert_eq!(received, vec!["blocked:permission", "state:working"]);
    cleanup(&socket_path);
}
