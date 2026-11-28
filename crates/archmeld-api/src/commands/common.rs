// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `common` CLI commands.

use super::*;

pub(crate) fn passphrase(variable: &str) -> Result<String, String> {
    let value = std::env::var(variable)
        .map_err(|_| format!("passphrase environment variable {variable} is not set"))?;
    if value.is_empty() {
        return Err(format!(
            "passphrase environment variable {variable} is empty"
        ));
    }
    Ok(value)
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) fn helper_path(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Ok(path) = std::env::var("ARCHMELD_INDEX_HELPER") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current executable has no parent directory".to_string())?;
    let candidates = if cfg!(windows) {
        vec![
            dir.join("archmeld-index.exe"),
            dir.join("archmeld_export.exe"),
        ]
    } else {
        vec![dir.join("archmeld-index"), dir.join("archmeld_export")]
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            "archmeld-index helper not found next to archmeld; pass --index-helper".to_string()
        })
}

pub(crate) struct HelperOutput {
    pub(crate) success: bool,
    pub(crate) status: Option<i32>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) type HelperRunner<'a> = dyn FnMut(
        &PathBuf,
        &std::path::Path,
        &std::path::Path,
        &std::path::Path,
    ) -> Result<HelperOutput, String>
    + 'a;

pub(crate) fn helper_exit_hint(status: Option<i32>) -> String {
    match status {
        Some(-1073741511) => {
            "0xC0000139 ENTRYPOINT_NOT_FOUND: the helper is missing a MinGW runtime DLL; add its bin directory to PATH (ARCHMELD_MINGW_BIN, alias ARCHMELD_STRAWBERRY_BIN) or rebuild it statically".to_string()
        }
        Some(-1073741515) => {
            "0xC0000135 DLL_NOT_FOUND: a helper dependency is missing from PATH".to_string()
        }
        Some(code) if code < 0 => format!("0x{:08X}", code as u32),
        Some(code) => format!("exit {code}"),
        None => "terminated without an exit code".to_string(),
    }
}

#[cfg(test)]
mod helper_hint_tests {
    use super::helper_exit_hint;

    #[test]
    fn windows_helper_status_codes_are_decoded() {
        assert!(helper_exit_hint(Some(-1073741511)).contains("0xC0000139"));
        assert!(helper_exit_hint(Some(-1073741511)).contains("ENTRYPOINT_NOT_FOUND"));
        assert!(helper_exit_hint(Some(-1073741515)).contains("0xC0000135"));
        assert!(helper_exit_hint(Some(-1073741515)).contains("DLL_NOT_FOUND"));
        assert_eq!(helper_exit_hint(Some(-1)), "0xFFFFFFFF");
        assert_eq!(helper_exit_hint(Some(3)), "exit 3");
        assert!(helper_exit_hint(None).contains("without an exit code"));
    }
}

pub(crate) fn helper_runner(
    state_root: PathBuf,
    max_files: usize,
    include_dirs: Vec<String>,
    exclude_dirs: Vec<String>,
    max_file_size: Option<u64>,
    max_depth: Option<u32>,
) -> impl FnMut(
    &PathBuf,
    &std::path::Path,
    &std::path::Path,
    &std::path::Path,
) -> Result<HelperOutput, String> {
    move |helper, analysis_input, export, ledger_state| {
        // Persistent per-project graph directory. The key is the stable
        // materialization path, so a project keeps one graph across runs.
        let graph_key = hash::digest_bytes(analysis_input.to_string_lossy().as_bytes());
        let graph_dir = ledger_state
            .join("index-graph")
            .join(graph_key.strip_prefix("sha256:").unwrap_or(&graph_key).chars().take(24).collect::<String>());
        let mut command = Command::new(helper);
        command
            .env("ARCHMELD_INDEX_GRAPH", &graph_dir)
            .arg(analysis_input)
            .arg(export)
            .arg(&state_root)
            .arg(max_files.to_string());
        for dir in &include_dirs {
            command.arg("--include-dir").arg(dir);
        }
        for dir in &exclude_dirs {
            command.arg("--exclude-dir").arg(dir);
        }
        if let Some(limit) = max_file_size {
            command.arg("--max-file-size").arg(limit.to_string());
        }
        if let Some(depth) = max_depth {
            command.arg("--max-depth").arg(depth.to_string());
        }
        let output = command
            .output()
            .map_err(|error| format!("run {}: {error}", helper.display()))?;
        Ok(HelperOutput {
            success: output.status.success(),
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

pub(crate) fn load_locator(path: Option<&PathBuf>) -> Result<TraceLocator, String> {
    let mut locator = TraceLocator::new();
    let Some(path) = path else {
        return Ok(locator);
    };
    let value: Value = serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;
    if let Some(locations) = value["locations"].as_array() {
        for entry in locations {
            let entity_id = entry["entity_id"]
                .as_str()
                .ok_or_else(|| "locator location needs entity_id".to_string())?;
            let line = entry["line"]
                .as_u64()
                .ok_or_else(|| "locator location needs a numeric line".to_string())?;
            let path = entry["path"]
                .as_str()
                .ok_or_else(|| "locator location needs path".to_string())?;
            locator.add_location(path, line, entity_id);
        }
    }
    if let Some(symbols) = value["symbols"].as_object() {
        for (symbol, entity) in symbols {
            let entity = entity
                .as_str()
                .ok_or_else(|| "locator symbol mapping must be a string".to_string())?;
            locator.add_symbol(symbol, entity);
        }
    }
    Ok(locator)
}

pub(crate) fn write_trace_result(out: Option<&PathBuf>, result: Value) -> Result<Value, String> {
    if let Some(path) = out {
        fs::write(
            path,
            serde_json::to_string_pretty(&result).map_err(|error| error.to_string())? + "\n",
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(result)
}
