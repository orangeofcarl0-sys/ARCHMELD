// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B028: Unix-domain socket serving with an owner-only socket and the same
//! bearer-token gate as the loopback TCP listener. The Unix test runs on Linux
//! and macOS; the Windows test asserts an explicit refusal instead of a silent
//! TCP fallback.

use std::process::{Command, Stdio};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "archmeld-unix-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(windows)]
#[test]
fn unix_socket_mode_is_refused_explicitly_on_windows() {
    let dir = temp_dir("refused");
    let output = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"))
        .args([
            "--state",
            dir.to_str().unwrap(),
            "--unix-socket",
            dir.join("archmeld.sock").to_str().unwrap(),
            "--no-auth",
        ])
        .output()
        .expect("archmeld-serve runs");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("supported on Unix only"), "{stderr}");
    assert!(!dir.join("archmeld.sock").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn unix_socket_is_owner_only_and_enforces_the_bearer_token() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;

    let dir = temp_dir("serve");
    let socket = dir.join("archmeld.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"))
        .args([
            "--state",
            dir.to_str().unwrap(),
            "--unix-socket",
            socket.to_str().unwrap(),
            "--token",
            "test-token",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("archmeld-serve starts");
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let announcement: serde_json::Value =
        serde_json::from_str(&lines.next().expect("announcement").unwrap()).unwrap();
    assert_eq!(announcement["endpoint"], serde_json::Value::Null);
    assert_eq!(announcement["unix_socket"], socket.to_str().unwrap());
    assert_eq!(announcement["socket_mode"], "0600");
    assert_eq!(announcement["token"], "test-token");
    let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "socket mode is {mode:o}");

    let request = |authorization: Option<&str>, body: Option<&str>| {
        let mut stream = UnixStream::connect(&socket).unwrap();
        let mut text = String::from("POST /v1/query HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
        if let Some(value) = authorization {
            text.push_str(&format!("Authorization: Bearer {value}\r\n"));
        }
        if let Some(body) = body {
            text.push_str("Content-Type: application/json\r\n");
            text.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        text.push_str("\r\n");
        if let Some(body) = body {
            text.push_str(body);
        }
        stream.write_all(text.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let denied = request(None, Some("{}"));
    assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
    let wrong = request(Some("wrong-token"), Some("{}"));
    assert!(wrong.starts_with("HTTP/1.1 403"), "{wrong}");
    let allowed = request(Some("test-token"), Some("{}"));
    assert!(!allowed.starts_with("HTTP/1.1 403"), "{allowed}");

    child.kill().unwrap();
    let _ = child.wait();
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&dir);
}
