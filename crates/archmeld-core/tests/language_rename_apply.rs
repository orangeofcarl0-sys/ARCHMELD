// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B030: per-language semantic rename applied through the controlled ChangeSet
//! gate. Each group performs a real LSP rename, applies the returned
//! WorkspaceEdit to the staging tree, publishes a prepared digest, obtains a
//! second approval and applies it. The original repository is untouched until
//! apply; same-named symbols in other files must stay unchanged and the
//! project must still build/run afterwards.

mod common;

use archmeld_core::language::{apply_workspace_edit, PreviewRequest};
use archmeld_core::lsp::{LspServerSpec, LspSession, SpawnPermission};
use archmeld_core::scenario::run_command_capture;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TOOLS: &str = "F:/Codex_Work_Space/_archmeld_tools";
const TEMP: &str = "F:/Codex_Work_Space/_archmeld_temp";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/fixtures")
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn position(text: &str, needle: &str) -> (u64, u64) {
    let index = text.find(needle).unwrap();
    let before = &text[..index];
    (
        before.matches('\n').count() as u64,
        before.rsplit('\n').next().unwrap().encode_utf16().count() as u64,
    )
}

fn has_edits(edit: &Value) -> bool {
    edit["documentChanges"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["edits"].as_array().is_some_and(|edits| !edits.is_empty()))
    }) || edit["changes"].as_object().is_some_and(|changes| {
        changes
            .values()
            .any(|edits| edits.as_array().is_some_and(|items| !items.is_empty()))
    })
}

fn rename_edit(
    session: &LspSession,
    file: &Path,
    needle: &str,
    new_name: &str,
    attempts: usize,
    expected_files: &[&str],
) -> Value {
    let text = fs::read_to_string(file).unwrap();
    let (line, character) = position(&text, needle);
    for _ in 0..attempts {
        if let Ok(value) = session.rename(file, line, character, new_name, Duration::from_secs(30)) {
            let serialized = serde_json::to_string(&value)
                .unwrap_or_default()
                .replace('\\', "/");
            if has_edits(&value)
                && expected_files
                    .iter()
                    .all(|expected| serialized.contains(expected))
            {
                return value;
            }
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    panic!(
        "language server returned no complete rename for {needle} in {} (expected {expected_files:?})",
        file.display()
    );
}

fn temp_dir(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("b030-rename-{label}-"))
        .tempdir_in(TEMP)
        .unwrap()
}

fn run(
    cwd: &Path,
    argv: Vec<String>,
    env: &[(String, String)],
    label: &str,
) -> (Option<i32>, String, String) {
    let capture = run_command_capture(&argv, cwd, env, Duration::from_secs(300), None)
        .unwrap_or_else(|error| panic!("{label} failed to start: {error}"));
    (
        capture.exit_code,
        String::from_utf8_lossy(&capture.stdout).to_string(),
        String::from_utf8_lossy(&capture.stderr).to_string(),
    )
}

fn require_ok(
    cwd: &Path,
    argv: Vec<String>,
    env: &[(String, String)],
    label: &str,
) -> String {
    let (code, stdout, stderr) = run(cwd, argv, env, label);
    assert_eq!(code, Some(0), "{label} failed\nstdout={stdout}\nstderr={stderr}");
    stdout
}

fn build_fixture(kind: &str, root: &Path, temp: &Path) -> String {
    match kind {
        "cpp" => {
            let exe = temp.join("e2e.exe");
            require_ok(
                root,
                vec![
                    "D:/Strawberry/c/bin/g++.exe".into(),
                    "-std=c++17".into(),
                    "-DFACTOR=2".into(),
                    "-Isrc".into(),
                    "src/main.cpp".into(),
                    "src/circle.cpp".into(),
                    "src/run_main.cpp".into(),
                    "-o".into(),
                    exe.to_string_lossy().into_owned(),
                ],
                &[],
                "c++ build",
            );
            require_ok(
                root,
                vec![exe.to_string_lossy().into_owned()],
                &[],
                "c++ run",
            )
            .trim()
            .to_string()
        }
        "python" => {
            require_ok(
                root,
                vec![
                    "python".into(),
                    "-m".into(),
                    "compileall".into(),
                    "-q".into(),
                    ".".into(),
                ],
                &[],
                "python compileall",
            );
            require_ok(
                root,
                vec![
                    "python".into(),
                    "-c".into(),
                    "from app import main; print(main(3))".into(),
                ],
                &[],
                "python run",
            )
            .trim()
            .to_string()
        }
        "typescript" => {
            let tsc = format!("{TOOLS}/lang-servers/node_modules/typescript/bin/tsc");
            require_ok(
                root,
                vec!["node".into(), tsc.clone(), "--noEmit".into(), "-p".into(), ".".into()],
                &[],
                "tsc noEmit",
            );
            require_ok(
                root,
                vec!["node".into(), tsc, "-p".into(), ".".into(), "--outDir".into(), "out".into()],
                &[],
                "tsc emit",
            );
            require_ok(
                root,
                vec![
                    "node".into(),
                    "-e".into(),
                    "console.log(require('./out/app.js').value)".into(),
                ],
                &[],
                "ts run",
            )
            .trim()
            .to_string()
        }
        "javascript" => {
            require_ok(
                root,
                vec!["node".into(), "--check".into(), "src/app.js".into()],
                &[],
                "node --check",
            );
            require_ok(
                root,
                vec![
                    "node".into(),
                    "-e".into(),
                    "import('./src/app.js').then((m) => console.log(m.value))".into(),
                ],
                &[],
                "node run",
            )
            .trim()
            .to_string()
        }
        "java" => {
            let jdk = format!("{TOOLS}/jdk25/jdk-25.0.5+8/bin");
            let out = temp.join("classes");
            fs::create_dir_all(&out).unwrap();
            let mut argv = vec![
                format!("{jdk}/javac.exe"),
                "-d".into(),
                out.to_string_lossy().into_owned(),
            ];
            for entry in fs::read_dir(root.join("src/main/java/pkg")).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().and_then(|value| value.to_str()) == Some("java") {
                    argv.push(path.to_string_lossy().into_owned());
                }
            }
            require_ok(
                root,
                argv,
                &[("JAVA_HOME".into(), format!("{TOOLS}/jdk25/jdk-25.0.5+8"))],
                "javac",
            );
            "compiled".to_string()
        }
        "csharp" => {
            let dotnet = format!("{TOOLS}/dotnet8/dotnet.exe");
            let env = csharp_env(root);
            require_ok(
                root,
                vec![dotnet.clone(), "build".into(), "-nologo".into(), "-v:q".into()],
                &env,
                "dotnet build",
            );
            require_ok(
                root,
                vec![dotnet, "bin/Debug/net8.0/LangPack.dll".into()],
                &env,
                "dotnet run",
            )
            .trim()
            .to_string()
        }
        "go" => {
            let go = format!("{TOOLS}/go/go/bin/go.exe");
            let exe = temp.join("app.exe");
            let env = go_env();
            require_ok(
                root,
                vec![
                    go.clone(),
                    "build".into(),
                    "-o".into(),
                    exe.to_string_lossy().into_owned(),
                    ".".into(),
                ],
                &env,
                "go build",
            );
            require_ok(
                root,
                vec![exe.to_string_lossy().into_owned()],
                &env,
                "go run",
            )
            .trim()
            .to_string()
        }
        "rust" => {
            let mut env = vec![(
                "RUSTUP_TOOLCHAIN".to_string(),
                "1.94.1-x86_64-pc-windows-gnu".to_string(),
            )];
            env.push((
                "LIBCLANG_PATH".to_string(),
                format!("{TOOLS}/pylibclang/clang/native"),
            ));
            env.push((
                "BINDGEN_EXTRA_CLANG_ARGS".to_string(),
                format!("-isystem {TOOLS}/llvm18/lib/clang/18/include"),
            ));
            env.push(("CXXFLAGS".to_string(), "-include cstdint".to_string()));
            require_ok(
                root,
                vec!["cargo".into(), "check".into(), "--quiet".into(), "--offline".into()],
                &env,
                "cargo check",
            );
            require_ok(
                root,
                vec![
                    "cargo".into(),
                    "run".into(),
                    "--quiet".into(),
                    "--offline".into(),
                    "--example".into(),
                    "run".into(),
                ],
                &env,
                "cargo run",
            )
            .trim()
            .to_string()
        }
        other => panic!("unknown build kind {other}"),
    }
}


fn baseline_build(kind: &str, root: &Path, label: &str) -> String {
    let dir = temp_dir(&format!("{label}-baseline"));
    let copy = dir.path().join("repo");
    copy_dir(root, &copy);
    build_fixture(kind, &copy, dir.path())
}

fn csharp_env(root: &Path) -> Vec<(String, String)> {
    vec![
        ("DOTNET_ROOT".into(), format!("{TOOLS}/dotnet8")),
        (
            "NUGET_PACKAGES".into(),
            "<local-user>/.nuget/packages".into(),
        ),
        ("DOTNET_CLI_TELEMETRY_OPTOUT".into(), "1".into()),
        ("DOTNET_NOLOGO".into(), "1".into()),
        (
            "DOTNET_CLI_HOME".into(),
            root.to_string_lossy().into_owned(),
        ),
        ("HOMEDRIVE".into(), "C:".into()),
        ("HOMEPATH".into(), "<local-user>".into()),
        ("ProgramFiles".into(), "C:/Program Files".into()),
        ("ProgramFiles(x86)".into(), "C:/Program Files (x86)".into()),
        ("PROCESSOR_ARCHITECTURE".into(), "AMD64".into()),
        ("COMPUTERNAME".into(), "LOCALHOST".into()),
    ]
}

fn go_env() -> Vec<(String, String)> {
    let go = format!("{TOOLS}/go");
    vec![
        ("GOROOT".into(), format!("{go}/go")),
        ("GOPATH".into(), format!("{go}/gopath")),
        ("GOMODCACHE".into(), format!("{go}/gopath/pkg/mod")),
        ("GOTOOLCHAIN".into(), "local".into()),
        ("CGO_ENABLED".into(), "0".into()),
    ]
}

fn commit_rename(
    fixture: &mut common::Fixture,
    label: &str,
    tool: &str,
    allowed: &[&str],
    edit: &Value,
) -> Value {
    let plan = common::approved_plan(fixture, &format!("plan.{label}"), allowed);
    let changeset_id = format!("changeset.{label}");
    common::create_changeset(fixture, &changeset_id, plan["id"].as_str().unwrap(), &[]);
    let seed = common::prepare(fixture, &changeset_id, 1, None, "seed").unwrap();
    let staging = common::staging_path(fixture, &seed);
    let allowed_strings: Vec<String> = allowed.iter().map(|value| value.to_string()).collect();
    let files = apply_workspace_edit(
        &fixture.store,
        &PreviewRequest {
            project_id: &fixture.project,
            snapshot_id: &fixture.snapshot_id,
            allowed_paths: &allowed_strings,
            max_files: 32,
            workspace_root: Some(&fixture.root),
        },
        edit,
    )
    .unwrap();
    for (path, bytes) in &files {
        let target = staging.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, bytes).unwrap();
    }
    let transform = common::transform(fixture, "structured_codemod", json!([]));
    let prepared = common::prepare(
        fixture,
        &changeset_id,
        seed["revision"].as_i64().unwrap(),
        Some(transform),
        "rename",
    )
    .unwrap();
    let approval = common::approve_prepared(
        fixture,
        &changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
    );
    let applied = common::apply(
        fixture,
        &changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
        approval["approval"]["approval_id"].as_str().unwrap(),
        &format!("apply-{label}"),
    )
    .unwrap();
    assert_eq!(applied["state"], "applied", "{applied}");
    for (path, bytes) in &files {
        assert_eq!(
            fs::read(fixture.root.join(path)).unwrap(),
            *bytes,
            "{path} did not match the approved staged content"
        );
    }
    json!({
        "tool": tool,
        "plan_id": plan["id"],
        "changeset_id": changeset_id,
        "prepared_digest": prepared["prepared_digest"],
        "approval_id": approval["approval"]["approval_id"],
        "changed_files": files.keys().cloned().collect::<Vec<_>>(),
        "state": applied["state"],
        "staged_only_until_apply": true,
    })
}

fn assert_unchanged(root: &Path, path: &str, needle: &str) {
    let text = fs::read_to_string(root.join(path)).unwrap();
    assert!(text.contains(needle), "{path} lost {needle}");
}

#[test]
fn seven_groups_apply_semantic_renames_through_the_changeset_gate() {
    let mut evidence = Map::new();
    let csharp_available = Path::new(&format!("{TOOLS}/csharp-tools/csharp-ls.exe")).is_file();
    let jdtls_available = Path::new(&format!("{TOOLS}/jdtls/plugins")).is_dir();

    // ---------------- C/C++ ----------------
    {
        let source = fixtures().join("c-cpp");
        let mut fixture = common::setup_prepared("b030-rename-cpp", |root| {
            copy_dir(&source, root);
            fs::write(
                root.join("src/run_main.cpp"),
                "#include \"shape.h\"\n#include <cstdio>\ndouble measure(const Shape&);\nint main(){ Circle circle(2.0); std::printf(\"%.2f %d\\n\", measure(circle), static_cast<int>(scale_value(3.0))); return 0; }\n",
            )
            .unwrap();
        });
        let before = baseline_build("cpp", &fixture.root, "cpp");
        let clangd = PathBuf::from(format!("{TOOLS}/llvm18/bin/clangd.exe"));
        let session = LspSession::start(
            &LspServerSpec {
                id: "clangd".into(),
                program: clangd,
                args: vec![
                    format!(
                        "--compile-commands-dir={}",
                        fixture.root.join("build-a").to_string_lossy()
                    ),
                    "--background-index=false".into(),
                ],
                cwd: fixture.root.clone(),
                env: Vec::new(),
                initialization_options: None,
            },
            &SpawnPermission::grant("grant.b030.cpp", "rename apply acceptance"),
            &PathBuf::from(format!("{TEMP}/b030-rename-cpp-audit.ndjson")),
        )
        .unwrap();
        session.initialize(&fixture.root).unwrap();
        for path in ["src/circle.cpp", "src/main.cpp", "src/shape.h", "src/run_main.cpp"] {
            let text = fs::read_to_string(fixture.root.join(path)).unwrap();
            session.did_open(&fixture.root.join(path), "cpp", &text).unwrap();
        }
        let edit = rename_edit(&session, &fixture.root.join("src/circle.cpp"), "area", "surface", 90, &["shape.h", "circle.cpp", "main.cpp"]);
        let summary = session.shutdown().unwrap();
        assert!(summary.orphan_pids.is_empty());
        let applied = commit_rename(
            &mut fixture,
            "cpp",
            "clangd 18.1.8",
            &["src/shape.h", "src/circle.cpp", "src/main.cpp", "src/run_main.cpp"],
            &edit,
        );
        assert_unchanged(&fixture.root, "src/shape.h", "surface");
        let after = build_fixture("cpp", &fixture.root, temp_dir("cpp-after").path());
        assert_eq!(before, after);
        evidence.insert(
            "c_cpp".into(),
            json!({"applied": applied, "before_output": before, "after_output": after}),
        );
    }

    // ---------------- Python ----------------
    {
        let source = fixtures().join("python");
        let mut fixture = common::setup_prepared("b030-rename-python", |root| copy_dir(&source, root));
        let before = baseline_build("python", &fixture.root, "py");
        let session = LspSession::start(
            &LspServerSpec {
                id: "pyright".into(),
                program: PathBuf::from("C:/Program Files/nodejs/node.exe"),
                args: vec![
                    format!("{TOOLS}/lang-servers/node_modules/pyright/langserver.index.js"),
                    "--stdio".into(),
                ],
                cwd: fixture.root.clone(),
                env: Vec::new(),
                initialization_options: None,
            },
            &SpawnPermission::grant("grant.b030.python", "rename apply acceptance"),
            &PathBuf::from(format!("{TEMP}/b030-rename-python-audit.ndjson")),
        )
        .unwrap();
        session.initialize(&fixture.root).unwrap();
        for (path, language) in [("app.py", "python"), ("pkg/util.py", "python"), ("pkg/other.py", "python")] {
            let text = fs::read_to_string(fixture.root.join(path)).unwrap();
            session.did_open(&fixture.root.join(path), language, &text).unwrap();
        }
        let edit = rename_edit(&session, &fixture.root.join("pkg/util.py"), "helper", "assist", 25, &["app.py", "util.py"]);
        let summary = session.shutdown().unwrap();
        assert!(summary.orphan_pids.is_empty());
        let applied = commit_rename(
            &mut fixture,
            "python",
            "pyright 1.1.408",
            &["app.py", "pkg/util.py"],
            &edit,
        );
        assert_unchanged(&fixture.root, "pkg/other.py", "def helper");
        let after = build_fixture("python", &fixture.root, temp_dir("py-after").path());
        assert_eq!(before, after);
        evidence.insert("python".into(), json!({"applied": applied, "before_output": before, "after_output": after}));
    }

    // ---------------- TypeScript / JavaScript ----------------
    {
        let source = fixtures().join("typescript");
        let mut fixture = common::setup_prepared("b030-rename-ts", |root| copy_dir(&source, root));
        let before = baseline_build("typescript", &fixture.root, "ts");
        let session = LspSession::start(
            &LspServerSpec {
                id: "typescript-language-server".into(),
                program: PathBuf::from("C:/Program Files/nodejs/node.exe"),
                args: vec![
                    format!("{TOOLS}/lang-servers/node_modules/typescript-language-server/lib/cli.mjs"),
                    "--stdio".into(),
                ],
                cwd: fixture.root.clone(),
                env: Vec::new(),
                initialization_options: None,
            },
            &SpawnPermission::grant("grant.b030.typescript", "rename apply acceptance"),
            &PathBuf::from(format!("{TEMP}/b030-rename-ts-audit.ndjson")),
        )
        .unwrap();
        session.initialize(&fixture.root).unwrap();
        for path in ["src/app.ts", "src/util.ts", "src/other.ts"] {
            let text = fs::read_to_string(fixture.root.join(path)).unwrap();
            session.did_open(&fixture.root.join(path), "typescript", &text).unwrap();
        }
        let edit = rename_edit(&session, &fixture.root.join("src/util.ts"), "helper", "assist", 25, &["app.ts", "util.ts"]);
        let summary = session.shutdown().unwrap();
        assert!(summary.orphan_pids.is_empty());
        let applied = commit_rename(
            &mut fixture,
            "typescript",
            "typescript-language-server 4.3.4",
            &["src/util.ts", "src/app.ts"],
            &edit,
        );
        assert_unchanged(&fixture.root, "src/other.ts", "export function helper");
        let after = build_fixture("typescript", &fixture.root, temp_dir("ts-after").path());
        assert_eq!(before, after);
        evidence.insert(
            "javascript_typescript".into(),
            json!({"applied": applied, "before_output": before, "after_output": after}),
        );
    }

    // ---------------- JavaScript ----------------
    {
        let source = fixtures().join("javascript");
        let mut fixture = common::setup_prepared("b030-rename-js", |root| copy_dir(&source, root));
        let before = baseline_build("javascript", &fixture.root, "js");
        let session = LspSession::start(
            &LspServerSpec {
                id: "typescript-language-server".into(),
                program: PathBuf::from("C:/Program Files/nodejs/node.exe"),
                args: vec![
                    format!("{TOOLS}/lang-servers/node_modules/typescript-language-server/lib/cli.mjs"),
                    "--stdio".into(),
                ],
                cwd: fixture.root.clone(),
                env: Vec::new(),
                initialization_options: None,
            },
            &SpawnPermission::grant("grant.b030.javascript", "rename apply acceptance"),
            &PathBuf::from(format!("{TEMP}/b030-rename-js-audit.ndjson")),
        )
        .unwrap();
        session.initialize(&fixture.root).unwrap();
        for path in ["src/app.js", "src/util.js", "src/other.js"] {
            let text = fs::read_to_string(fixture.root.join(path)).unwrap();
            session.did_open(&fixture.root.join(path), "javascript", &text).unwrap();
        }
        let edit = rename_edit(&session, &fixture.root.join("src/util.js"), "helper", "assist", 25, &["app.js", "util.js"]);
        let summary = session.shutdown().unwrap();
        assert!(summary.orphan_pids.is_empty());
        let applied = commit_rename(
            &mut fixture,
            "javascript",
            "typescript-language-server 4.3.4",
            &["src/util.js", "src/app.js"],
            &edit,
        );
        assert_unchanged(&fixture.root, "src/other.js", "export function helper");
        let after = build_fixture("javascript", &fixture.root, temp_dir("js-after").path());
        assert_eq!(before, after);
        if let Some(group) = evidence
            .get_mut("javascript_typescript")
            .and_then(Value::as_object_mut)
        {
            group.insert(
                "javascript".into(),
                json!({"applied": applied, "before_output": before, "after_output": after}),
            );
        }
    }

    // ---------------- Java ----------------
    assert!(jdtls_available, "jdtls is required for the Java rename gate");
    {
        let source = fixtures().join("java");
        let mut fixture = common::setup_prepared("b030-rename-java", |root| copy_dir(&source, root));
        let before = baseline_build("java", &fixture.root, "java");
        let jdk = PathBuf::from(format!("{TOOLS}/jdk25/jdk-25.0.5+8/bin/java.exe"));
        let jdtls = PathBuf::from(format!("{TOOLS}/jdtls"));
        let launcher = fs::read_dir(jdtls.join("plugins"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("org.eclipse.equinox.launcher_")
            })
            .unwrap();
        let cfg = jdtls.join("config_win");
        let mut args = vec![
            "-Declipse.application=org.eclipse.jdt.ls.core.id1".into(),
            "-Declipse.product=org.eclipse.jdt.ls.core.product".into(),
            "-Dosgi.bundles.defaultStartLevel=4".into(),
            format!("-Dosgi.sharedConfiguration.area={}", cfg.display()),
            "-Dosgi.sharedConfiguration.area.readOnly=true".into(),
            "-Dosgi.configuration.cascaded=true".into(),
            "-noverify".into(),
            "-Xms512m".into(),
            "-Xmx1024m".into(),
            "--add-modules=ALL-SYSTEM".into(),
        ];
        for module in [
            "java.util", "java.lang", "java.nio", "java.io", "java.net", "java.text",
            "java.lang.reflect", "java.lang.invoke",
        ] {
            args.push(format!("--add-opens=java.base/{module}=ALL-UNNAMED"));
        }
        args.push("--add-opens=java.base/java.util.concurrent=ALL-UNNAMED".into());
        args.push("--add-opens=java.base/java.util.concurrent.atomic=ALL-UNNAMED".into());
        args.push("--add-opens=java.base/sun.nio.ch=ALL-UNNAMED".into());
        args.push("-jar".into());
        args.push(launcher.to_string_lossy().into());
        args.push("-configuration".into());
        args.push(cfg.to_string_lossy().into());
        args.push("-data".into());
        args.push(
            fixture
                .root
                .join("jdt-data")
                .to_string_lossy()
                .into_owned(),
        );
        let session = LspSession::start(
            &LspServerSpec {
                id: "jdtls".into(),
                program: jdk,
                args,
                cwd: fixture.root.clone(),
                env: vec![],
                initialization_options: None,
            },
            &SpawnPermission::grant("grant.b030.java", "rename apply acceptance"),
            &PathBuf::from(format!("{TEMP}/b030-rename-java-audit.ndjson")),
        )
        .unwrap();
        session.initialize(&fixture.root).unwrap();
        for path in [
            "src/main/java/pkg/Util.java",
            "src/main/java/pkg/Other.java",
            "src/main/java/pkg/App.java",
            "src/main/java/pkg/Marker.java",
            "src/main/java/pkg/Transformer.java",
            "src/main/java/pkg/Doubler.java",
        ] {
            let text = fs::read_to_string(fixture.root.join(path)).unwrap();
            session.did_open(&fixture.root.join(path), "java", &text).unwrap();
        }
        let edit = rename_edit(
            &session,
            &fixture.root.join("src/main/java/pkg/Util.java"),
            "helper",
            "assist",
            40,
            &["App.java", "Util.java"],
        );
        let summary = session.shutdown().unwrap();
        assert!(summary.orphan_pids.is_empty());
        let applied = commit_rename(
            &mut fixture,
            "java",
            "jdtls 1.64.0",
            &["src/main/java/pkg/Util.java", "src/main/java/pkg/App.java"],
            &edit,
        );
        assert_unchanged(&fixture.root, "src/main/java/pkg/Other.java", "helper");
        assert_unchanged(&fixture.root, "src/main/java/pkg/App.java", "\"helper\"");
        let after = build_fixture("java", &fixture.root, temp_dir("java-after").path());
        assert_eq!(before, after);
        evidence.insert("java".into(), json!({"applied": applied, "before_output": before, "after_output": after}));
    }

    // ---------------- C# ----------------
    assert!(csharp_available, "csharp-ls is required for the C# rename gate");
    {
        let source = fixtures().join("csharp");
        let mut fixture = common::setup_prepared("b030-rename-csharp", |root| copy_dir(&source, root));
        let before = baseline_build("csharp", &fixture.root, "cs");
        let dn = format!("{TOOLS}/dotnet8");
        let session = LspSession::start(
            &LspServerSpec {
                id: "csharp-ls".into(),
                program: PathBuf::from(format!("{TOOLS}/csharp-tools/csharp-ls.exe")),
                args: vec![],
                cwd: fixture.root.clone(),
                env: vec![
                    ("DOTNET_ROOT".into(), dn.clone()),
                    (
                        "PATH".into(),
                        format!("{dn};{}", std::env::var("PATH").unwrap_or_default()),
                    ),
                ],
                initialization_options: None,
            },
            &SpawnPermission::grant("grant.b030.csharp", "rename apply acceptance"),
            &PathBuf::from(format!("{TEMP}/b030-rename-csharp-audit.ndjson")),
        )
        .unwrap();
        session.initialize(&fixture.root).unwrap();
        for path in ["Util.cs", "Other.cs", "Program.cs"] {
            let text = fs::read_to_string(fixture.root.join(path)).unwrap();
            session.did_open(&fixture.root.join(path), "csharp", &text).unwrap();
        }
        let edit = rename_edit(&session, &fixture.root.join("Util.cs"), "Helper", "Assist", 35, &["Program.cs", "Util.cs"]);
        let summary = session.shutdown().unwrap();
        assert!(summary.orphan_pids.is_empty());
        let applied = commit_rename(
            &mut fixture,
            "csharp",
            "csharp-ls 0.14.0",
            &["Util.cs", "Program.cs"],
            &edit,
        );
        assert_unchanged(&fixture.root, "Other.cs", "Helper");
        let after = build_fixture("csharp", &fixture.root, temp_dir("cs-after").path());
        assert_eq!(before, after);
        evidence.insert("csharp".into(), json!({"applied": applied, "before_output": before, "after_output": after}));
    }

    // ---------------- Go ----------------
    {
        let source = fixtures().join("go");
        let mut fixture = common::setup_prepared("b030-rename-go", |root| copy_dir(&source, root));
        let before = baseline_build("go", &fixture.root, "go");
        let go = format!("{TOOLS}/go");
        let session = LspSession::start(
            &LspServerSpec {
                id: "gopls".into(),
                program: PathBuf::from(format!("{go}/bin/gopls.exe")),
                args: vec![],
                cwd: fixture.root.clone(),
                env: vec![
                    (
                        "PATH".into(),
                        format!("{go}/go/bin;{}", std::env::var("PATH").unwrap_or_default()),
                    ),
                    ("GOROOT".into(), format!("{go}/go")),
                    ("GOPATH".into(), format!("{go}/gopath")),
                    ("GOMODCACHE".into(), format!("{go}/gopath/pkg/mod")),
                ],
                initialization_options: None,
            },
            &SpawnPermission::grant("grant.b030.go", "rename apply acceptance"),
            &PathBuf::from(format!("{TEMP}/b030-rename-go-audit.ndjson")),
        )
        .unwrap();
        session.initialize(&fixture.root).unwrap();
        for path in ["main.go", "util/util.go", "other/other.go"] {
            let text = fs::read_to_string(fixture.root.join(path)).unwrap();
            session.did_open(&fixture.root.join(path), "go", &text).unwrap();
        }
        let edit = rename_edit(&session, &fixture.root.join("util/util.go"), "Helper", "Assist", 25, &["main.go", "util.go"]);
        let summary = session.shutdown().unwrap();
        assert!(summary.orphan_pids.is_empty());
        let applied = commit_rename(
            &mut fixture,
            "go",
            "gopls 0.20.0",
            &["main.go", "util/util.go"],
            &edit,
        );
        assert_unchanged(&fixture.root, "other/other.go", "func Helper");
        let after = build_fixture("go", &fixture.root, temp_dir("go-after").path());
        assert_eq!(before, after);
        evidence.insert("go".into(), json!({"applied": applied, "before_output": before, "after_output": after}));
    }

    // ---------------- Rust ----------------
    {
        let source = fixtures().join("rust");
        let mut fixture = common::setup_prepared("b030-rename-rust", |root| {
            copy_dir(&source, root);
            fs::create_dir_all(root.join("examples")).unwrap();
            fs::write(
                root.join("examples/run.rs"),
                "fn main() { println!(\"{}\", langpack_rust::scaled()); }\n",
            )
            .unwrap();
        });
        let before = baseline_build("rust", &fixture.root, "rust");
        let session = LspSession::start(
            &LspServerSpec {
                id: "rust-analyzer".into(),
                program: PathBuf::from("<local-user>/.rustup/toolchains/1.94.1-x86_64-pc-windows-gnu/bin/rust-analyzer.exe"),
                args: vec![],
                cwd: fixture.root.clone(),
                env: Vec::new(),
                initialization_options: None,
            },
            &SpawnPermission::grant("grant.b030.rust", "rename apply acceptance"),
            &PathBuf::from(format!("{TEMP}/b030-rename-rust-audit.ndjson")),
        )
        .unwrap();
        session.initialize(&fixture.root).unwrap();
        for path in ["src/lib.rs", "src/util.rs", "src/other.rs", "examples/run.rs"] {
            let text = fs::read_to_string(fixture.root.join(path)).unwrap();
            session.did_open(&fixture.root.join(path), "rust", &text).unwrap();
        }
        let edit = rename_edit(&session, &fixture.root.join("src/util.rs"), "helper", "assist", 150, &["lib.rs", "util.rs"]);
        let summary = session.shutdown().unwrap();
        assert!(summary.orphan_pids.is_empty());
        let applied = commit_rename(
            &mut fixture,
            "rust",
            "rust-analyzer 1.94.1",
            &["src/util.rs", "src/lib.rs"],
            &edit,
        );
        assert_unchanged(&fixture.root, "src/other.rs", "pub fn helper");
        let after = build_fixture("rust", &fixture.root, temp_dir("rust-after").path());
        assert_eq!(before, after);
        evidence.insert("rust".into(), json!({"applied": applied, "before_output": before, "after_output": after}));
    }

    let payload = json!({
        "schema_version": "1.0",
        "generated_by": "crates/archmeld-core/tests/language_rename_apply.rs",
        "platform": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "groups": evidence,
        "note": "Each rename came from a real language server, was applied to the staging tree only, approved by exact prepared digest, applied through the journaled writer gate, then built and executed again. Same-named symbols in other files and reflective/string literals remained untouched.",
    });
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        for path in [
            "docs/release-evidence/B030-rename-apply.json",
            "docs/evidence/B030-rename-apply.json",
        ] {
            let full = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, serde_json::to_string_pretty(&payload).unwrap() + "\n").unwrap();
        }
    }
}
