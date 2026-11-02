// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Snapshot copies and OCI container execution shared by verification checks
//! and scenario runs.
//!
//! A materialized sandbox always comes from the ledger's content-addressed
//! blob store, never from the live workspace. The container runner only starts
//! a locally present, digest-pinned image with no network, a read-only root
//! filesystem and a writable `/tmp`; a timeout or cancellation removes the
//! container by name.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{validate, Store};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a caller may request for one sandboxed execution.
#[derive(Debug, Clone, Default)]
pub struct SandboxRequest {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub image: Option<String>,
    pub network: Option<String>,
    /// Path inside the snapshot copy, POSIX separators. Defaults to the root.
    pub cwd: Option<String>,
}

impl SandboxRequest {
    pub fn is_container(&self, mode: &str) -> bool {
        mode == "container"
    }

    /// Reject declarations that cannot be enforced instead of silently
    /// downgrading them.
    pub fn validate(&self, mode: &str) -> Result<()> {
        match mode {
            "workspace" => {
                if !self.include.is_empty() || !self.exclude.is_empty() {
                    return Err(CoreError::invalid(
                        "sandbox.mode=workspace does not accept include/exclude",
                    ));
                }
                if self.image.is_some() || self.network.is_some() {
                    return Err(CoreError::invalid(
                        "sandbox.mode=workspace does not accept image/network",
                    ));
                }
                if self.cwd.is_some() {
                    return Err(CoreError::invalid(
                        "sandbox.mode=workspace does not accept sandbox cwd",
                    ));
                }
            }
            "workspace_copy" | "container" => {
                if self.include.is_empty() {
                    return Err(CoreError::invalid(format!(
                        "sandbox.include must not be empty for {mode}"
                    )));
                }
                if mode == "workspace_copy" {
                    if self.image.is_some() || self.network.is_some() {
                        return Err(CoreError::invalid(
                            "sandbox.mode=workspace_copy does not accept image/network",
                        ));
                    }
                } else {
                    let image = self.image.as_deref().unwrap_or("");
                    if !image.contains("@sha256:") && !image.starts_with("sha256:") {
                        return Err(CoreError::invalid(
                            "container sandbox requires a digest-pinned image (name@sha256:... or sha256:...)",
                        ));
                    }
                    if let Some(network) = &self.network {
                        if network != "none" {
                            return Err(CoreError::invalid(
                                "container sandbox only supports network=none",
                            ));
                        }
                    }
                }
            }
            other => {
                return Err(CoreError::invalid(format!(
                    "unsupported sandbox mode {other}"
                )))
            }
        }
        if let Some(cwd) = &self.cwd {
            validate::validate_path("sandbox.cwd", cwd)?;
        }
        Ok(())
    }
}

pub struct Materialized {
    pub root: PathBuf,
    pub cwd: PathBuf,
    pub detail: Value,
}

/// Materialize the selected snapshot files under `state_root/<scope...>/`.
/// The scope path is caller-defined (for example `verify-sandboxes/<id>/<check>`)
/// and is always resolved inside the ledger state directory.
pub fn materialize(
    store: &mut Store,
    project_id: &str,
    snapshot_id: &str,
    scope: &[&str],
    request: &SandboxRequest,
) -> Result<Materialized> {
    if request.include.is_empty() {
        return Err(CoreError::invalid(
            "sandbox.include must not be empty for a snapshot copy",
        ));
    }
    materialize_with(
        store,
        project_id,
        snapshot_id,
        scope,
        |path| path_selected(path, &request.include, &request.exclude),
        &request.include,
        &request.exclude,
        request.cwd.as_deref(),
    )
}

/// Materialize every file of a snapshot. Used by the indexer so the parser
/// consumes exactly the bytes that were hashed, never a live-tree race.
pub fn materialize_all(
    store: &mut Store,
    project_id: &str,
    snapshot_id: &str,
    scope: &[&str],
) -> Result<Materialized> {
    materialize_with(
        store,
        project_id,
        snapshot_id,
        scope,
        |_| true,
        &[],
        &[],
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn materialize_with<F>(
    store: &mut Store,
    project_id: &str,
    snapshot_id: &str,
    scope: &[&str],
    selected: F,
    include: &[String],
    exclude: &[String],
    cwd_relative: Option<&str>,
) -> Result<Materialized>
where
    F: Fn(&str) -> bool,
{
    let payload = store
        .snapshot_payload(project_id, snapshot_id)?
        .ok_or_else(|| CoreError::not_found(format!("sandbox snapshot {snapshot_id} is missing")))?;
    let files = payload["files"]
        .as_array()
        .ok_or_else(|| CoreError::Storage("snapshot payload has no files".to_string()))?;
    let mut sandbox_root: PathBuf = store.state_root().to_path_buf();
    for component in scope {
        sandbox_root = sandbox_root.join(component);
    }
    if sandbox_root.exists() {
        fs::remove_dir_all(&sandbox_root)
            .map_err(|error| CoreError::Storage(format!("sandbox reset: {error}")))?;
    }
    fs::create_dir_all(&sandbox_root)
        .map_err(|error| CoreError::Storage(format!("sandbox create: {error}")))?;
    let mut entries: Vec<Value> = Vec::new();
    let mut selected_paths: Vec<String> = Vec::new();
    let mut total_bytes: u64 = 0;
    for file in files {
        let path = file["path"].as_str().unwrap_or("");
        if !selected(path) {
            continue;
        }
        if let Err(error) = validate::validate_path("snapshot file", path) {
            let _ = fs::remove_dir_all(&sandbox_root);
            return Err(error);
        }
        let digest = file["blob_digest"].as_str().ok_or_else(|| {
            let _ = fs::remove_dir_all(&sandbox_root);
            CoreError::Storage(format!("sandbox file {path} has no blob digest"))
        })?;
        let bytes = match store.read_blob_bytes(digest) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                let _ = fs::remove_dir_all(&sandbox_root);
                return Err(CoreError::not_found(format!(
                    "sandbox blob {digest} is missing from the CAS"
                )));
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&sandbox_root);
                return Err(error);
            }
        };
        let target = sandbox_root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                CoreError::Storage(format!("sandbox directory {path}: {error}"))
            })?;
        }
        fs::write(&target, &bytes)
            .map_err(|error| CoreError::Storage(format!("sandbox write {path}: {error}")))?;
        #[cfg(unix)]
        if file["executable"].as_bool().unwrap_or(false) {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o755));
        }
        selected_paths.push(path.to_string());
        total_bytes += bytes.len() as u64;
        entries.push(json!({
            "path": path,
            "digest": digest,
            "size": bytes.len(),
        }));
    }
    if entries.is_empty() {
        let _ = fs::remove_dir_all(&sandbox_root);
        return Err(CoreError::invalid(
            "snapshot materialization selected no files",
        ));
    }
    // An included prefix may name an empty directory; create it so a declared
    // cwd inside it resolves.
    for prefix in include {
        let has_files = selected_paths
            .iter()
            .any(|path| path == prefix || path.starts_with(&format!("{prefix}/")));
        if !has_files {
            let _ = fs::create_dir_all(sandbox_root.join(prefix));
        }
    }
    let file_count = entries.len();
    let manifest_digest = hash::digest_value(&json!({
        "version": "archmeld-sandbox-manifest-v1",
        "snapshot_id": snapshot_id,
        "files": entries,
    }))
    .map_err(|error| {
        let _ = fs::remove_dir_all(&sandbox_root);
        CoreError::Storage(format!("sandbox manifest: {error}"))
    })?;
    let cwd = match resolve_cwd(&sandbox_root, cwd_relative) {
        Ok(cwd) => cwd,
        Err(error) => {
            let _ = fs::remove_dir_all(&sandbox_root);
            return Err(error);
        }
    };
    Ok(Materialized {
        detail: json!({
            "source": "resulting_snapshot",
            "snapshot_id": snapshot_id,
            "root": sandbox_root.to_string_lossy(),
            "include": include,
            "exclude": exclude,
            "file_count": file_count,
            "bytes": total_bytes,
            "manifest_digest": manifest_digest,
        }),
        root: sandbox_root,
        cwd,
    })
}

pub fn path_selected(path: &str, include: &[String], exclude: &[String]) -> bool {
    let matches = |prefix: &str| {
        path == prefix
            || path
                .strip_prefix(prefix)
                .map(|rest| rest.starts_with('/'))
                .unwrap_or(false)
    };
    let included = include.is_empty() || include.iter().any(|prefix| matches(prefix));
    let excluded = exclude.iter().any(|prefix| matches(prefix));
    included && !excluded
}

pub fn resolve_cwd(root: &Path, relative: Option<&str>) -> Result<PathBuf> {
    let canonical_root = fs::canonicalize(root)?;
    let candidate = match relative {
        None | Some(".") | Some("") => canonical_root.clone(),
        Some(relative) => {
            validate::validate_path("sandbox.cwd", relative)?;
            canonical_root.join(relative)
        }
    };
    let canonical = fs::canonicalize(&candidate).map_err(|err| {
        CoreError::invalid(format!("sandbox cwd is missing or unreadable: {err}"))
    })?;
    if !canonical.starts_with(&canonical_root) {
        return Err(CoreError::invalid("sandbox cwd escapes the snapshot copy"));
    }
    if !canonical.is_dir() {
        return Err(CoreError::invalid("sandbox cwd is not a directory"));
    }
    Ok(canonical)
}

#[derive(Debug, Clone)]
pub struct ImageIdentity {
    pub image: String,
    pub image_id: String,
    pub repo_digests: Vec<String>,
    pub platform: String,
}

pub fn inspect_image(image: &str, cwd: &Path) -> Result<ImageIdentity> {
    let capture = crate::scenario::run_command_capture(
        &[
            "docker".to_string(),
            "image".to_string(),
            "inspect".to_string(),
            image.to_string(),
            "--format".to_string(),
            "{{.Id}}|{{json .RepoDigests}}|{{.Os}}/{{.Architecture}}".to_string(),
        ],
        cwd,
        &[],
        Duration::from_secs(30),
        None,
    )?;
    if capture.timed_out {
        return Err(CoreError::Storage(
            "docker image inspect timed out".to_string(),
        ));
    }
    if capture.exit_code != Some(0) {
        return Err(CoreError::not_found(format!(
            "container image {image} is not available locally: {}",
            String::from_utf8_lossy(&capture.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&capture.stdout);
    let line = text
        .lines()
        .next()
        .ok_or_else(|| CoreError::Storage("docker image inspect produced no output".to_string()))?;
    let parts: Vec<&str> = line.split('|').collect();
    if parts.len() != 3 {
        return Err(CoreError::Storage(
            "unexpected docker image inspect output".to_string(),
        ));
    }
    let image_id = parts[0].to_string();
    let digests: Vec<String> = serde_json::from_str(parts[1])
        .map_err(|error| CoreError::Storage(format!("docker image digests: {error}")))?;
    let platform = parts[2].to_string();
    if !platform.starts_with("linux/") {
        return Err(CoreError::Unsupported(format!(
            "container image platform is {platform}; only linux/* is supported"
        )));
    }
    if let Some(digest) = image.strip_prefix("sha256:") {
        if !image_id.eq_ignore_ascii_case(&format!("sha256:{digest}")) {
            return Err(CoreError::conflict(
                "container image id does not match the pinned sha256".to_string(),
            ));
        }
    } else if let Some((_, digest)) = image.split_once('@') {
        if !digests.iter().any(|value| value.ends_with(digest)) {
            return Err(CoreError::conflict(format!(
                "container image {image} is not pinned to the local image digest"
            )));
        }
    } else {
        return Err(CoreError::invalid(
            "container image must be pinned by digest".to_string(),
        ));
    }
    Ok(ImageIdentity {
        image: image.to_string(),
        image_id,
        repo_digests: digests,
        platform,
    })
}

pub struct ContainerRun {
    pub name: String,
    pub image: String,
    pub image_id: String,
    pub repo_digests: Vec<String>,
    pub platform: String,
    pub mount_root: PathBuf,
    pub workdir: String,
}

impl ContainerRun {
    pub fn exists(&self) -> bool {
        crate::scenario::run_command_capture(
            &[
                "docker".to_string(),
                "inspect".to_string(),
                self.name.clone(),
            ],
            &self.mount_root,
            &[],
            Duration::from_secs(15),
            None,
        )
        .map(|capture| capture.exit_code == Some(0))
        .unwrap_or(false)
    }

    pub fn cleanup(&self) {
        let _ = crate::scenario::run_command_capture(
            &[
                "docker".to_string(),
                "rm".to_string(),
                "-f".to_string(),
                self.name.clone(),
            ],
            &self.mount_root,
            &[],
            Duration::from_secs(30),
            None,
        );
    }

    /// True when the container is gone; a surviving container is removed.
    pub fn removed(&self) -> bool {
        if self.exists() {
            self.cleanup();
            !self.exists()
        } else {
            true
        }
    }

    pub fn detail(&self, removed: bool) -> Value {
        json!({
            "name": self.name,
            "image": self.image,
            "image_id": self.image_id,
            "repo_digests": self.repo_digests,
            "platform": self.platform,
            "workdir": self.workdir,
            "removed": removed,
        })
    }
}

/// Materialize the snapshot copy and prepare the `docker run` invocation.
/// Returns the sandbox and the executable argv; the caller still owns running
/// and cleaning up the container.
pub fn prepare_container(
    store: &mut Store,
    project_id: &str,
    snapshot_id: &str,
    scope: &[&str],
    request: &SandboxRequest,
    env: &[(String, String)],
    command: &[String],
) -> Result<(Materialized, ContainerRun, Vec<String>)> {
    let materialized = materialize(store, project_id, snapshot_id, scope, request)?;
    let image = request
        .image
        .clone()
        .ok_or_else(|| CoreError::invalid("container sandbox requires a pinned image"))?;
    let identity = inspect_image(&image, &materialized.root)?;
    let canonical_root = fs::canonicalize(&materialized.root)
        .map_err(|error| CoreError::Storage(format!("container sandbox root: {error}")))?;
    let relative = materialized
        .cwd
        .strip_prefix(&canonical_root)
        .map_err(|_| CoreError::invalid("container cwd is outside the snapshot copy"))?;
    let relative_text = relative.to_string_lossy().replace('\\', "/");
    let workdir = if relative_text.is_empty() {
        "/workspace".to_string()
    } else {
        format!("/workspace/{relative_text}")
    };
    let name = format!(
        "archmeld-{}-{}",
        &short_hash(&format!("{project_id}/{snapshot_id}/{}", scope.join("/")))[..16],
        {
            let raw = sanitize_id(scope.last().copied().unwrap_or("run"));
            raw.chars().take(40).collect::<String>()
        }
    );
    let container = ContainerRun {
        name: name.clone(),
        image: identity.image.clone(),
        image_id: identity.image_id.clone(),
        repo_digests: identity.repo_digests.clone(),
        platform: identity.platform.clone(),
        mount_root: materialized.root.clone(),
        workdir: workdir.clone(),
    };
    let argv = container_argv(&container, &materialized.root, env, command);
    Ok((materialized, container, argv))
}

/// Build the `docker run` argv in front of the caller's command.
pub fn container_argv(
    container: &ContainerRun,
    mount_root: &Path,
    env: &[(String, String)],
    command: &[String],
) -> Vec<String> {
    let mut exec_argv = vec![
        "docker".to_string(),
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        container.name.clone(),
        "--network".to_string(),
        "none".to_string(),
        "--read-only".to_string(),
        "--tmpfs".to_string(),
        "/tmp:rw,size=64m".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
    ];
    for (name, value) in env {
        exec_argv.push("-e".to_string());
        exec_argv.push(format!("{name}={value}"));
    }
    exec_argv.extend([
        "-v".to_string(),
        format!("{}:/workspace", mount_root.display()),
        "--workdir".to_string(),
        container.workdir.clone(),
        "--pull".to_string(),
        "never".to_string(),
        container.image.clone(),
    ]);
    exec_argv.extend(command.iter().cloned());
    exec_argv
}

/// Copy of a `docker run` argv with container environment values removed.
/// The ledger records env names elsewhere; values must not leak into evidence.
pub fn redact_exec_argv(argv: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(argv.len());
    let mut redact_next = false;
    for argument in argv {
        if redact_next {
            let name = argument.split_once('=').map(|(name, _)| name).unwrap_or(argument);
            redacted.push(format!("{name}=<redacted>"));
            redact_next = false;
            continue;
        }
        if argument == "-e" || argument == "--env" {
            redact_next = true;
        }
        redacted.push(argument.clone());
    }
    redacted
}

pub fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn short_hash(identity: &str) -> String {
    let digest = hash::digest_bytes(identity.as_bytes());
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    hex[..32].to_string()
}
