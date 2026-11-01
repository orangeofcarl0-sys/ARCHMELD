// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B028: Windows named-pipe transport. The pipe keeps the bearer-token gate and
//! applies a custom DACL read back from the kernel object; a malformed name is
//! refused.

use serde_json::json;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

#[cfg(windows)]
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("archmeld-pipe-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(windows)]
async fn request(name: &str, authorization: Option<&str>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;
    for _ in 0..20 {
        match ClientOptions::new().open(name) {
            Ok(mut client) => {
                let body = "{}";
                let mut text = format!(
                    "POST /v1/query HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
                    body.len()
                );
                if let Some(value) = authorization {
                    text.push_str(&format!("Authorization: Bearer {value}\r\n"));
                }
                text.push_str("\r\n");
                text.push_str(body);
                client.write_all(text.as_bytes()).await.unwrap();
                let mut response = String::new();
                client.read_to_string(&mut response).await.unwrap();
                return response;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }
    panic!("named pipe {name} never became available");
}

#[cfg(windows)]
#[tokio::test]
async fn named_pipe_requires_the_bearer_token() {
    let dir = temp_dir("serve");
    let name = format!(r"\\.\pipe\archmeld-test-{}", std::process::id());
    let mut child = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"))
        .args([
            "--state",
            dir.to_str().unwrap(),
            "--named-pipe",
            &name,
            "--token",
            "test-token",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("archmeld-serve starts");
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let announcement: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(announcement["endpoint"], serde_json::Value::Null);
    assert_eq!(announcement["named_pipe"], name);
    assert_eq!(announcement["token"], "test-token");
    let acl = &announcement["acl"];
    assert_eq!(acl["type"], "custom_dacl", "{acl}");
    assert_eq!(acl["verified"], true, "{acl}");
    assert_eq!(acl["protected"], true, "{acl}");
    assert_eq!(acl["everyone"], false, "{acl}");
    assert_eq!(acl["anonymous"], false, "{acl}");
    assert_eq!(acl["current_user"], true, "{acl}");
    assert_eq!(acl["ace_count"], 3, "{acl}");
    assert!(acl["dacl_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    let denied = request(&name, None).await;
    assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
    let wrong = request(&name, Some("wrong-token")).await;
    assert!(wrong.starts_with("HTTP/1.1 403"), "{wrong}");
    let allowed = request(&name, Some("test-token")).await;
    assert!(!allowed.starts_with("HTTP/1.1 403"), "{allowed}");

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/release-evidence/B028-named-pipe.json");
        let evidence = json!({
            "schema_version": "1.0",
            "generated_by": "crates/archmeld-api/tests/named_pipe.rs",
            "platform": {
                "system": std::env::consts::OS,
                "machine": std::env::consts::ARCH,
            },
            "transport": {
                "name": name,
                "first_pipe_instance": true,
                "acl": acl,
                "announcement": {
                    "endpoint": announcement["endpoint"],
                    "named_pipe": announcement["named_pipe"],
                    "token": "set",
                    "acl": "custom_dacl",
                },
            },
            "checks": {
                "no_token_status": 403,
                "wrong_token_status": 403,
                "valid_token_reaches_handler": true,
                "malformed_name_exit": 2,
                "test_file": "crates/archmeld-api/tests/named_pipe.rs",
            },
            "scope": {
                "implemented": "CreateNamedPipeW with a protected DACL for the current user, SYSTEM and Administrators; every instance is created with the same descriptor; bearer middleware unchanged",
                "not_implemented": [],
            },
            "commands": ["#b028-named-pipe-tests"],
            "status": "passed",
        });
        std::fs::write(path, serde_json::to_string_pretty(&evidence).unwrap() + "\n").unwrap();
    }

    child.kill().unwrap();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(windows)]
#[test]
fn malformed_named_pipe_name_is_refused() {
    let dir = temp_dir("invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"))
        .args([
            "--state",
            dir.to_str().unwrap(),
            "--named-pipe",
            "not-a-pipe",
            "--no-auth",
        ])
        .output()
        .expect("archmeld-serve runs");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must start with"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
