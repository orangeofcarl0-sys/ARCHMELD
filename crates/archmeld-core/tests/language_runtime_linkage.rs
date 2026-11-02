// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B030 runtime linkage: every language group is executed for real, captured
//! with a native tool or an explicit in-process probe, normalized through the
//! shared adapter layer, imported into a bound snapshot and mapped back to the
//! exact source entities.
//!
//! This test does not upgrade static candidates. It only records spans and
//! observed runtime relations that came from the captured trace.

use archmeld_core::hash;
use archmeld_core::identity::line_entity_id;
use archmeld_core::runtime::{import_trace, RuntimeImport};
use archmeld_core::scenario::run_command_capture;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use archmeld_core::trace_adapters::{
    chrome_trace, jfr_execution_samples, pprof_raw, probe_jsonl, speedscope, v8_cpuprofile,
    TraceLocator, TraceMeta,
};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TOOLS: &str = "<workspace>/_archmeld_tools";
const TEMP: &str = "<workspace>/_archmeld_temp";
const PROJECT: &str = "project.b030.runtime";

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    snapshot_id: String,
    config_digest: String,
    payload: Value,
    entities: Vec<Value>,
}

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

fn fixture_root(language: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/languages/fixtures")
        .join(language)
}

fn setup(name: &str, files: &[(&str, &str)]) -> Fixture {
    let base = PathBuf::from(TEMP);
    let dir = tempfile::Builder::new()
        .prefix(&format!("b030-runtime-{name}-"))
        .tempdir_in(if base.is_dir() { base } else { std::env::temp_dir() })
        .unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    for (path, text) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    Fixture {
        _dir: dir,
        root,
        store,
        snapshot_id: String::new(),
        config_digest: String::new(),
        payload: Value::Null,
        entities: Vec::new(),
    }
}

impl Fixture {
    fn capture(&mut self, name: &str) {
        let snapshot = capture_snapshot(
            &mut self.store,
            PROJECT,
            &self.root,
            &SnapshotInputs {
                build_config: json!({"group": name, "variant": "runtime-linkage"}),
                environment: json!({"os": "windows", "arch": "x86_64"}),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        self.payload = self
            .store
            .snapshot_payload(PROJECT, &snapshot.snapshot_id)
            .unwrap()
            .unwrap();
        self.snapshot_id = snapshot.snapshot_id;
        self.config_digest = snapshot.snapshot["config_digest"]
            .as_str()
            .unwrap()
            .to_string();
    }

    fn file_digest(&self, path: &str) -> String {
        self.payload["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == path)
            .unwrap_or_else(|| panic!("snapshot is missing {path}"))["blob_digest"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn add_entity(
        &mut self,
        locator: &mut TraceLocator,
        path: &str,
        needle: &str,
        name: &str,
        language: &str,
        symbol: Option<&str>,
    ) -> String {
        let text = fs::read_to_string(self.root.join(path)).unwrap();
        let line = line_of(&text, needle);
        let (start, end) = function_range(&text, line);
        let digest = self.file_digest(path);
        let entity_id = line_entity_id(
            &digest,
            "symbol",
            start,
            end,
            &format!("{path}::{name}"),
        )
        .unwrap();
        self.entities.push(json!({
            "id": entity_id,
            "project_id": PROJECT,
            "snapshot_id": self.snapshot_id,
            "config_digest": self.config_digest,
            "kind": "symbol",
            "language": language,
            "qualified_name": format!("{path}::{name}"),
            "anchors": [{
                "path": path,
                "blob_digest": digest,
                "precision": "line_only",
                "byte_start": null,
                "byte_end": null,
                "line_start": start,
                "line_end": end,
                "encoding": "utf-8",
            }],
            "external": false,
            "backend_refs": [],
        }));
        locator.add_location(path, line, &entity_id);
        locator.add_range(path, start, end, &entity_id);
        let absolute = self.root.join(path).to_string_lossy().into_owned();
        locator.add_location(&absolute, line, &entity_id);
        locator.add_range(&absolute, start, end, &entity_id);
        if let Some(symbol) = symbol {
            locator.add_symbol(symbol, &entity_id);
        }
        entity_id
    }

    fn import_entities(&mut self, name: &str) {
        let count = self.entities.len();
        let batch = json!({
            "schema_version": "1.0",
            "project_id": PROJECT,
            "snapshot_id": self.snapshot_id,
            "config_digest": self.config_digest,
            "run_id": format!("run.entities.{name}"),
            "adapter_id": "b030-runtime-fixture",
            "adapter_version": "1",
            "sequence": "0",
            "final": true,
            "coverage": {
                "state": "complete",
                "scope": "runtime linkage fixture entities",
                "expected_count": count,
                "observed_count": count,
                "omitted_count": 0,
                "unresolved_count": 0,
                "limitations": [],
            },
            "entities": self.entities.clone(),
            "evidence": [],
            "relations": [],
            "diagnostics": [],
        });
        self.store.import_batch(&batch).unwrap();
    }

    fn record(
        &mut self,
        _locator: &TraceLocator,
        meta: &TraceMeta,
        trace: Value,
        raw: &[u8],
        media_type: &str,
        run_id: &str,
        adapter: &str,
    ) -> Value {
        let raw_digest = self.store.put_blob(raw, media_type).unwrap();
        let span_count = trace["spans"].as_array().map(Vec::len).unwrap_or(0);
        assert!(span_count > 0, "{adapter} produced no spans");
        let trace_digest = hash::digest_value(&trace).unwrap();
        let report = import_trace(
            &mut self.store,
            &RuntimeImport {
                project_id: PROJECT,
                run_id,
                snapshot_id: Some(&self.snapshot_id),
                config_digest: &self.config_digest,
                allow_unbound: false,
            scenario_run_id: None,
            },
            &trace,
        )
        .unwrap();
        let causal = &report["causal"];
        let mapped = causal["mapped_spans"].as_u64().unwrap_or(0);
        let unmapped = causal["unmapped_spans"].as_u64().unwrap_or(0);
        assert!(mapped >= 1, "{adapter} mapped no spans: {causal}");
        let observed = self
            .store
            .relation_payloads(PROJECT, &self.snapshot_id)
            .unwrap()
            .into_iter()
            .filter(|relation| relation["basis"] == "runtime")
            .count();
        json!({
            "status": "verified",
            "adapter": adapter,
            "capture_tool": meta.tool,
            "capture_version": meta.version,
            "scenario_id": meta.scenario_id,
            "clock_kind": meta.clock_kind,
            "lossy": meta.lossy,
            "loss_notes": meta.loss_notes,
            "raw_artifact_digest": raw_digest,
            "normalized_trace_digest": trace_digest,
            "spans": span_count,
            "mapped_spans": mapped,
            "unmapped_spans": unmapped,
            "unknown_entity": causal["unknown_entity"],
            "observed_relations": observed,
            "bound_snapshot_id": self.snapshot_id,
            "bound_config_digest": self.config_digest,
            "run_id": run_id,
        })
    }
}

fn line_of(text: &str, needle: &str) -> u64 {
    let index = text
        .find(needle)
        .unwrap_or_else(|| panic!("needle {needle:?} not found"));
    (text[..index].matches('\n').count() + 1) as u64
}

fn function_range(text: &str, declaration_line: u64) -> (u64, u64) {
    let lines: Vec<&str> = text.lines().collect();
    let index = declaration_line.saturating_sub(1) as usize;
    let declaration = lines.get(index).copied().unwrap_or("");
    let indent = declaration.len() - declaration.trim_start().len();
    let mut depth = 0i64;
    let mut seen = false;
    for (offset, line) in lines.iter().enumerate().skip(index) {
        for character in line.chars() {
            if character == '{' {
                depth += 1;
                seen = true;
            } else if character == '}' && seen {
                depth -= 1;
                if depth == 0 {
                    return (declaration_line, (offset + 1) as u64);
                }
            }
        }
        if !seen && offset > index + 5 {
            break;
        }
    }
    let mut end = declaration_line;
    for (offset, line) in lines.iter().enumerate().skip(index + 1) {
        if line.trim().is_empty() {
            continue;
        }
        let line_indent = line.len() - line.trim_start().len();
        if line_indent <= indent {
            break;
        }
        end = (offset + 1) as u64;
    }
    (declaration_line, end.max(declaration_line))
}

fn run(cwd: &Path, argv: Vec<String>, env: &[(String, String)], label: &str) -> (String, String) {
    let display = argv.clone();
    let capture = run_command_capture(&argv, cwd, env, Duration::from_secs(180), None)
        .unwrap_or_else(|error| panic!("{label} failed to start: {error}"));
    let stdout = String::from_utf8_lossy(&capture.stdout).to_string();
    let stderr = String::from_utf8_lossy(&capture.stderr).to_string();
    assert_eq!(
        capture.exit_code,
        Some(0),
        "{label} exit {:?}\nargv={display:?}\nstdout={stdout}\nstderr={stderr}",
        capture.exit_code
    );
    (stdout, stderr)
}

fn run_bytes(
    cwd: &Path,
    argv: Vec<String>,
    env: &[(String, String)],
    label: &str,
) -> (Vec<u8>, String) {
    let display = argv.clone();
    let capture = run_command_capture(&argv, cwd, env, Duration::from_secs(180), None)
        .unwrap_or_else(|error| panic!("{label} failed to start: {error}"));
    let stderr = String::from_utf8_lossy(&capture.stderr).to_string();
    assert_eq!(
        capture.exit_code,
        Some(0),
        "{label} exit {:?}\nargv={display:?}\nstderr={stderr}",
        capture.exit_code
    );
    (capture.stdout, stderr)
}

fn write_json_evidence(groups: &Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let matrix = json!({
        "schema_version": "1.0",
        "generated_by": "crates/archmeld-core/tests/language_runtime_linkage.rs",
        "platform": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "groups": groups,
        "note": "Each group was executed for real; the listed tool captured the trace. Sampling tools are lossy and say so. Source mapping uses declared entity ids or declared source ranges, never automatic name matching.",
    });
    for path in [
        "docs/release-evidence/B030-runtime-linkage.json",
        "docs/evidence/B030-runtime-linkage.json",
    ] {
        let full = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(
            &full,
            serde_json::to_string_pretty(&matrix).unwrap() + "\n",
        )
        .unwrap();
    }
}

#[test]
fn seven_language_groups_capture_and_import_real_native_traces() {
    let mut groups = serde_json::Map::new();

    // ---------------- Python / VizTracer ----------------
    {
        let python_fixture = fixture_root("python");
        let mut fixture = setup("python", &[]);
        copy_dir(&python_fixture, &fixture.root);
        fs::write(
            fixture.root.join("trace_harness.py"),
            "from app import main\n\n\ndef run():\n    total = 0\n    for _ in range(2000):\n        total += main(3)\n    return total\n\n\nif __name__ == \"__main__\":\n    print(run())\n",
        )
        .unwrap();
        fixture.capture("python");
        let mut locator = TraceLocator::new();
        fixture.add_entity(
            &mut locator,
            "app.py",
            "def main",
            "main",
            "python",
            None,
        );
        fixture.add_entity(
            &mut locator,
            "pkg/util.py",
            "def helper",
            "helper",
            "python",
            None,
        );
        fixture.add_entity(
            &mut locator,
            "trace_harness.py",
            "def run",
            "run",
            "python",
            None,
        );
        fixture.import_entities("python");
        let trace_path = fixture.root.join("trace.json");
        let (stdout, stderr) = run(
            &fixture.root,
            vec![
                "python".into(),
                "-m".into(),
                "viztracer".into(),
                "--output_file".into(),
                trace_path.to_string_lossy().into_owned(),
                "--".into(),
                "trace_harness.py".into(),
            ],
            &[(
                "PYTHONPATH".into(),
                format!("{TOOLS}/viztracer"),
            )],
            "viztracer",
        );
        let _ = (stdout, stderr);
        let raw = fs::read(&trace_path).unwrap();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        let meta = TraceMeta::new(
            &fixture.snapshot_id,
            &fixture.config_digest,
            json!({"kind": "python", "runtime": "cpython-3.11"}),
            "viztracer",
            "0.16.3",
        )
        .scenario("scenario.python.loop");
        let trace = chrome_trace(&value, &locator, &meta, true).unwrap();
        let report = fixture.record(
            &locator,
            &meta,
            trace,
            &raw,
            "application/json",
            "run.python.viztracer",
            "viztracer-chrome-json",
        );
        groups.insert("python".into(), report);
    }

    // ---------------- JavaScript / V8 CPU profile ----------------
    {
        let source_fixture = fixture_root("javascript");
        let mut fixture = setup("javascript", &[]);
        copy_dir(&source_fixture, &fixture.root);
        fs::write(
            fixture.root.join("src/trace.js"),
            "import { helper as h } from './util.js';\n\nexport function inner(n) {\n  let total = 0;\n  for (let i = 0; i < n; i++) total += h(1);\n  return total;\n}\n\nexport function run() { return inner(5000000); }\n",
        )
        .unwrap();
        fs::write(
            fixture.root.join("trace-harness.mjs"),
            "import { run } from './src/trace.js';\nconsole.log(run() > 0 ? 'ok' : 'fail');\n",
        )
        .unwrap();
        fixture.capture("javascript");
        let mut locator = TraceLocator::new();
        fixture.add_entity(
            &mut locator,
            "src/trace.js",
            "export function inner",
            "inner",
            "javascript",
            None,
        );
        fixture.add_entity(
            &mut locator,
            "src/trace.js",
            "export function run",
            "run",
            "javascript",
            None,
        );
        fixture.add_entity(
            &mut locator,
            "src/util.js",
            "export function helper",
            "helper",
            "javascript",
            None,
        );
        fixture.import_entities("javascript");
        let profile = fixture.root.join("js.cpuprofile");
        run(
            &fixture.root,
            vec![
                "node".into(),
                "--no-opt".into(),
                "--cpu-prof".into(),
                "--cpu-prof-dir".into(),
                fixture.root.to_string_lossy().into_owned(),
                "--cpu-prof-name".into(),
                "js.cpuprofile".into(),
                "trace-harness.mjs".into(),
            ],
            &[],
            "node js cpu-prof",
        );
        let raw = fs::read(&profile).unwrap();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        let meta = TraceMeta::new(
            &fixture.snapshot_id,
            &fixture.config_digest,
            json!({"kind": "javascript", "runtime": "node-24.14.1"}),
            "v8-cpu-profiler",
            "node 24.14.1",
        )
        .scenario("scenario.javascript.loop")
        .lossy(&[
            "V8 CPU sampling at about 1 ms; durations are sample deltas, not exact call durations",
            "optimized and inlined frames may be missing from the sample stack",
        ]);
        let trace = v8_cpuprofile(&value, &locator, &meta).unwrap();
        let report = fixture.record(
            &locator,
            &meta,
            trace,
            &raw,
            "application/json",
            "run.javascript.v8",
            "v8-cpuprofile",
        );
        groups.insert("javascript".into(), report);
    }

    // ---------------- TypeScript / V8 CPU profile ----------------
    {
        let source_fixture = fixture_root("typescript");
        let mut fixture = setup("typescript", &[]);
        copy_dir(&source_fixture, &fixture.root);
        fs::write(
            fixture.root.join("src/trace.ts"),
            "import { helper as h } from './util.ts';\n\nexport function inner(n: number): number {\n  let total = 0;\n  for (let i = 0; i < n; i++) total += h(1);\n  return total;\n}\n\nexport function run(): number { return inner(5000000); }\n",
        )
        .unwrap();
        fs::write(
            fixture.root.join("trace-harness.ts"),
            "import { run } from './src/trace.ts';\nconsole.log(run() > 0 ? 'ok' : 'fail');\n",
        )
        .unwrap();
        fixture.capture("typescript");
        let mut locator = TraceLocator::new();
        fixture.add_entity(
            &mut locator,
            "src/trace.ts",
            "export function inner",
            "inner",
            "typescript",
            None,
        );
        fixture.add_entity(
            &mut locator,
            "src/trace.ts",
            "export function run",
            "run",
            "typescript",
            None,
        );
        fixture.add_entity(
            &mut locator,
            "src/util.ts",
            "export function helper",
            "helper",
            "typescript",
            None,
        );
        fixture.import_entities("typescript");
        let profile = fixture.root.join("ts.cpuprofile");
        run(
            &fixture.root,
            vec![
                "node".into(),
                "--no-opt".into(),
                "--cpu-prof".into(),
                "--cpu-prof-dir".into(),
                fixture.root.to_string_lossy().into_owned(),
                "--cpu-prof-name".into(),
                "ts.cpuprofile".into(),
                "trace-harness.ts".into(),
            ],
            &[],
            "node ts cpu-prof",
        );
        let raw = fs::read(&profile).unwrap();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        let meta = TraceMeta::new(
            &fixture.snapshot_id,
            &fixture.config_digest,
            json!({"kind": "typescript", "runtime": "node-24.14.1-type-stripping"}),
            "v8-cpu-profiler",
            "node 24.14.1",
        )
        .scenario("scenario.typescript.loop")
        .lossy(&[
            "V8 CPU sampling at about 1 ms; durations are sample deltas, not exact call durations",
            "Node type stripping executes the TypeScript source directly; no emitted JavaScript was profiled",
        ]);
        let trace = v8_cpuprofile(&value, &locator, &meta).unwrap();
        let report = fixture.record(
            &locator,
            &meta,
            trace,
            &raw,
            "application/json",
            "run.typescript.v8",
            "v8-cpuprofile",
        );
        groups.insert("typescript".into(), report);
    }

    // ---------------- C/C++ explicit probe ----------------
    {
        let source_fixture = fixture_root("c-cpp");
        let mut fixture = setup("c-cpp", &[]);
        copy_dir(&source_fixture, &fixture.root);
        fs::write(
            fixture.root.join("src/trace_probe.h"),
            "#pragma once\n#include <chrono>\n#include <cstdio>\n#include <cstdlib>\n#include <string>\n#include <vector>\n#include <atomic>\n\nnamespace archmeld_probe {\ninline std::string next_id(const std::string& name) {\n    static std::atomic<long long> counter{0};\n    return name + \".\" + std::to_string(++counter);\n}\ninline long long now_ns() {\n    static const auto epoch = std::chrono::steady_clock::now();\n    return std::chrono::duration_cast<std::chrono::nanoseconds>(\n        std::chrono::steady_clock::now() - epoch).count();\n}\ninline std::vector<std::string>& stack() {\n    static thread_local std::vector<std::string> value;\n    return value;\n}\ninline std::string env(const char* key) {\n    const char* value = std::getenv(key);\n    return value ? value : \"\";\n}\nstruct Span {\n    std::string id, name, entity, parent;\n    long long start;\n    Span(std::string id_, std::string name_, std::string entity_)\n        : id(next_id(id_)), name(std::move(name_)), entity(std::move(entity_)), start(now_ns()) {\n        parent = stack().empty() ? \"\" : stack().back();\n        stack().push_back(id);\n    }\n    ~Span() {\n        long long end = now_ns();\n        stack().pop_back();\n        const char* path = std::getenv(\"ARCHMELD_TRACE_OUT\");\n        if (!path) return;\n        FILE* out = std::fopen(path, \"a\");\n        if (!out) return;\n        std::fprintf(out,\n            \"{\\\"span_id\\\":\\\"%s\\\",\\\"parent_span_id\\\":\\\"%s\\\",\\\"name\\\":\\\"%s\\\",\\\"kind\\\":\\\"function\\\",\\\"start_ns\\\":\\\"%lld\\\",\\\"end_ns\\\":\\\"%lld\\\",\\\"entity_id\\\":\\\"%s\\\"}\\n\",\n            id.c_str(), parent.c_str(), name.c_str(), start, end, entity.c_str());\n        std::fclose(out);\n    }\n};\n}\n",
        )
        .unwrap();
        fs::write(
            fixture.root.join("src/circle.cpp"),
            "#include \"shape.h\"\n#include \"trace_probe.h\"\n\nCircle::Circle(double radius) : radius_(radius) {}\n\ndouble Circle::area() const {\n    archmeld_probe::Span span(\"area\", \"Circle::area\", archmeld_probe::env(\"ARCHMELD_ENTITY_AREA\"));\n    return 3.14159 * radius_ * radius_;\n}\n",
        )
        .unwrap();
        fs::write(
            fixture.root.join("src/main.cpp"),
            "#include \"shape.h\"\n#include \"trace_probe.h\"\n\ndouble apply(double (*fn)(double), double value) {\n    archmeld_probe::Span span(\"apply\", \"apply\", archmeld_probe::env(\"ARCHMELD_ENTITY_APPLY\"));\n    return fn(value);\n}\n\ndouble scale_value(double value) {\n    archmeld_probe::Span span(\"scale\", \"scale_value\", archmeld_probe::env(\"ARCHMELD_ENTITY_SCALE\"));\n    return SCALE(value);\n}\n\ndouble measure(const Shape& shape) {\n    archmeld_probe::Span span(\"measure\", \"measure\", archmeld_probe::env(\"ARCHMELD_ENTITY_MEASURE\"));\n    return shape.area();\n}\n",
        )
        .unwrap();
        fs::write(
            fixture.root.join("src/trace_main.cpp"),
            "#include \"shape.h\"\n#include \"trace_probe.h\"\n#include <cstdio>\n\ndouble measure(const Shape&);\n\nint main() {\n    archmeld_probe::Span root(\"main\", \"main\", archmeld_probe::env(\"ARCHMELD_ENTITY_MAIN\"));\n    double total = 0;\n    for (int i = 0; i < 2000; i++) {\n        Circle circle((i % 5) + 1);\n        total += measure(circle);\n    }\n    for (int i = 0; i < 2000; i++) {\n        total += apply(scale_value, 2.0);\n    }\n    std::printf(\"%.2f\\n\", total);\n    return 0;\n}\n",
        )
        .unwrap();
        fixture.capture("c-cpp");
        let mut locator = TraceLocator::new();
        let area_entity = fixture.add_entity(
            &mut locator,
            "src/circle.cpp",
            "double Circle::area",
            "Circle::area",
            "cpp",
            None,
        );
        let apply_entity = fixture.add_entity(
            &mut locator,
            "src/main.cpp",
            "double apply",
            "apply",
            "cpp",
            None,
        );
        let scale_entity = fixture.add_entity(
            &mut locator,
            "src/main.cpp",
            "double scale_value",
            "scale_value",
            "cpp",
            None,
        );
        let measure_entity = fixture.add_entity(
            &mut locator,
            "src/main.cpp",
            "double measure",
            "measure",
            "cpp",
            None,
        );
        let main_entity = fixture.add_entity(
            &mut locator,
            "src/trace_main.cpp",
            "int main",
            "main",
            "cpp",
            None,
        );
        fixture.import_entities("c-cpp");
        let exe = fixture.root.join("trace.exe");
        run(
            &fixture.root,
            vec![
                "D:/Strawberry/c/bin/g++.exe".into(),
                "-std=c++17".into(),
                "-O1".into(),
                "-DFACTOR=2".into(),
                "-Isrc".into(),
                "src/main.cpp".into(),
                "src/circle.cpp".into(),
                "src/trace_main.cpp".into(),
                "-o".into(),
                exe.to_string_lossy().into_owned(),
            ],
            &[],
            "g++ probe build",
        );
        let trace_path = fixture.root.join("cpp-probe.jsonl");
        let env = vec![
            (
                "ARCHMELD_TRACE_OUT".to_string(),
                trace_path.to_string_lossy().into_owned(),
            ),
            ("ARCHMELD_ENTITY_MAIN".to_string(), main_entity),
            ("ARCHMELD_ENTITY_MEASURE".to_string(), measure_entity),
            ("ARCHMELD_ENTITY_AREA".to_string(), area_entity),
            ("ARCHMELD_ENTITY_APPLY".to_string(), apply_entity),
            ("ARCHMELD_ENTITY_SCALE".to_string(), scale_entity),
        ];
        run(
            &fixture.root,
            vec![exe.to_string_lossy().into_owned()],
            &env,
            "c++ probe run",
        );
        let raw = fs::read(&trace_path).unwrap();
        let text = String::from_utf8(raw.clone()).unwrap();
        let meta = TraceMeta::new(
            &fixture.snapshot_id,
            &fixture.config_digest,
            json!({"kind": "c++", "runtime": "mingw-g++", "instrumentation": "explicit-probe"}),
            "archmeld-explicit-probe",
            "cpp-1",
        )
        .scenario("scenario.c-cpp.loop");
        let trace = probe_jsonl(&text, &locator, &meta).unwrap();
        let report = fixture.record(
            &locator,
            &meta,
            trace,
            &raw,
            "application/x-ndjson",
            "run.c-cpp.probe",
            "explicit-probe-jsonl",
        );
        groups.insert("c_cpp".into(), report);
    }

    // ---------------- Java / JFR ----------------
    {
        let source_fixture = fixture_root("java");
        let mut fixture = setup("java", &[]);
        copy_dir(&source_fixture, &fixture.root);
        fs::write(
            fixture.root.join("src/main/java/pkg/TraceMain.java"),
            "package pkg;\n\npublic final class TraceMain {\n    public static void main(String[] args) {\n        long total = 0;\n        long end = System.nanoTime() + 1_500_000_000L;\n        while (System.nanoTime() < end) {\n            for (int i = 0; i < 20000; i++) total += Util.helper(i);\n            total += App.run();\n        }\n        System.out.println(total > 0 ? \"ok\" : \"fail\");\n    }\n}\n",
        )
        .unwrap();
        fixture.capture("java");
        let mut locator = TraceLocator::new();
        fixture.add_entity(
            &mut locator,
            "src/main/java/pkg/App.java",
            "public static int run",
            "App.run",
            "java",
            Some("jfr:pkg/App#run"),
        );
        fixture.add_entity(
            &mut locator,
            "src/main/java/pkg/Util.java",
            "public static int helper(int",
            "Util.helper",
            "java",
            Some("jfr:pkg/Util#helper"),
        );
        fixture.import_entities("java");
        let jdk = format!("{TOOLS}/jdk25/jdk-25.0.5+8/bin");
        let mut sources = vec![
            format!("{jdk}/javac.exe"),
            "-d".into(),
            "out".into(),
        ];
        for entry in fs::read_dir(fixture.root.join("src/main/java/pkg")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|value| value.to_str()) == Some("java") {
                sources.push(path.to_string_lossy().into_owned());
            }
        }
        let env = vec![("JAVA_HOME".to_string(), format!("{TOOLS}/jdk25/jdk-25.0.5+8"))];
        run(&fixture.root, sources, &env, "javac trace build");
        let recording = fixture.root.join("trace.jfr");
        run(
            &fixture.root,
            vec![
                format!("{jdk}/java.exe"),
                format!(
                    "-XX:StartFlightRecording=filename={},settings=profile,dumponexit=true",
                    recording.to_string_lossy()
                ),
                "-Xint".into(),
                "-cp".into(),
                "out".into(),
                "pkg.TraceMain".into(),
            ],
            &env,
            "java jfr run",
        );
        let jfr = format!("{jdk}/jfr.exe");
        let (stdout, stderr) = run_bytes(
            &fixture.root,
            vec![
                jfr.clone(),
                "print".into(),
                "--json".into(),
                "--events".into(),
                "jdk.ExecutionSample".into(),
                recording.to_string_lossy().into_owned(),
            ],
            &env,
            "jfr print",
        );
        assert!(!stderr.contains("Exception"));
        let value: Value = serde_json::from_slice(&stdout).unwrap();
        let meta = TraceMeta::new(
            &fixture.snapshot_id,
            &fixture.config_digest,
            json!({"kind": "java", "runtime": "temurin-25.0.5", "instrumentation": "jfr"}),
            "jfr",
            "25.0.5",
        )
        .scenario("scenario.java.loop")
        .clock("wall", true)
        .lossy(&[
            "JFR execution sampling at the profile interval; durations are sample intervals, not exact call durations",
            "JIT compilation may omit inlined frames",
        ]);
        let trace = jfr_execution_samples(&value, &locator, &meta).unwrap();
        let raw = fs::read(&recording).unwrap();
        let report = fixture.record(
            &locator,
            &meta,
            trace,
            &raw,
            "application/octet-stream",
            "run.java.jfr",
            "jfr-json",
        );
        groups.insert("java".into(), report);
    }

    // ---------------- C# / EventPipe (dotnet-trace speedscope) ----------------
    {
        let source_fixture = fixture_root("csharp");
        let mut fixture = setup("csharp", &[]);
        copy_dir(&source_fixture, &fixture.root);
        fs::write(
            fixture.root.join("Program.cs"),
            "using System;\nusing System.Diagnostics;\nusing System.Runtime.CompilerServices;\n\npublic static class Runner\n{\n    [MethodImpl(MethodImplOptions.NoInlining)]\n    public static int Run(int value) => Util.Helper(value) + 1;\n}\n\npublic static class Program\n{\n    public static int Main()\n    {\n        long total = 0;\n        var watch = Stopwatch.StartNew();\n        while (watch.ElapsedMilliseconds < 1500)\n        {\n            for (int i = 0; i < 10000; i++) total += Runner.Run(i);\n        }\n        Console.WriteLine(total > 0 ? \"12\" : \"0\");\n        return 0;\n    }\n}\n",
        )
        .unwrap();
        fs::write(
            fixture.root.join("Util.cs"),
            "using System.Runtime.CompilerServices;\n\npublic static class Util\n{\n    [MethodImpl(MethodImplOptions.NoInlining)]\n    public static int Helper(int value) => value * 2;\n}\n",
        )
        .unwrap();
        fixture.capture("csharp");
        let mut locator = TraceLocator::new();
        let main_entity = fixture.add_entity(
            &mut locator,
            "Program.cs",
            "public static int Main",
            "Program.Main",
            "csharp",
            Some("LangPack!LangPack.Program.Main()"),
        );
        let runner_entity = fixture.add_entity(
            &mut locator,
            "Program.cs",
            "public static int Run",
            "Runner.Run",
            "csharp",
            Some("LangPack!LangPack.Runner.Run(int32)"),
        );
        fixture.add_entity(
            &mut locator,
            "Util.cs",
            "public static int Helper",
            "Util.Helper",
            "csharp",
            Some("LangPack!Util.Helper(int32)"),
        );
        let _ = (main_entity, runner_entity);
        fixture.import_entities("csharp");
        let dotnet = format!("{TOOLS}/dotnet8/dotnet.exe");
        let csharp_env = vec![
            ("DOTNET_ROOT".to_string(), format!("{TOOLS}/dotnet8")),
            (
                "NUGET_PACKAGES".to_string(),
                "<home>/.nuget/packages".to_string(),
            ),
            ("DOTNET_CLI_TELEMETRY_OPTOUT".to_string(), "1".to_string()),
            ("DOTNET_NOLOGO".to_string(), "1".to_string()),
            (
                "DOTNET_CLI_HOME".to_string(),
                fixture.root.to_string_lossy().into_owned(),
            ),
            ("HOMEDRIVE".to_string(), "C:".to_string()),
            ("HOMEPATH".to_string(), "<home>".to_string()),
            ("ProgramFiles".to_string(), "C:/Program Files".to_string()),
            (
                "ProgramFiles(x86)".to_string(),
                "C:/Program Files (x86)".to_string(),
            ),
            ("PROCESSOR_ARCHITECTURE".to_string(), "AMD64".to_string()),
            ("COMPUTERNAME".to_string(), "LOCALHOST".to_string()),
        ];
        run(
            &fixture.root,
            vec![dotnet.clone(), "build".into(), "-nologo".into(), "-v:q".into()],
            &csharp_env,
            "dotnet trace build",
        );
        let nettrace = fixture.root.join("trace.nettrace");
        run(
            &fixture.root,
            vec![
                format!("{TOOLS}/dotnet-tools/dotnet-trace.exe"),
                "collect".into(),
                "--profile".into(),
                "dotnet-sampled-thread-time".into(),
                "--duration".into(),
                "00:00:02".into(),
                "--output".into(),
                nettrace.to_string_lossy().into_owned(),
                "--".into(),
                dotnet.clone(),
                "bin/Debug/net8.0/LangPack.dll".into(),
            ],
            &[(
                "DOTNET_ROOT".into(),
                format!("{TOOLS}/dotnet8"),
            )],
            "dotnet-trace collect",
        );
        run(
            &fixture.root,
            vec![
                format!("{TOOLS}/dotnet-tools/dotnet-trace.exe"),
                "convert".into(),
                "--format".into(),
                "speedscope".into(),
                "--output".into(),
                "speedscope".into(),
                nettrace.to_string_lossy().into_owned(),
            ],
            &[],
            "dotnet-trace convert",
        );
        let speedscope_path = fixture.root.join("speedscope.speedscope.json");
        let raw = fs::read(&speedscope_path).unwrap();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        let meta = TraceMeta::new(
            &fixture.snapshot_id,
            &fixture.config_digest,
            json!({"kind": "csharp", "runtime": "dotnet-8.0.427", "instrumentation": "eventpipe"}),
            "dotnet-trace",
            "10.0.750501",
        )
        .scenario("scenario.csharp.loop")
        .clock("monotonic", false)
        .lossy(&[
            "EventPipe sampled thread time: stack frames are sampled, not every call",
            "speedscope frames carry exact managed symbol names but no file/line; mapping uses the declared symbol probe",
        ]);
        let trace = speedscope(&value, &locator, &meta).unwrap();
        let report = fixture.record(
            &locator,
            &meta,
            trace,
            &raw,
            "application/json",
            "run.csharp.eventpipe",
            "dotnet-trace-speedscope",
        );
        groups.insert("csharp".into(), report);
        let _ = nettrace;
    }

    // ---------------- Go / runtime CPU profile ----------------
    {
        let source_fixture = fixture_root("go");
        let mut fixture = setup("go", &[]);
        copy_dir(&source_fixture, &fixture.root);
        let main_text = fs::read_to_string(fixture.root.join("main.go")).unwrap();
        fs::write(
            fixture.root.join("main.go"),
            format!("//go:build !archmeld_trace\n\n{main_text}"),
        )
        .unwrap();
        fs::write(
            fixture.root.join("trace_main.go"),
            "//go:build archmeld_trace\n\npackage main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n\t\"runtime/pprof\"\n\n\tu \"example.com/langpack/util\"\n)\n\n//go:noinline\nfunc inner(n int) int {\n\ttotal := 0\n\tfor i := 0; i < n; i++ {\n\t\ttotal += i\n\t}\n\treturn total\n}\n\n//go:noinline\nfunc outer(n int) int {\n\treturn inner(n)\n}\n\nfunc main() {\n\tfile, err := os.Create(os.Getenv(\"ARCHMELD_PROFILE\"))\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tpprof.StartCPUProfile(file)\n\ttotal := 0\n\tfor i := 0; i < 300000; i++ {\n\t\ttotal += outer(200) + u.Helper(1)\n\t}\n\tpprof.StopCPUProfile()\n\tfile.Close()\n\tfmt.Println(total, u.Helper(1))\n}\n",
        )
        .unwrap();
        fixture.capture("go");
        let mut locator = TraceLocator::new();
        fixture.add_entity(
            &mut locator,
            "trace_main.go",
            "func inner",
            "inner",
            "go",
            None,
        );
        fixture.add_entity(
            &mut locator,
            "trace_main.go",
            "func outer",
            "outer",
            "go",
            None,
        );
        fixture.add_entity(
            &mut locator,
            "util/util.go",
            "func Helper",
            "Helper",
            "go",
            None,
        );
        fixture.import_entities("go");
        let go = format!("{TOOLS}/go/go/bin/go.exe");
        let profile = fixture.root.join("cpu.pprof");
        run(
            &fixture.root,
            vec![
                go.clone(),
                "run".into(),
                "-tags".into(),
                "archmeld_trace".into(),
                ".".into(),
            ],
            &[(
                "ARCHMELD_PROFILE".into(),
                profile.to_string_lossy().into_owned(),
            )],
            "go trace run",
        );
        let (stdout, _) = run_bytes(
            &fixture.root,
            vec![
                go.clone(),
                "tool".into(),
                "pprof".into(),
                "-raw".into(),
                "-nodecount".into(),
                "200".into(),
                profile.to_string_lossy().into_owned(),
            ],
            &[],
            "go tool pprof raw",
        );
        let text = String::from_utf8(stdout).unwrap();
        let raw = fs::read(&profile).unwrap();
        let meta = TraceMeta::new(
            &fixture.snapshot_id,
            &fixture.config_digest,
            json!({"kind": "go", "runtime": "go1.27.3", "instrumentation": "runtime-pprof"}),
            "go-pprof",
            "go1.27.3",
        )
        .scenario("scenario.go.loop")
        .clock("cpu", false)
        .lossy(&[
            "Go pprof aggregates CPU samples and has no per-sample wall-clock timestamp",
            "spans use measured CPU nanoseconds on an explicit cpu clock; they are not ordered in wall time",
        ]);
        let trace = pprof_raw(&text, &locator, &meta).unwrap();
        let report = fixture.record(
            &locator,
            &meta,
            trace,
            &raw,
            "application/octet-stream",
            "run.go.pprof",
            "go-pprof-raw",
        );
        groups.insert("go".into(), report);
    }

    // ---------------- Rust explicit probe ----------------
    {
        let source_fixture = fixture_root("rust");
        let mut fixture = setup("rust", &[]);
        copy_dir(&source_fixture, &fixture.root);
        fs::create_dir_all(fixture.root.join("examples")).unwrap();
        fs::write(
            fixture.root.join("examples/trace.rs"),
            "use std::io::Write;\nuse std::sync::OnceLock;\nuse std::time::Instant;\n\nstatic EPOCH: OnceLock<Instant> = OnceLock::new();\n\nfn now_ns() -> u128 {\n    EPOCH.get_or_init(Instant::now).elapsed().as_nanos()\n}\n\nfn env(key: &str) -> String {\n    std::env::var(key).unwrap_or_default()\n}\n\nfn emit(id: &str, parent: Option<&str>, name: &str, entity: &str, start: u128, end: u128) {\n    let Some(path) = std::env::var_os(\"ARCHMELD_TRACE_OUT\") else { return };\n    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();\n    let parent = parent.map(|value| format!(\"\\\"{value}\\\"\")).unwrap_or_else(|| \"null\".to_string());\n    writeln!(file,\n        \"{{\\\"span_id\\\":\\\"{id}\\\",\\\"parent_span_id\\\":{parent},\\\"name\\\":\\\"{name}\\\",\\\"kind\\\":\\\"function\\\",\\\"start_ns\\\":\\\"{start}\\\",\\\"end_ns\\\":\\\"{end}\\\",\\\"entity_id\\\":\\\"{entity}\\\"}}\")\n        .unwrap();\n}\n\nfn main() {\n    let start = now_ns();\n    let mut total = 0;\n    for i in 0..2000 {\n        let begin = now_ns();\n        total += langpack_rust::util::helper(2);\n        let finish = now_ns();\n        emit(&format!(\"helper.{i}\"), Some(\"main\"), \"helper\", &env(\"ARCHMELD_ENTITY_HELPER\"), begin, finish);\n        let begin = now_ns();\n        total += langpack_rust::scaled();\n        let finish = now_ns();\n        emit(&format!(\"scaled.{i}\"), Some(\"main\"), \"scaled\", &env(\"ARCHMELD_ENTITY_SCALED\"), begin, finish);\n    }\n    emit(\"main\", None, \"main\", &env(\"ARCHMELD_ENTITY_MAIN\"), start, now_ns());\n    println!(\"{total}\");\n}\n",
        )
        .unwrap();
        fixture.capture("rust");
        let mut locator = TraceLocator::new();
        let helper_entity = fixture.add_entity(
            &mut locator,
            "src/util.rs",
            "pub fn helper",
            "helper",
            "rust",
            None,
        );
        let scaled_entity = fixture.add_entity(
            &mut locator,
            "src/lib.rs",
            "pub fn scaled",
            "scaled",
            "rust",
            None,
        );
        let main_entity = fixture.add_entity(
            &mut locator,
            "examples/trace.rs",
            "fn main",
            "main",
            "rust",
            None,
        );
        fixture.import_entities("rust");
        let trace_path = fixture.root.join("rust-probe.jsonl");
        let env = vec![
            (
                "ARCHMELD_TRACE_OUT".to_string(),
                trace_path.to_string_lossy().into_owned(),
            ),
            (
                "ARCHMELD_ENTITY_MAIN".to_string(),
                main_entity,
            ),
            (
                "ARCHMELD_ENTITY_HELPER".to_string(),
                helper_entity,
            ),
            (
                "ARCHMELD_ENTITY_SCALED".to_string(),
                scaled_entity,
            ),
            (
                "RUSTUP_TOOLCHAIN".to_string(),
                "1.94.1-x86_64-pc-windows-gnu".to_string(),
            ),
        ];
        run(
            &fixture.root,
            vec![
                "cargo".into(),
                "run".into(),
                "--offline".into(),
                "--quiet".into(),
                "--example".into(),
                "trace".into(),
            ],
            &env,
            "rust probe run",
        );
        let raw = fs::read(&trace_path).unwrap();
        let text = String::from_utf8(raw.clone()).unwrap();
        let meta = TraceMeta::new(
            &fixture.snapshot_id,
            &fixture.config_digest,
            json!({"kind": "rust", "runtime": "rustc-1.94.1", "instrumentation": "explicit-probe"}),
            "archmeld-explicit-probe",
            "rust-1",
        )
        .scenario("scenario.rust.loop");
        let trace = probe_jsonl(&text, &locator, &meta).unwrap();
        let report = fixture.record(
            &locator,
            &meta,
            trace,
            &raw,
            "application/x-ndjson",
            "run.rust.probe",
            "explicit-probe-jsonl",
        );
        groups.insert("rust".into(), report);
    }

    let groups_value = Value::Object(groups);
    write_json_evidence(&groups_value);
    for (name, report) in groups_value.as_object().unwrap() {
        assert_eq!(report["status"], "verified", "{name}: {report}");
        assert!(
            report["mapped_spans"].as_u64().unwrap_or(0) >= 1,
            "{name}: {report}"
        );
    }
}

