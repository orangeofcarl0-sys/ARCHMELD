// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT021/AT022: SCIP definitions stay definitions, references never become
//! calls, external symbols get no fake anchors, and duplicate names stay
//! scoped by their SCIP moniker.

use archmeld_core::scip::{import_scip, ScipConfig};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::json;
use std::fs;
use std::path::Path;

const PROJECT: &str = "project.scip";

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

fn key(field: u32, wire: u8) -> Vec<u8> {
    varint(((field as u64) << 3) | wire as u64)
}

fn length_delimited(field: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = key(field, 2);
    out.extend(varint(bytes.len() as u64));
    out.extend_from_slice(bytes);
    out
}

fn integer(field: u32, value: u64) -> Vec<u8> {
    let mut out = key(field, 0);
    out.extend(varint(value));
    out
}

fn packed(field: u32, values: &[u32]) -> Vec<u8> {
    let mut body = Vec::new();
    for value in values {
        body.extend(varint(*value as u64));
    }
    length_delimited(field, &body)
}

fn concat(parts: Vec<Vec<u8>>) -> Vec<u8> {
    parts.into_iter().flatten().collect()
}

fn occurrence(range: &[u32], symbol: &str, roles: u32) -> Vec<u8> {
    let mut parts = Vec::new();
    if !range.is_empty() {
        parts.push(packed(1, range));
    }
    parts.push(length_delimited(2, symbol.as_bytes()));
    parts.push(integer(3, roles as u64));
    concat(parts)
}

fn symbol_info(symbol: &str, kind: u64) -> Vec<u8> {
    concat(vec![
        length_delimited(1, symbol.as_bytes()),
        integer(5, kind),
    ])
}

struct Doc<'a> {
    path: &'a str,
    language: &'a str,
    encoding: u32,
    occurrences: Vec<Vec<u8>>,
    symbols: Vec<Vec<u8>>,
}

fn document(doc: &Doc<'_>) -> Vec<u8> {
    let mut parts = vec![
        length_delimited(1, doc.path.as_bytes()),
        length_delimited(4, doc.language.as_bytes()),
        integer(6, doc.encoding as u64),
    ];
    parts.extend(doc.occurrences.iter().cloned().map(|o| length_delimited(2, &o)));
    parts.extend(doc.symbols.iter().cloned().map(|s| length_delimited(3, &s)));
    concat(parts)
}

fn index(documents: &[Doc<'_>], external: &[Vec<u8>]) -> Vec<u8> {
    let tool = concat(vec![
        length_delimited(1, b"scip-test"),
        length_delimited(2, b"1.0.0"),
    ]);
    let metadata = concat(vec![
        length_delimited(2, &tool),
        integer(4, 1),
    ]);
    let mut parts = vec![length_delimited(1, &metadata)];
    parts.extend(documents.iter().map(|doc| length_delimited(2, &document(doc))));
    parts.extend(external.iter().cloned().map(|symbol| length_delimited(3, &symbol)));
    concat(parts)
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: std::path::PathBuf,
    snapshot_id: String,
    config_digest: String,
    files: Vec<archmeld_core::store::FileEntry>,
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    for (path, content) in files {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, content).unwrap();
    }
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "scip"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_digest = captured.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    Fixture {
        _dir: dir,
        store,
        root,
        snapshot_id: captured.snapshot_id,
        config_digest,
        files: captured.files,
    }
}

fn config<'a>(snapshot_id: &'a str, config_digest: &'a str) -> ScipConfig<'a> {
    ScipConfig {
        project_id: PROJECT,
        snapshot_id,
        config_digest,
        run_id: "run.scip.001",
        max_entities_per_batch: 500,
        max_relations_per_batch: 2000,
    }
}

fn config_digest(fixture: &Fixture) -> String {
    fixture.config_digest.clone()
}

#[test]
fn definitions_references_and_external_symbols_are_separated() {
    let mut fixture = fixture(&[
        (
            "app.py",
            "main = 1\nmain(Foo)\nFoo\next()\n",
        ),
        ("other.py", "main = 2\n"),
    ]);
    let main = "scip-python python pkg 1.0 app/main.";
    let other_main = "scip-python python pkg 1.0 other/main.";
    let foo = "scip-python python pkg 1.0 app/Foo#";
    let external = "scip-python python pkg 1.0 ext/helper().";
    let docs = vec![
        Doc {
            path: "app.py",
            language: "python",
            encoding: 1,
            occurrences: vec![
                occurrence(&[0, 0, 0, 4], main, 1),
                occurrence(&[2, 0, 2, 3], foo, 1),
                occurrence(&[1, 0, 1, 4], main, 0),
                occurrence(&[1, 5, 1, 8], foo, 0),
                occurrence(&[3, 0, 3, 3], external, 0),
            ],
            symbols: vec![symbol_info(main, 17), symbol_info(foo, 7)],
        },
        Doc {
            path: "other.py",
            language: "python",
            encoding: 1,
            occurrences: vec![occurrence(&[0, 0, 0, 4], other_main, 1)],
            symbols: vec![symbol_info(other_main, 17)],
        },
    ];
    let bytes = index(&docs, &[symbol_info(external, 17)]);
    let digest = config_digest(&fixture);
    let snapshot_id = fixture.snapshot_id.clone();
    let cfg = config(&snapshot_id, &digest);
    let report = import_scip(&mut fixture.store, &cfg, &fixture.files, &bytes).expect("import");

    assert_eq!(report.documents, 2);
    assert_eq!(report.external_entities, 1);
    assert_eq!(report.relations, 3);
    assert_eq!(report.coverage["state"], "complete");
    assert_eq!(report.diagnostics, 0);
    let kinds = fixture
        .store
        .relation_kinds(PROJECT, &fixture.snapshot_id)
        .unwrap();
    assert_eq!(kinds.len(), 1);
    assert!(kinds.contains("references"));
    assert!(!kinds.contains("resolved_call"), "references must not become calls");

    let definition = fixture
        .store
        .find_entity_by_qualified_name(PROJECT, &fixture.snapshot_id, main)
        .unwrap()
        .expect("definition entity");
    assert_eq!(definition["external"], false);
    assert_eq!(definition["anchors"][0]["precision"], "bytes");
    assert_eq!(definition["anchors"][0]["byte_start"], 0);
    assert_eq!(definition["anchors"][0]["byte_end"], 4);

    let external_entity = fixture
        .store
        .find_entity_by_qualified_name(PROJECT, &fixture.snapshot_id, external)
        .unwrap()
        .expect("external entity");
    assert_eq!(external_entity["external"], true);
    assert_eq!(external_entity["anchors"].as_array().unwrap().len(), 0);
    assert!(fixture.store.integrity_check().unwrap().is_empty());

    let other = fixture
        .store
        .find_entity_by_qualified_name(PROJECT, &fixture.snapshot_id, other_main)
        .unwrap()
        .expect("other main");
    assert_ne!(definition["id"], other["id"], "same name, different scope");
}

#[test]
fn utf16_ranges_convert_to_utf8_bytes() {
    let mut fixture = fixture(&[("u.py", "中文 main()\n")]);
    let symbol = "scip-python python pkg 1.0 u/main().";
    let docs = vec![Doc {
        path: "u.py",
        language: "python",
        encoding: 2,
        occurrences: vec![occurrence(&[0, 3, 0, 7], symbol, 1)],
        symbols: vec![symbol_info(symbol, 17)],
    }];
    let digest = config_digest(&fixture);
    let snapshot_id = fixture.snapshot_id.clone();
    let cfg = config(&snapshot_id, &digest);
    let report = import_scip(&mut fixture.store, &cfg, &fixture.files, &index(&docs, &[]))
        .expect("import");
    assert_eq!(report.coverage["state"], "complete");
    let entity = fixture
        .store
        .find_entity_by_qualified_name(PROJECT, &fixture.snapshot_id, symbol)
        .unwrap()
        .expect("entity");
    assert_eq!(entity["anchors"][0]["precision"], "bytes");
    assert_eq!(entity["anchors"][0]["byte_start"], 7);
    assert_eq!(entity["anchors"][0]["byte_end"], 11);
}

#[test]
fn documents_outside_the_snapshot_are_explicitly_partial() {
    let mut fixture = fixture(&[("app.py", "x = 1\n")]);
    let docs = vec![Doc {
        path: "missing.py",
        language: "python",
        encoding: 1,
        occurrences: vec![],
        symbols: vec![],
    }];
    let digest = config_digest(&fixture);
    let snapshot_id = fixture.snapshot_id.clone();
    let cfg = config(&snapshot_id, &digest);
    let report = import_scip(&mut fixture.store, &cfg, &fixture.files, &index(&docs, &[]))
        .expect("import");
    assert_eq!(report.omitted_documents, 1);
    assert_eq!(report.diagnostics, 1);
    assert_eq!(report.coverage["state"], "partial");
    assert_eq!(
        fixture
            .store
            .published_counts(PROJECT, &fixture.snapshot_id)
            .unwrap()
            .entities,
        0
    );
}

#[test]
fn real_rust_analyzer_index_imports_without_promoting_references() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream-probes/scip/rustdemo");
    let index_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream-probes/scip/rustdemo.scip");
    if !fixture_root.is_dir() || !index_path.is_file() {
        eprintln!("real SCIP fixture missing; skipping");
        return;
    }
    let index_bytes = fs::read(&index_path).expect("index");
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &fixture_root,
        &SnapshotInputs {
            build_config: json!({"variant": "scip-real"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let digest = captured.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let cfg = ScipConfig {
        project_id: PROJECT,
        snapshot_id: &captured.snapshot_id,
        config_digest: &digest,
        run_id: "run.scip.real.001",
        max_entities_per_batch: 500,
        max_relations_per_batch: 2000,
    };
    let report = import_scip(&mut store, &cfg, &captured.files, &index_bytes).expect("import");
    assert_eq!(report.documents, 1, "rustdemo has one document");
    assert!(report.entities >= 3, "helper/main_value/Boxed plus files");
    assert!(report.relations >= 1, "main_value references helper");
    let kinds = store
        .relation_kinds(PROJECT, &captured.snapshot_id)
        .unwrap();
    assert_eq!(kinds.len(), 1);
    assert!(kinds.contains("references"));
    assert!(store.integrity_check().unwrap().is_empty());
}

#[test]
fn real_python_scip_index_imports_references_without_calls() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream-probes/scip/python");
    let index_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream-probes/scip/python.scip");
    if !fixture_root.is_dir() || !index_path.is_file() {
        eprintln!("real Python SCIP fixture missing; skipping");
        return;
    }
    let index_bytes = fs::read(&index_path).expect("index");
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &fixture_root,
        &SnapshotInputs {
            build_config: json!({"variant": "scip-python-real"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let digest = captured.snapshot["config_digest"].as_str().unwrap().to_string();
    let cfg = ScipConfig {
        project_id: PROJECT,
        snapshot_id: &captured.snapshot_id,
        config_digest: &digest,
        run_id: "run.scip.python.001",
        max_entities_per_batch: 500,
        max_relations_per_batch: 2000,
    };
    let report = import_scip(&mut store, &cfg, &captured.files, &index_bytes).expect("import");
    assert_eq!(report.documents, 3, "pyproject fixture has three documents");
    assert!(report.entities >= 4, "main/helper/Box/__init__ plus file entities");
    assert!(report.relations >= 1, "app references helper");
    let kinds = store
        .relation_kinds(PROJECT, &captured.snapshot_id)
        .unwrap();
    assert_eq!(kinds.len(), 1);
    assert!(kinds.contains("references"), "no SCIP reference may become a call");
    assert!(store.integrity_check().unwrap().is_empty());
}
