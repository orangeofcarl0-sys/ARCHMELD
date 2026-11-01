// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `archmeld-serve`: local API entry point. Prints one JSON line with the
//! bound endpoint and token, then serves until killed.

use archmeld_api::{generate_token, router, ApiState};
use archmeld_core::store::Store;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
mod pipe_security;

#[tokio::main]
async fn main() {
    let mut state_dir = None;
    let mut port: u16 = 0;
    let mut token = None;
    let mut no_auth = false;
    let mut unix_socket: Option<PathBuf> = None;
    let mut named_pipe: Option<String> = None;
    let mut allow_workspaces: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state" => state_dir = args.next(),
            "--port" => port = args.next().and_then(|value| value.parse().ok()).unwrap_or(0),
            "--token" => token = args.next(),
            "--unix-socket" => unix_socket = args.next().map(PathBuf::from),
            "--named-pipe" => named_pipe = args.next(),
            "--no-auth" => no_auth = true,
            "--allow-workspace" => {
                if let Some(path) = args.next() {
                    allow_workspaces.push(PathBuf::from(path));
                }
            }
            "--help" | "-h" => {
                println!(
                    "usage: archmeld-serve --state <dir> [--port N | --unix-socket <path> | --named-pipe <name>] [--allow-workspace <path>]... [--token T | --no-auth]"
                );
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(state_dir) = state_dir else {
        eprintln!("--state <dir> is required");
        std::process::exit(2);
    };
    let state_root = match std::fs::canonicalize(&state_dir) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("cannot resolve state dir {state_dir}: {error}");
            std::process::exit(1);
        }
    };
    let mut workspaces: HashMap<String, PathBuf> = HashMap::new();
    for workspace in &allow_workspaces {
        let canonical = match std::fs::canonicalize(workspace) {
            Ok(path) if path.is_dir() => path,
            _ => {
                eprintln!("workspace {} is not an accessible directory", workspace.display());
                std::process::exit(1);
            }
        };
        if inside(&state_root, &canonical) {
            eprintln!("refusing state dir inside workspace {}: {}", canonical.display(), state_root.display());
            std::process::exit(1);
        }
        let digest = archmeld_core::hash::digest_bytes(canonical.to_string_lossy().as_bytes());
        let handle = format!("ws.{}", &digest.trim_start_matches("sha256:")[..16]);
        workspaces.insert(handle, canonical);
    }
    let store = match Store::open(&state_dir) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("cannot open ledger: {error}");
            std::process::exit(1);
        }
    };
    let token = if no_auth {
        None
    } else {
        Some(token.unwrap_or_else(generate_token))
    };
    let state = ApiState {
        store: Arc::new(Mutex::new(store)),
        token: token.clone(),
        workspaces: workspaces.clone(),
        state_dir: state_root.clone(),
    };
    let app = router(state);
    if let Some(name) = named_pipe {
        #[cfg(windows)]
        {
            serve_named_pipe(name, token, workspaces, app).await;
            return;
        }
        #[cfg(not(windows))]
        {
            let _ = (name, token, workspaces, app);
            eprintln!("--named-pipe is supported on Windows only; use --unix-socket or --port");
            std::process::exit(2);
        }
    }
    if let Some(path) = unix_socket {
        #[cfg(unix)]
        {
            serve_unix(path, token, workspaces, app).await;
            return;
        }
        #[cfg(not(unix))]
        {
            let _ = (path, token, workspaces, app);
            eprintln!("--unix-socket is supported on Unix only; use --port on this platform");
            std::process::exit(2);
        }
    }
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("cannot bind: {error}");
            std::process::exit(1);
        }
    };
    let address = listener.local_addr().expect("bound address");
    let announcement = serde_json::json!({
        "endpoint": format!("http://{address}"),
        "token": token,
        "contract_version": archmeld_api::CONTRACT_VERSION,
        "workspaces": workspaces,
    });
    println!("{announcement}");
    let _ = std::io::stdout().flush();
    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("server error: {error}");
        std::process::exit(1);
    }
}

#[cfg(unix)]
async fn serve_unix(
    path: PathBuf,
    token: Option<String>,
    workspaces: HashMap<String, PathBuf>,
    app: axum::Router,
) {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        eprintln!(
            "refusing to replace existing socket {}; remove it first",
            path.display()
        );
        std::process::exit(1);
    }
    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("cannot bind unix socket {}: {error}", path.display());
            std::process::exit(1);
        }
    };
    if let Err(error) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!("cannot set socket mode 0600: {error}");
        let _ = std::fs::remove_file(&path);
        std::process::exit(1);
    }
    let mode = std::fs::metadata(&path)
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .unwrap_or(0);
    if mode != 0o600 {
        eprintln!("unix socket mode is {mode:o}, expected 600");
        let _ = std::fs::remove_file(&path);
        std::process::exit(1);
    }
    let announcement = serde_json::json!({
        "endpoint": null,
        "unix_socket": path,
        "socket_mode": "0600",
        "token": token,
        "contract_version": archmeld_api::CONTRACT_VERSION,
        "workspaces": workspaces,
    });
    println!("{announcement}");
    let _ = std::io::stdout().flush();
    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("server error: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
struct NamedPipeListener {
    name: String,
    security: pipe_security::PipeSecurity,
    current: tokio::net::windows::named_pipe::NamedPipeServer,
}

#[cfg(windows)]
impl NamedPipeListener {
    fn bind(name: &str) -> std::io::Result<Self> {
        let security = pipe_security::PipeSecurity::for_current_user()?;
        let current = pipe_security::create(name, &security, true)?;
        Ok(Self {
            name: name.to_string(),
            security,
            current,
        })
    }
}

#[cfg(windows)]
impl axum::serve::Listener for NamedPipeListener {
    type Io = tokio::net::windows::named_pipe::NamedPipeServer;
    type Addr = ();

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.current.connect().await {
                Ok(()) => break,
                Err(_) => {
                    self.current = pipe_security::create(&self.name, &self.security, false)
                        .expect("create the next named pipe instance");
                }
            }
        }
        let connected = std::mem::replace(
            &mut self.current,
            pipe_security::create(&self.name, &self.security, false)
                .expect("create the next named pipe instance"),
        );
        (connected, ())
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(())
    }
}

#[cfg(windows)]
async fn serve_named_pipe(
    name: String,
    token: Option<String>,
    workspaces: HashMap<String, PathBuf>,
    app: axum::Router,
) {
    if !name.starts_with(r"\\.\pipe\") {
        eprintln!("named pipe name must start with \\\\.\\pipe\\, got {name}");
        std::process::exit(2);
    }
    let listener = match NamedPipeListener::bind(&name) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("cannot create named pipe {name}: {error}");
            std::process::exit(1);
        }
    };
    let acl = match {
        use std::os::windows::io::AsRawHandle;
        pipe_security::describe(listener.current.as_raw_handle(), &listener.security)
    } {
        Ok(value) => value,
        Err(error) => serde_json::json!({"type": "custom_dacl", "verified": false, "error": error.to_string()}),
    };
    let announcement = serde_json::json!({
        "endpoint": null,
        "named_pipe": name,
        "token": token,
        "contract_version": archmeld_api::CONTRACT_VERSION,
        "workspaces": workspaces,
        "acl": acl,
    });
    println!("{announcement}");
    let _ = std::io::stdout().flush();
    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("server error: {error}");
        std::process::exit(1);
    }
}

fn inside(child: &std::path::Path, parent: &std::path::Path) -> bool {
    let mut child = child.to_string_lossy().replace('\\', "/").to_lowercase();
    let mut parent = parent.to_string_lossy().replace('\\', "/").to_lowercase();
    while child.ends_with('/') {
        child.pop();
    }
    while parent.ends_with('/') {
        parent.pop();
    }
    child == parent || child.starts_with(&format!("{parent}/"))
}
