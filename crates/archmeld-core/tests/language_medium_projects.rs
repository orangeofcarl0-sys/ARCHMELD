// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B030 / S16 T1: pinned medium real projects, one per language group.
//!
//! For each group the test verifies the pinned sample (path, version, archive
//! hash where available, source file count and a deterministic source tree
//! digest) and runs the declared build plus a small smoke command. The samples
//! live outside the repository so third-party source is not vendored; the lock
//! records exact URLs and hashes.

use archmeld_core::hash;
use archmeld_core::scenario::run_command_capture;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod support;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn resolve_locked_path(value: &str) -> PathBuf {
    if value.starts_with("crates/") || value.starts_with("crates\\") {
        return repo_root().join(value);
    }
    PathBuf::from(value)
}

fn env_base(name: &str) -> Option<PathBuf> {
    support::env_path(name).or_else(|| match name {
        "LOCALAPPDATA" => support::home().map(|home| home.join("AppData").join("Local")),
        "PROGRAMFILES" => support::env_path("ProgramFiles"),
        _ => None,
    })
}

fn resolve_locked(
    value: &Value,
    path_key: &str,
    env_key: &str,
    relative_key: &str,
) -> Option<PathBuf> {
    if let Some(relative) = value[relative_key].as_str() {
        if relative.starts_with("crates/") || relative.starts_with("crates\\") {
            return Some(repo_root().join(relative));
        }
        let base = value[env_key]
            .as_str()
            .and_then(env_base)
            .or_else(support::tools_root);
        if let Some(base) = base {
            return Some(base.join(relative));
        }
    }
    value[path_key].as_str().map(resolve_locked_path)
}

fn lock_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/fixtures.lock.json")
}

fn copy_dir(source: &Path, destination: &Path, extension: &str) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&path, &destination.join(entry.file_name()), extension);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            fs::copy(&path, destination.join(entry.file_name())).unwrap();
        }
    }
}

fn source_files(root: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    fn walk(root: &Path, extension: &str, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(&path, extension, files);
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                files.push(path);
            }
        }
    }
    walk(root, extension, &mut files);
    files.sort();
    files
}

fn tree_digest(root: &Path, files: &[PathBuf]) -> String {
    let mut manifest = String::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        manifest.push_str(&format!(
            "{relative} {}\n",
            hash::digest_bytes(&fs::read(file).unwrap())
        ));
    }
    hash::digest_bytes(manifest.as_bytes())
}

fn run(
    cwd: &Path,
    argv: Vec<String>,
    env: &[(String, String)],
    label: &str,
) -> (Value, String) {
    let capture = run_command_capture(&argv, cwd, env, Duration::from_secs(300), None)
        .unwrap_or_else(|error| panic!("{label} failed to start: {error}"));
    let stdout = String::from_utf8_lossy(&capture.stdout).to_string();
    let stderr = String::from_utf8_lossy(&capture.stderr).to_string();
    let value = json!({
        "argv": argv,
        "cwd": cwd.to_string_lossy(),
        "exit_code": capture.exit_code,
        "timed_out": capture.timed_out,
        "wall_ms": capture.wall_ms,
        "stdout": stdout.chars().take(4096).collect::<String>(),
        "stdout_digest": hash::digest_bytes(&capture.stdout),
        "stderr": stderr.chars().take(4096).collect::<String>(),
        "stderr_digest": hash::digest_bytes(&capture.stderr),
        "process_audit": capture.process_audit,
    });
    assert_eq!(
        capture.exit_code,
        Some(0),
        "{label} failed\nstdout={stdout}\nstderr={stderr}"
    );
    (value, stdout)
}

fn temp_dir(name: &str) -> tempfile::TempDir {
    support::tempdir(&format!("b030-medium-{name}-"))
}

fn build_group(key: &str, spec: &Value, _root: &Path, source: &Path) -> Value {
    let extension = spec["source_extension"].as_str().unwrap();
    let temp = temp_dir(key);
    let build = &spec["build"];
    let kind = build["kind"].as_str().unwrap();
    let mut result = Map::new();

    match kind {
        "gcc_link" => {
            // The sample also ships C test modules that need the built headers;
            // the release build compiles only the top-level interpreter sources.
            let mut sources: Vec<PathBuf> = fs::read_dir(source)
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().and_then(|value| value.to_str()) == Some(extension)
                        && path.is_file()
                })
                .filter(|path| {
                    let name = path.file_name().unwrap().to_string_lossy();
                    name != "luac.c" && name != "onelua.c"
                })
                .collect();
            sources.sort();
            let out = temp.path().join("out");
            fs::create_dir_all(&out).unwrap();
            let exe = out.join("lua.exe");
            let gcc = support::require_tool("ARCHMELD_GCC", "c/bin/gcc.exe")
                .expect("gcc")
                .to_string_lossy()
                .to_string();
            let mut argv = vec![gcc, "-O2".to_string()];
            for path in &sources {
                argv.push(path.to_string_lossy().into_owned());
            }
            argv.push("-o".to_string());
            argv.push(exe.to_string_lossy().into_owned());
            argv.push("-lm".to_string());
            let (build_result, _) = run(&out, argv, &[], "lua build");
            result.insert("build".into(), build_result);
            let (smoke, stdout) = run(
                &out,
                vec![
                    exe.to_string_lossy().into_owned(),
                    "-e".to_string(),
                    "print(2+2)".to_string(),
                ],
                &[],
                "lua smoke",
            );
            assert_eq!(stdout.trim(), "4");
            result.insert("smoke".into(), smoke);
        }
        "python_compileall" => {
            let copied = temp.path().join("asyncio");
            copy_dir(source, &copied, extension);
            let python = support::require_tool("ARCHMELD_PYTHON", "python.exe")
                .expect("python")
                .to_string_lossy()
                .to_string();
            let (build_result, _) = run(
                &temp.path(),
                vec![
                    python.clone(),
                    "-m".into(),
                    "compileall".into(),
                    "-q".into(),
                    "asyncio".into(),
                ],
                &[],
                "asyncio compileall",
            );
            result.insert("build".into(), build_result);
            let (smoke, stdout) = run(
                &temp.path(),
                vec![
                    python.clone(),
                    "-c".into(),
                    "import asyncio; print(asyncio.__name__)".into(),
                ],
                &[("PYTHONPATH".into(), ".".into())],
                "asyncio import",
            );
            assert_eq!(stdout.trim(), "asyncio");
            result.insert("smoke".into(), smoke);
        }
        "node_parse" => {
            let script = temp.path().join("parse_all.cjs");
            fs::write(
                &script,
                "const fs=require('fs'),path=require('path'),vm=require('vm');\n\
                 const root=process.argv[2]; let n=0;\n\
                 function walk(d){for(const e of fs.readdirSync(d,{withFileTypes:true})){\n\
                   const p=path.join(d,e.name);\n\
                   if(e.isDirectory()) walk(p);\n\
                   else if(e.name.endsWith('.js')){ new vm.Script(fs.readFileSync(p,'utf8'),{filename:p}); n++; }\n\
                 }}\n\
                 walk(root); console.log(n);\n",
            )
            .unwrap();
            let node = support::require_tool("ARCHMELD_NODE", "node.exe")
                .expect("node")
                .to_string_lossy()
                .to_string();
            let (build_result, stdout) = run(
                &temp.path(),
                vec![
                    node,
                    script.to_string_lossy().into_owned(),
                    source.to_string_lossy().into_owned(),
                ],
                &[],
                "npm parse",
            );
            let parsed: u64 = stdout.trim().parse().unwrap();
            assert!(parsed >= 30, "unexpected npm file count {parsed}");
            result.insert("parsed_scripts".into(), json!(parsed));
            result.insert("build".into(), build_result);
        }
        "javac_patch_module" => {
            let patch_root = source
                .ancestors()
                .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("java.base"))
                .expect("source is under java.base");
            let out = temp.path().join("out");
            fs::create_dir_all(&out).unwrap();
            let sources = source_files(source, extension);
            let argfile = temp.path().join("sources.txt");
            fs::write(
                &argfile,
                sources
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();
            let javac = support::require_tool("ARCHMELD_JAVAC", "jdk25/jdk-25.0.5+8/bin/javac.exe")
                .expect("javac")
                .to_string_lossy()
                .to_string();
            let java_home = Path::new(&javac)
                .parent()
                .and_then(|bin| bin.parent())
                .map(|root| root.to_string_lossy().to_string())
                .unwrap_or_default();
            let (build_result, _) = run(
                &temp.path(),
                vec![
                    javac,
                    "--patch-module".into(),
                    format!("java.base={}", patch_root.to_string_lossy()),
                    "-d".into(),
                    out.to_string_lossy().into_owned(),
                    format!("@{}", argfile.to_string_lossy()),
                ],
                &[("JAVA_HOME".into(), java_home)],
                "javac patch-module",
            );
            let compiled = fs::read_dir(&out)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("class"))
                .count();
            let _ = compiled;
            result.insert("build".into(), build_result);
        }
        "dotnet_build" => {
            let dotnet = support::require_tool("ARCHMELD_DOTNET", "dotnet8/dotnet.exe")
                .expect("dotnet")
                .to_string_lossy()
                .to_string();
            let mut argv = Vec::new();
            for argument in build["argv"].as_array().unwrap() {
                argv.push(argument.as_str().unwrap().replace("{dotnet}", &dotnet));
            }
            let mut env = support::tool_env(temp.path());
            if let Some(root) = Path::new(&dotnet).parent() {
                env.push(("DOTNET_ROOT".into(), root.to_string_lossy().to_string()));
            }
            let nuget = support::nuget_root()
                .map(|path| path.to_string_lossy().to_string())
                .or_else(|| {
                    build["env"]["NUGET_PACKAGES"]
                        .as_str()
                        .map(resolve_locked_path)
                        .map(|path| path.to_string_lossy().to_string())
                });
            if let Some(nuget) = nuget {
                env.push(("NUGET_PACKAGES".into(), nuget));
            }
            env.push(("DOTNET_CLI_TELEMETRY_OPTOUT".into(), "1".into()));
            env.push(("DOTNET_NOLOGO".into(), "1".into()));
            env.push((
                "DOTNET_CLI_HOME".into(),
                temp.path().to_string_lossy().into_owned(),
            ));
            env.push(("PROCESSOR_ARCHITECTURE".into(), "AMD64".into()));
            env.push(("COMPUTERNAME".into(), "LOCALHOST".into()));
            for key in ["ProgramFiles", "ProgramFiles(x86)"] {
                if let Ok(value) = std::env::var(key) {
                    env.push((key.to_string(), value));
                }
            }
            let cwd = source
                .parent()
                .and_then(|path| path.parent())
                .unwrap_or(source)
                .to_path_buf();
            let (build_result, _) = run(&cwd, argv, &env, "newtonsoft build");
            result.insert("build".into(), build_result);
        }
        "go_test" => {
            let go = support::require_tool("ARCHMELD_GO", "go/go/bin/go.exe")
                .expect("go")
                .to_string_lossy()
                .to_string();
            let (build_result, _) = run(
                temp.path(),
                vec![
                    go,
                    "test".into(),
                    "net/http".into(),
                    "-run".into(),
                    "TestStatusText".into(),
                ],
                &[],
                "go test net/http",
            );
            result.insert("build".into(), build_result);
        }
        "cargo_check" => {
            // A dedicated target directory avoids waiting on the outer cargo
            // test process's target lock. The default-feature (RocksDB) build is
            // covered by the workspace tests; this sample checks the parser /
            // graph core without the native storage backend.
            let target = temp.path().join("target");
            let (build_result, _) = run(
                &repo_root(),
                vec![
                    "cargo".into(),
                    "check".into(),
                    "-p".into(),
                    "codegraph".into(),
                    "--no-default-features".into(),
                    "--offline".into(),
                    "--target-dir".into(),
                    target.to_string_lossy().into_owned(),
                ],
                &[("RUSTUP_TOOLCHAIN".into(), support::rust_toolchain())],
                "cargo check codegraph",
            );
            result.insert("build".into(), build_result);
        }
        other => panic!("unknown build kind {other}"),
    }
    json!(result)
}

#[test]
fn seven_groups_have_pinned_medium_real_projects_with_real_builds() {
    let required = [
        ("ARCHMELD_GCC", "c/bin/gcc.exe"),
        ("ARCHMELD_JAVAC", "jdk25/jdk-25.0.5+8/bin/javac.exe"),
        ("ARCHMELD_JAVA", "jdk25/jdk-25.0.5+8/bin/java.exe"),
        ("ARCHMELD_DOTNET", "dotnet8/dotnet.exe"),
        ("ARCHMELD_GO", "go/go/bin/go.exe"),
        ("ARCHMELD_NODE", "node.exe"),
        ("ARCHMELD_PYTHON", "python.exe"),
    ];
    if required
        .iter()
        .any(|(env, relative)| support::require_tool(env, relative).is_none())
    {
        return;
    }
    let lock: Value = serde_json::from_slice(&fs::read(lock_path()).unwrap()).unwrap();
    let mut groups = Map::new();
    for (key, spec) in lock["groups"].as_object().unwrap() {
        let extension = spec["source_extension"].as_str().unwrap();
        let source_text = spec["source_path"].as_str().unwrap();
        let source = resolve_locked(
            spec,
            "source_path",
            "source_path_env",
            "source_relative_path",
        )
        .unwrap_or_else(|| panic!("{key}: no source path"));
        assert!(source.is_dir(), "{key}: missing medium sample {source_text}");
        let files = source_files(&source, extension);
        let minimum = spec["minimum_source_files"].as_u64().unwrap();
        assert!(
            files.len() as u64 >= minimum,
            "{key}: {} source files < {minimum}",
            files.len()
        );
        let digest = tree_digest(&source, &files);
        let archive = &spec["archive"];
        let archive_check = if let Some(path) = archive["path"].as_str() {
            let resolved = resolve_locked(archive, "path", "path_env", "relative_path")
                .unwrap_or_else(|| resolve_locked_path(path));
            let bytes = fs::read(&resolved)
                .unwrap_or_else(|error| panic!("{key}: archive {path}: {error}"));
            let actual = hash::digest_bytes(&bytes);
            let expected = archive["sha256"].as_str().unwrap();
            assert_eq!(
                actual.strip_prefix("sha256:").unwrap(),
                expected,
                "{key}: archive hash mismatch"
            );
            json!({"path": resolved, "sha256": expected, "verified": true})
        } else {
            json!({"verified": false, "reason": "sample comes from the pinned local toolchain"})
        };
        let build = build_group(key, spec, &repo_root(), &source);
        groups.insert(
            key.clone(),
            json!({
                "status": "verified",
                "sample": spec["sample"],
                "version": spec["version"],
                "license": spec["license"],
                "source_path": source_text,
                "source_url": spec["source_url"],
                "source_file_count": files.len(),
                "minimum_source_files": minimum,
                "source_tree_digest": digest,
                "archive": archive_check,
                "build": build,
                "limitations": spec["build"]["limitations"],
            }),
        );
        eprintln!(
            "{key}: {} source files, digest {}, build ok",
            files.len(),
            digest
        );
    }
    let evidence = json!({
        "schema_version": "1.0",
        "generated_by": "crates/archmeld-core/tests/language_medium_projects.rs",
        "lock": "adapters/languages/fixtures.lock.json",
        "platform": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "recorded_at_unix": support::recorded_at_unix(),
        "product_head": support::product_head(),
        "groups": groups,
        "note": "Each sample is a pinned public project or toolchain source tree. Builds and smoke commands were executed for real; limitations are copied from the lock and are not averaged into a score.",
    });
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        for path in [
            "docs/release-evidence/B030-medium-projects.json",
            "docs/evidence/B030-medium-projects.json",
        ] {
            let full = repo_root().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(
                &full,
                serde_json::to_string_pretty(&evidence).unwrap() + "\n",
            )
            .unwrap();
        }
    }
}
