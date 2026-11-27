// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B030: real build and runtime execution for the seven language groups.
//!
//! This matrix does not promote execution into runtime linkage. A group is only
//! `runtime_linkage=verified` when a captured trace is imported and mapped back
//! to the correct source entity; that remains an explicit blocker here.

use archmeld_core::hash;
use archmeld_core::scenario::run_command_capture;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod support;

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            "target" | "obj" | "bin" | "__pycache__" | ".git" | "node_modules"
        ) {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn matrix_tempdir() -> tempfile::TempDir {
    support::tempdir("b030-matrix-")
}

fn fixture_root(language: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/languages/fixtures")
        .join(language)
}

fn run(cwd: &Path, argv: Vec<String>, env: &[(String, String)]) -> Value {
    let capture = run_command_capture(&argv, cwd, env, Duration::from_secs(120), None)
        .unwrap_or_else(|error| panic!("command {argv:?} failed to start: {error}"));
    json!({
        "argv": argv,
        "cwd": cwd.to_string_lossy(),
        "exit_code": capture.exit_code,
        "timed_out": capture.timed_out,
        "wall_ms": capture.wall_ms,
        "stdout": String::from_utf8_lossy(&capture.stdout).chars().take(4096).collect::<String>(),
        "stdout_digest": hash::digest_bytes(&capture.stdout),
        "stderr": String::from_utf8_lossy(&capture.stderr).chars().take(4096).collect::<String>(),
        "stderr_digest": hash::digest_bytes(&capture.stderr),
        "process_audit": capture.process_audit,
    })
}

fn require_ok(result: &Value, label: &str) -> String {
    assert_eq!(
        result["exit_code"].as_i64(),
        Some(0),
        "{label} failed: stdout={} stderr={}",
        result["stdout"],
        result["stderr"]
    );
    result["stdout"].as_str().unwrap_or("").trim().to_string()
}

fn run_ok(result: &Value) -> bool {
    result["exit_code"].as_i64() == Some(0)
}

fn evidence_verified(result: &Value) -> bool {
    result["state"].as_str() == Some("verified")
}

fn tool_version(argv: Vec<String>) -> Value {
    let cwd = std::env::current_dir().unwrap();
    let capture = run_command_capture(&argv, &cwd, &[], Duration::from_secs(30), None).ok();
    match capture {
        Some(capture) => json!({
            "argv": argv,
            "exit_code": capture.exit_code,
            "first_line": String::from_utf8_lossy(&capture.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_string(),
        }),
        None => json!({"argv": argv, "error": "unavailable"}),
    }
}

#[test]
fn seven_language_groups_build_and_execute_real_fixtures() {
    let required = [
        ("ARCHMELD_NODE", "node.exe"),
        ("ARCHMELD_TSC", "lang-servers/node_modules/typescript/bin/tsc"),
        ("ARCHMELD_JAVAC", "jdk25/jdk-25.0.5+8/bin/javac.exe"),
        ("ARCHMELD_JAVA", "jdk25/jdk-25.0.5+8/bin/java.exe"),
        ("ARCHMELD_DOTNET", "dotnet8/dotnet.exe"),
        ("ARCHMELD_GXX", "c/bin/g++.exe"),
        ("ARCHMELD_CLANGXX", "llvm18/bin/clang++.exe"),
        ("ARCHMELD_GO", "go/go/bin/go.exe"),
        ("ARCHMELD_PYTHON", "python.exe"),
    ];
    if required
        .iter()
        .any(|(env, relative)| support::require_tool(env, relative).is_none())
    {
        return;
    }
    let base = matrix_tempdir();
    let node = support::require_tool("ARCHMELD_NODE", "node.exe")
        .expect("node")
        .to_string_lossy()
        .to_string();
    let tsc = support::require_tool("ARCHMELD_TSC", "lang-servers/node_modules/typescript/bin/tsc")
        .expect("tsc")
        .to_string_lossy()
        .to_string();
    let javac = support::require_tool("ARCHMELD_JAVAC", "jdk25/jdk-25.0.5+8/bin/javac.exe")
        .expect("javac")
        .to_string_lossy()
        .to_string();
    let java = support::require_tool("ARCHMELD_JAVA", "jdk25/jdk-25.0.5+8/bin/java.exe")
        .expect("java")
        .to_string_lossy()
        .to_string();
    let dotnet = support::require_tool("ARCHMELD_DOTNET", "dotnet8/dotnet.exe")
        .expect("dotnet")
        .to_string_lossy()
        .to_string();
    let gpp = support::require_tool("ARCHMELD_GXX", "c/bin/g++.exe")
        .expect("g++")
        .to_string_lossy()
        .to_string();
    let clang = support::require_tool("ARCHMELD_CLANGXX", "llvm18/bin/clang++.exe")
        .expect("clang++")
        .to_string_lossy()
        .to_string();
    let go = support::require_tool("ARCHMELD_GO", "go/go/bin/go.exe")
        .expect("go")
        .to_string_lossy()
        .to_string();
    let rust_env = vec![("RUSTUP_TOOLCHAIN".to_string(), support::rust_toolchain())];
    let mut groups = serde_json::Map::new();

    // Python
    let python = base.path().join("python");
    copy_dir(&fixture_root("python"), &python);
    let python_cmd = support::require_tool("ARCHMELD_PYTHON", "python.exe")
        .expect("python")
        .to_string_lossy()
        .to_string();
    let build = run(
        &python,
        vec![
            python_cmd.clone(),
            "-m".to_string(),
            "compileall".to_string(),
            "-q".to_string(),
            ".".to_string(),
        ],
        &[],
    );
    require_ok(&build, "python compileall");
    let runtime = run(
        &python,
        vec![
            python_cmd.clone(),
            "-c".to_string(),
            "import app; print(app.main(3))".to_string(),
        ],
        &[],
    );
    assert_eq!(require_ok(&runtime, "python runtime"), "6");
    groups.insert(
        "python".to_string(),
        json!({
            "build": build,
            "runtime": runtime,
            "runtime_linkage": "not_verified: execution is real, no imported trace mapped to source",
            "medium_project": "not_run",
        }),
    );

    // TypeScript / JavaScript (one language group)
    let typescript = base.path().join("typescript");
    copy_dir(&fixture_root("typescript"), &typescript);
    let build = run(
        &typescript,
        vec![node.clone(), tsc.clone(), "--noEmit".to_string(), "-p".to_string(), ".".to_string()],
        &[],
    );
    require_ok(&build, "typescript tsc --noEmit");
    let emit = run(
        &typescript,
        vec![node.clone(), tsc.clone(), "-p".to_string(), ".".to_string(), "--outDir".to_string(), "out".to_string()],
        &[],
    );
    require_ok(&emit, "typescript tsc emit");
    let runtime = run(
        &typescript,
        vec![
            node.clone(),
            "-e".to_string(),
            "console.log(require('./out/app.js').value)".to_string(),
        ],
        &[],
    );
    assert_eq!(require_ok(&runtime, "typescript runtime"), "4");

    let javascript = base.path().join("javascript");
    copy_dir(&fixture_root("javascript"), &javascript);
    let package_path = javascript.join("package.json");
    let mut package: Value = serde_json::from_slice(&fs::read(&package_path).unwrap()).unwrap();
    package["type"] = json!("module");
    fs::write(&package_path, serde_json::to_vec_pretty(&package).unwrap()).unwrap();
    let js_build = run(
        &javascript,
        vec![node.clone(), "--check".to_string(), "src/app.js".to_string()],
        &[],
    );
    require_ok(&js_build, "javascript node --check");
    let js_runtime = run(
        &javascript,
        vec![
            node.clone(),
            "-e".to_string(),
            "import('./src/app.js').then((m) => console.log(m.value))".to_string(),
        ],
        &[],
    );
    assert_eq!(require_ok(&js_runtime, "javascript runtime"), "4");
    groups.insert(
        "javascript_typescript".to_string(),
        json!({
            "typescript": {"build": build, "emit": emit, "runtime": runtime},
            "javascript": {"build": js_build, "runtime": js_runtime},
            "runtime_linkage": "not_verified: execution is real, no imported trace mapped to source",
            "medium_project": "not_run",
        }),
    );

    // C/C++
    let cpp = base.path().join("c-cpp");
    copy_dir(&fixture_root("c-cpp"), &cpp);
    fs::write(
        cpp.join("src/e2e_main.cpp"),
        "#include \"shape.h\"\n#include <cstdio>\nint main(){ Circle c(2.0); std::printf(\"%.2f %d\\n\", c.area(), static_cast<int>(scale_value(3.0))); return 0; }\n",
    )
    .unwrap();
    let build = run(
        &cpp,
        vec![
            gpp.clone(),
            "-std=c++17".to_string(),
            "-DFACTOR=2".to_string(),
            "-Isrc".to_string(),
            "src/main.cpp".to_string(),
            "src/circle.cpp".to_string(),
            "src/e2e_main.cpp".to_string(),
            "-o".to_string(),
            "e2e.exe".to_string(),
        ],
        &[],
    );
    require_ok(&build, "c++ build");
    let runtime = run(&cpp, vec![cpp.join("e2e.exe").to_string_lossy().to_string()], &[]);
    assert_eq!(require_ok(&runtime, "c++ runtime"), "12.57 6");
    groups.insert(
        "c_cpp".to_string(),
        json!({
            "build": build,
            "runtime": runtime,
            "runtime_linkage": "not_verified: execution is real, no imported trace mapped to source",
            "medium_project": "not_run",
        }),
    );

    // Java
    let java_root = base.path().join("java");
    copy_dir(&fixture_root("java"), &java_root);
    fs::write(
        java_root.join("src/main/java/pkg/Main.java"),
        "package pkg;\npublic final class Main { public static void main(String[] args) { System.out.println(App.run()); } }\n",
    )
    .unwrap();
    let mut sources = Vec::new();
    sources.push(javac.clone());
    sources.push("-d".to_string());
    sources.push("out".to_string());
    for entry in fs::read_dir(java_root.join("src/main/java/pkg")).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|value| value.to_str()) == Some("java") {
            sources.push(entry.path().to_string_lossy().to_string());
        }
    }
    let java_home = Path::new(&java)
        .parent()
        .and_then(|bin| bin.parent())
        .map(|root| root.to_string_lossy().to_string())
        .unwrap_or_default();
    let java_env = vec![("JAVA_HOME".to_string(), java_home)];
    let build = run(&java_root, sources, &java_env);
    require_ok(&build, "java build");
    let runtime = run(
        &java_root,
        vec![
            java.clone(),
            "-cp".to_string(),
            "out".to_string(),
            "pkg.Main".to_string(),
        ],
        &java_env,
    );
    assert_eq!(require_ok(&runtime, "java runtime"), "24");
    groups.insert(
        "java".to_string(),
        json!({
            "build": build,
            "runtime": runtime,
            "runtime_linkage": "not_verified: Java runtime scenario not instrumented/imported",
            "medium_project": "not_run",
        }),
    );

    // C#
    let csharp = base.path().join("csharp");
    copy_dir(&fixture_root("csharp"), &csharp);
    let mut csharp_env = support::tool_env(&csharp);
    if let Some(root) = Path::new(&dotnet).parent() {
        csharp_env.push(("DOTNET_ROOT".to_string(), root.to_string_lossy().to_string()));
    }
    if let Some(nuget) = support::nuget_root() {
        csharp_env.push(("NUGET_PACKAGES".to_string(), nuget.to_string_lossy().to_string()));
    }
    csharp_env.push(("DOTNET_CLI_TELEMETRY_OPTOUT".to_string(), "1".to_string()));
    csharp_env.push(("DOTNET_NOLOGO".to_string(), "1".to_string()));
    csharp_env.push(("DOTNET_CLI_HOME".to_string(), csharp.to_string_lossy().to_string()));
    csharp_env.push(("PROCESSOR_ARCHITECTURE".to_string(), "AMD64".to_string()));
    csharp_env.push(("COMPUTERNAME".to_string(), "LOCALHOST".to_string()));
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(value) = std::env::var(key) {
            csharp_env.push((key.to_string(), value));
        }
    }
    let build = run(
        &csharp,
        vec![
            dotnet.clone(),
            "build".to_string(),
            "-nologo".to_string(),
            "-v:q".to_string(),
        ],
        &csharp_env,
    );
    require_ok(&build, "csharp build");
    let runtime = run(
        &csharp,
        vec![
            dotnet.clone(),
            "bin/Debug/net8.0/LangPack.dll".to_string(),
        ],
        &csharp_env,
    );
    assert_eq!(require_ok(&runtime, "csharp runtime"), "12");
    groups.insert(
        "csharp".to_string(),
        json!({
            "build": build,
            "runtime": runtime,
            "runtime_linkage": "not_verified: no EventPipe trace imported",
            "medium_project": "not_run",
        }),
    );

    // Go
    let go_root = base.path().join("go");
    copy_dir(&fixture_root("go"), &go_root);
    let build = run(
        &go_root,
        vec![go.clone(), "build".to_string(), "./...".to_string()],
        &[],
    );
    require_ok(&build, "go build");
    let test = run(
        &go_root,
        vec![go.clone(), "test".to_string(), "./...".to_string()],
        &[],
    );
    require_ok(&test, "go test");
    let runtime = run(
        &go_root,
        vec![go.clone(), "run".to_string(), ".".to_string()],
        &[],
    );
    assert_eq!(require_ok(&runtime, "go runtime"), "2 4 6");
    groups.insert(
        "go".to_string(),
        json!({
            "build": build,
            "test": test,
            "runtime": runtime,
            "runtime_linkage": "not_verified: no goroutine/OTel trace imported",
            "medium_project": "not_run",
        }),
    );

    // Rust
    let rust_root = base.path().join("rust");
    copy_dir(&fixture_root("rust"), &rust_root);
    fs::create_dir_all(rust_root.join("examples")).unwrap();
    fs::write(
        rust_root.join("examples/run.rs"),
        "fn main() { println!(\"{}\", langpack_rust::scaled()); }\n",
    )
    .unwrap();
    let build = run(
        &rust_root,
        vec![
            "cargo".to_string(),
            "check".to_string(),
            "--quiet".to_string(),
            "--offline".to_string(),
        ],
        &rust_env,
    );
    require_ok(&build, "rust cargo check");
    let test = run(
        &rust_root,
        vec![
            "cargo".to_string(),
            "test".to_string(),
            "--quiet".to_string(),
            "--offline".to_string(),
        ],
        &rust_env,
    );
    require_ok(&test, "rust cargo test");
    let runtime = run(
        &rust_root,
        vec![
            "cargo".to_string(),
            "run".to_string(),
            "--quiet".to_string(),
            "--offline".to_string(),
            "--example".to_string(),
            "run".to_string(),
        ],
        &rust_env,
    );
    assert_eq!(require_ok(&runtime, "rust runtime"), "2");
    groups.insert(
        "rust".to_string(),
        json!({
            "build": build,
            "test": test,
            "runtime": runtime,
            "runtime_linkage": "not_verified: no async/runtime trace imported",
            "medium_project": "not_run",
        }),
    );

    let tool_versions = json!({
        "python": tool_version(vec![python_cmd, "--version".into()]),
        "node": tool_version(vec![node.clone(), "--version".into()]),
        "typescript": tool_version(vec![node.clone(), tsc, "--version".to_string()]),
        "javac": tool_version(vec![javac, "-version".to_string()]),
        "dotnet": tool_version(vec![dotnet, "--version".to_string()]),
        "g++": tool_version(vec![gpp, "--version".to_string()]),
        "clang++": tool_version(vec![clang, "--version".to_string()]),
        "go": tool_version(vec![go, "version".to_string()]),
        "rustc": tool_version(vec!["rustc".into(), "--version".into()]),
    });
    let runtime_evidence_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/release-evidence/B030-runtime-linkage.json");
    let runtime_evidence = fs::read(&runtime_evidence_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if let Some(runtime) = &runtime_evidence {
        if let Some(entries) = runtime["groups"].as_object() {
            for (name, report) in entries {
                let linkage = json!({
                    "state": "verified",
                    "evidence": "docs/release-evidence/B030-runtime-linkage.json",
                    "capture_tool": report["capture_tool"],
                    "capture_version": report["capture_version"],
                    "mapped_spans": report["mapped_spans"],
                    "unmapped_spans": report["unmapped_spans"],
                    "observed_relations": report["observed_relations"],
                    "lossy": report["lossy"],
                    "raw_artifact_digest": report["raw_artifact_digest"],
                });
                if name == "javascript" || name == "typescript" {
                    if let Some(group) = groups.get_mut("javascript_typescript") {
                        group[format!("runtime_linkage_{name}")] = linkage;
                    }
                } else if let Some(group) = groups.get_mut(name) {
                    group["runtime_linkage"] = linkage;
                }
            }
            if let Some(group) = groups.get_mut("javascript_typescript") {
                let javascript = group
                    .get("runtime_linkage_javascript")
                    .cloned()
                    .unwrap_or(Value::Null);
                let typescript = group
                    .get("runtime_linkage_typescript")
                    .cloned()
                    .unwrap_or(Value::Null);
                group["runtime_linkage"] = json!({
                    "state": "verified",
                    "evidence": "docs/release-evidence/B030-runtime-linkage.json",
                    "javascript": javascript,
                    "typescript": typescript,
                });
            }
        }
    }
    let medium_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/release-evidence/B030-medium-projects.json");
    let medium = fs::read(&medium_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if let Some(medium) = &medium {
        if let Some(entries) = medium["groups"].as_object() {
            for (name, report) in entries {
                if let Some(group) = groups.get_mut(name) {
                    group["medium_project"] = json!({
                        "state": "verified",
                        "evidence": "docs/release-evidence/B030-medium-projects.json",
                        "sample": report["sample"],
                        "version": report["version"],
                        "source_file_count": report["source_file_count"],
                        "source_tree_digest": report["source_tree_digest"],
                        "build_exit_code": report["build"]["build"]["exit_code"],
                        "limitations": report["limitations"],
                    });
                }
            }
        }
    }
    let rename_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/release-evidence/B030-rename-apply.json");
    let rename = fs::read(&rename_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if let Some(rename) = &rename {
        if let Some(entries) = rename["groups"].as_object() {
            for (name, report) in entries {
                let entry = json!({
                    "state": "verified",
                    "evidence": "docs/release-evidence/B030-rename-apply.json",
                    "tool": report["applied"]["tool"],
                    "changeset_id": report["applied"]["changeset_id"],
                    "changed_files": report["applied"]["changed_files"],
                    "before_output": report["before_output"],
                    "after_output": report["after_output"],
                });
                if name == "javascript_typescript" && report["javascript"].is_object() {
                    if let Some(group) = groups.get_mut("javascript_typescript") {
                        group["controlled_change"] = json!({
                            "state": "verified",
                            "evidence": "docs/release-evidence/B030-rename-apply.json",
                            "typescript": entry,
                            "javascript": {
                                "state": "verified",
                                "tool": report["javascript"]["applied"]["tool"],
                                "changeset_id": report["javascript"]["applied"]["changeset_id"],
                                "changed_files": report["javascript"]["applied"]["changed_files"],
                                "before_output": report["javascript"]["before_output"],
                                "after_output": report["javascript"]["after_output"],
                            },
                        });
                    }
                } else if let Some(group) = groups.get_mut(name) {
                    group["controlled_change"] = entry;
                }
            }
        }
    }
    let mut blockers: Vec<String> = Vec::new();
    if runtime_evidence.is_none() {
        blockers.push("runtime-linkage evidence missing".to_string());
    }
    if medium.is_none() {
        blockers.push("medium-project evidence missing".to_string());
    }
    if rename.is_none() {
        blockers.push("controlled-change evidence missing".to_string());
    }
    for (name, group) in &groups {
        if name == "javascript_typescript" {
            for part in ["typescript", "javascript"] {
                if !run_ok(&group[part]["build"]) {
                    blockers.push(format!("{name}.{part}: build not ok"));
                }
                if !run_ok(&group[part]["runtime"]) {
                    blockers.push(format!("{name}.{part}: runtime not ok"));
                }
            }
            for part in ["javascript", "typescript"] {
                if !evidence_verified(&group["runtime_linkage"][part]) {
                    blockers.push(format!("{name}.{part}: runtime linkage not verified"));
                }
                if !evidence_verified(&group["controlled_change"][part]) {
                    blockers.push(format!("{name}.{part}: controlled change not verified"));
                }
            }
        } else {
            if !run_ok(&group["build"]) {
                blockers.push(format!("{name}: build not ok"));
            }
            if !run_ok(&group["runtime"]) {
                blockers.push(format!("{name}: runtime not ok"));
            }
            if !evidence_verified(&group["runtime_linkage"]) {
                blockers.push(format!("{name}: runtime linkage not verified"));
            }
            if !evidence_verified(&group["controlled_change"]) {
                blockers.push(format!("{name}: controlled change not verified"));
            }
        }
        if !evidence_verified(&group["medium_project"]) {
            blockers.push(format!("{name}: medium project not verified"));
        } else if group["medium_project"]["build_exit_code"].as_i64() != Some(0) {
            blockers.push(format!("{name}: medium project build failed"));
        }
    }
    let status = if blockers.is_empty() { "passed" } else { "failed" };
    let r1_naming_allowed = blockers.is_empty();
    let limitations = vec![
        "C/C++ full SCIP indexing still requires a Linux/WSL worker on Windows; Windows clangd definition/reference and rename apply are verified",
    ];
    let matrix = json!({
        "schema_version": "1.0",
        "generated_by": "crates/archmeld-core/tests/language_matrix.rs",
        "platform": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "recorded_at_unix": support::recorded_at_unix(),
        "product_head": support::product_head(),
        "tool_versions": tool_versions,
        "groups": groups,
        "runtime_linkage_evidence": {
            "path": "docs/release-evidence/B030-runtime-linkage.json",
            "loaded": runtime_evidence.is_some(),
        },
        "medium_project_evidence": {
            "path": "docs/release-evidence/B030-medium-projects.json",
            "loaded": medium.is_some(),
        },
        "rename_evidence": {
            "path": "docs/release-evidence/B030-rename-apply.json",
            "loaded": rename.is_some(),
        },
        "blockers": blockers,
        "limitations": limitations,
        "AT067": {
            "status": status,
            "reason": if r1_naming_allowed {
                "all seven language groups have real build/runtime evidence, imported and mapped runtime linkage, a pinned medium real project with a real build, and a semantic rename applied through the prepared-digest ChangeSet gate"
            } else {
                "one or more language groups are missing build/runtime, runtime linkage, medium-project or controlled-change evidence; see blockers"
            },
            "evidence": [
                "docs/release-evidence/B030-runtime-linkage.json",
                "docs/release-evidence/B030-medium-projects.json",
                "docs/release-evidence/B030-rename-apply.json",
                "docs/release-evidence/B030-language-matrix.json"
            ],
            "r1_naming_allowed": r1_naming_allowed,
            "limitations": limitations,
        },
    });
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/release-evidence/B030-language-matrix.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&matrix).unwrap() + "\n").unwrap();
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/evidence/B030-language-matrix.json");
        fs::write(evidence, serde_json::to_string_pretty(&matrix).unwrap() + "\n").unwrap();
    }
    assert!(
        r1_naming_allowed,
        "AT067 blockers: {}",
        matrix["blockers"]
    );
}
