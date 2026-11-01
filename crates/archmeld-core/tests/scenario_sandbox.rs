// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Scenario sandbox modes (B028): snapshot-copy execution, declaration
//! refusal and real OCI container isolation/cleanup.

use archmeld_core::scenario::{run_scenario, ExecutionGrant, ScenarioSandbox, ScenarioSpec};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const P: &str = "project.scenario.sandbox";

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
    snapshot_id: String,
    config_digest: String,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("src/app.py"), "MARK = 'snapshot'\n").unwrap();
    fs::write(root.join("config/secret.txt"), "secret\n").unwrap();
    let state = dir.path().join("state");
    let (snapshot_id, config_digest) = {
        let mut store = Store::open(&state).unwrap();
        store.ensure_project(P, "expedition").unwrap();
        let snapshot = capture_snapshot(
            &mut store,
            P,
            &root,
            &SnapshotInputs {
                build_config: json!({"variant": "scenario-sandbox"}),
                environment: json!({"os": "test"}),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        (
            snapshot.snapshot_id.clone(),
            snapshot.snapshot["config_digest"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    };
    Fixture {
        _dir: dir,
        root,
        state,
        snapshot_id,
        config_digest,
    }
}

fn spec(
    fixture: &Fixture,
    run_id: &str,
    argv: Vec<String>,
    timeout: Duration,
    sandbox: Option<ScenarioSandbox>,
) -> ScenarioSpec {
    ScenarioSpec {
        project_id: P.into(),
        run_id: run_id.into(),
        snapshot_id: fixture.snapshot_id.clone(),
        config_digest: fixture.config_digest.clone(),
        argv,
        cwd: fixture.root.clone(),
        env: vec![],
        timeout,
        sandbox,
    }
}

fn read_stdout(store: &Store, manifest: &Value) -> String {
    let digest = manifest["stdout"]["digest"].as_str().unwrap();
    String::from_utf8_lossy(&store.read_blob_bytes(digest).unwrap().unwrap()).to_string()
}

fn container(image: String, cwd: Option<&str>) -> ScenarioSandbox {
    ScenarioSandbox {
        mode: "container".into(),
        include: vec!["src/app.py".into()],
        exclude: vec![],
        image: Some(image),
        network: Some("none".into()),
        cwd: cwd.map(str::to_string),
    }
}

#[test]
fn snapshot_copy_ignores_live_workspace_changes() {
    let fixture = setup();
    fs::write(fixture.root.join("src/app.py"), "MARK = 'live'\n").unwrap();
    fs::write(fixture.root.join("src/live_only.txt"), "live\n").unwrap();
    let script = concat!(
        "import pathlib\n",
        "assert pathlib.Path('app.py').read_text().strip() == \"MARK = 'snapshot'\"\n",
        "assert not pathlib.Path('live_only.txt').exists()\n",
        "assert not pathlib.Path('../config/secret.txt').exists()\n",
        "pathlib.Path('in_copy.txt').write_text('written')\n",
        "print('copy-ok')\n",
    );
    let mut store = Store::open(&fixture.state).unwrap();
    let manifest = run_scenario(
        &mut store,
        &spec(
            &fixture,
            "run.copy",
            vec!["python".into(), "-c".into(), script.into()],
            Duration::from_secs(30),
            Some(ScenarioSandbox {
                mode: "workspace_copy".into(),
                include: vec!["src/app.py".into()],
                exclude: vec![],
                image: None,
                network: None,
                cwd: Some("src".into()),
            }),
        ),
        &ExecutionGrant::grant("g.copy", "snapshot copy test"),
    )
    .unwrap();
    assert_eq!(manifest["exit_code"], 0, "{manifest}");
    let sandbox = &manifest["sandbox"];
    assert_eq!(sandbox["mode"], "workspace_copy");
    assert_eq!(sandbox["isolated"], true);
    assert_eq!(sandbox["enforcement"], "materialized_snapshot_copy");
    assert_eq!(sandbox["file_count"], 1);
    assert_eq!(sandbox["retained"], false);
    assert!(!Path::new(sandbox["root"].as_str().unwrap()).exists());
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/app.py")).unwrap(),
        "MARK = 'live'\n"
    );
    assert!(!fixture.root.join("src/in_copy.txt").exists());
    assert!(read_stdout(&store, &manifest).contains("copy-ok"));
    assert!(store.integrity_check().unwrap().is_empty());
}

#[test]
fn sandbox_declarations_that_cannot_be_enforced_are_rejected() {
    let fixture = setup();
    let mut store = Store::open(&fixture.state).unwrap();
    let pinned = format!("debian@sha256:{}", "a".repeat(64));
    let cases: Vec<(ScenarioSandbox, &str)> = vec![
        (
            ScenarioSandbox {
                mode: "container".into(),
                include: vec!["src/app.py".into()],
                exclude: vec![],
                image: Some("debian:latest".into()),
                network: None,
                cwd: None,
            },
            "digest-pinned",
        ),
        (
            ScenarioSandbox {
                mode: "container".into(),
                include: vec![],
                exclude: vec![],
                image: Some(pinned.clone()),
                network: None,
                cwd: None,
            },
            "include must not be empty",
        ),
        (
            ScenarioSandbox {
                mode: "container".into(),
                include: vec!["src/app.py".into()],
                exclude: vec![],
                image: Some(pinned),
                network: Some("host".into()),
                cwd: None,
            },
            "network=none",
        ),
        (
            ScenarioSandbox {
                mode: "workspace_copy".into(),
                include: vec![],
                exclude: vec![],
                image: None,
                network: None,
                cwd: None,
            },
            "include must not be empty",
        ),
        (
            ScenarioSandbox {
                mode: "workspace".into(),
                include: vec![],
                exclude: vec![],
                image: Some("sha256:00".into()),
                network: None,
                cwd: None,
            },
            "does not accept image/network",
        ),
        (
            ScenarioSandbox {
                mode: "workspace_copy".into(),
                include: vec!["src/app.py".into()],
                exclude: vec![],
                image: None,
                network: None,
                cwd: Some("../etc".into()),
            },
            "must not contain '..' segments",
        ),
        (
            ScenarioSandbox {
                mode: "bogus".into(),
                include: vec![],
                exclude: vec![],
                image: None,
                network: None,
                cwd: None,
            },
            "unsupported sandbox mode",
        ),
    ];
    for (sandbox, expected) in cases {
        let error = run_scenario(
            &mut store,
            &spec(
                &fixture,
                "run.invalid",
                vec!["python".into(), "-c".into(), "print(1)".into()],
                Duration::from_secs(5),
                Some(sandbox),
            ),
            &ExecutionGrant::grant("g.invalid", "declaration refusal"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains(expected),
            "expected {expected:?} in {error:?}"
        );
    }
    assert!(store.integrity_check().unwrap().is_empty());
}

#[test]
fn container_scenario_isolates_filesystem_and_network() {
    let Some(image) = std::env::var_os("ARCHMELD_CONTAINER_IMAGE") else {
        return;
    };
    let image = image.to_string_lossy().to_string();
    let fixture = setup();
    fs::write(fixture.root.join("src/app.py"), "MARK = 'live'\n").unwrap();
    fs::write(fixture.root.join("src/live_only.txt"), "live\n").unwrap();
    let script = concat!(
        "set -e\n",
        "test -f app.py\n",
        "grep -q snapshot app.py\n",
        "test ! -e live_only.txt\n",
        "test ! -e ../config/secret.txt\n",
        "test ${#SCENARIO_TOKEN} -eq 14\n",
        "echo tmp > /tmp/t\n",
        "echo out > /workspace/out.txt\n",
        "if echo x > /root/x 2>/dev/null; then exit 7; fi\n",
        "if grep -q eth0 /proc/net/dev; then exit 8; fi\n",
        "echo scenario-container-ok\n",
    );
    let mut store = Store::open(&fixture.state).unwrap();
    let mut scenario_spec = spec(
        &fixture,
        "run.container",
        vec!["sh".into(), "-c".into(), script.into()],
        Duration::from_secs(60),
        Some(container(image, Some("src"))),
    );
    scenario_spec.env = vec![("SCENARIO_TOKEN".into(), "sentinel-value".into())];
    let manifest = run_scenario(
        &mut store,
        &scenario_spec,
        &ExecutionGrant::grant("g.container", "container scenario test"),
    )
    .unwrap();
    assert_eq!(manifest["exit_code"], 0, "{manifest}");
    let sandbox = &manifest["sandbox"];
    assert_eq!(sandbox["mode"], "container");
    assert_eq!(sandbox["isolated"], true);
    assert_eq!(sandbox["enforcement"], "oci_container");
    assert_eq!(sandbox["network"], "none");
    assert_eq!(sandbox["read_only"], true);
    assert_eq!(sandbox["file_count"], 1);
    assert_eq!(sandbox["retained"], false);
    assert_eq!(sandbox["container"]["removed"], true);
    assert_eq!(sandbox["workdir"], "/workspace/src");
    assert!(manifest["env_names"]
        .as_array()
        .unwrap()
        .iter()
        .any(|name| name == "SCENARIO_TOKEN"));
    assert!(
        !manifest.to_string().contains("sentinel-value"),
        "container environment values must be redacted from the manifest: {manifest}"
    );
    assert!(sandbox["image_id"].as_str().unwrap().starts_with("sha256:"));
    assert!(!Path::new(sandbox["root"].as_str().unwrap()).exists());
    assert!(!fixture.root.join("out.txt").exists());
    assert!(!fixture.root.join("src/out.txt").exists());
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/app.py")).unwrap(),
        "MARK = 'live'\n"
    );
    assert!(read_stdout(&store, &manifest).contains("scenario-container-ok"));
    assert!(store.integrity_check().unwrap().is_empty());

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        write_evidence(json!({
            "case": "container_success",
            "state": manifest["completion"],
            "exit_code": manifest["exit_code"],
            "sandbox": sandbox,
            "stdout_digest": manifest["stdout"]["digest"],
            "workspace_app_after": fs::read_to_string(fixture.root.join("src/app.py")).unwrap(),
            "workspace_out_present": fixture.root.join("out.txt").exists()
                || fixture.root.join("src/out.txt").exists(),
        }));
    }
}

#[test]
fn container_scenario_is_removed_after_timeout() {
    let Some(image) = std::env::var_os("ARCHMELD_CONTAINER_IMAGE") else {
        return;
    };
    let image = image.to_string_lossy().to_string();
    let fixture = setup();
    let mut store = Store::open(&fixture.state).unwrap();
    let manifest = run_scenario(
        &mut store,
        &spec(
            &fixture,
            "run.container.timeout",
            vec!["sh".into(), "-c".into(), "sleep 30".into()],
            Duration::from_millis(700),
            Some(container(image, None)),
        ),
        &ExecutionGrant::grant("g.container.timeout", "container timeout cleanup"),
    )
    .unwrap();
    assert_eq!(manifest["timed_out"], true);
    assert_eq!(manifest["completion"], "timed_out");
    assert_eq!(manifest["sandbox"]["container"]["removed"], true);
    assert_eq!(manifest["sandbox"]["retained"], true);
    let root = Path::new(manifest["sandbox"]["root"].as_str().unwrap());
    assert!(root.exists(), "a timed-out run retains its copy for inspection");
    assert!(root.join("src/app.py").exists());
    assert!(store.integrity_check().unwrap().is_empty());

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        write_evidence(json!({
            "case": "container_timeout",
            "state": manifest["completion"],
            "timed_out": manifest["timed_out"],
            "sandbox": manifest["sandbox"],
            "retained_copy_present": root.exists(),
        }));
    }
}

fn write_evidence(case: Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/release-evidence/B028-scenario-container.json");
    let mut root: Value = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| {
            json!({
                "schema_version": "1.0",
                "generated_by": "crates/archmeld-core/tests/scenario_sandbox.rs",
                "platform": std::env::consts::OS,
                "cases": [],
            })
        });
    let case_name = case["case"].as_str().unwrap_or("").to_string();
    let cases = root["cases"].as_array_mut().unwrap();
    if let Some(existing) = cases.iter_mut().find(|item| item["case"] == case_name) {
        *existing = case;
    } else {
        cases.push(case);
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}
