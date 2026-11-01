// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B017: import a real CodeBoarding `analysis.json` produced by the authorized
//! model provider. The test runs only when the paths are supplied; it records
//! the provider/model provenance and never reads an API key.

use archmeld_core::hash;
use archmeld_core::interpret::{
    architecture_producer, architecture_review, import_codeboarding, CodeBoardingRequest,
    ModelProvenance,
};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const P: &str = "project.b017.model";

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

#[test]
fn real_model_analysis_imports_with_provider_provenance() {
    let Some(analysis_path) = env_path("ARCHMELD_B017_ANALYSIS_JSON") else {
        return;
    };
    let Some(target_repo) = env_path("ARCHMELD_B017_TARGET_REPO") else {
        return;
    };
    let provenance_path = env_path("ARCHMELD_B017_PROVENANCE")
        .expect("ARCHMELD_B017_PROVENANCE must point at the model sidecar");
    let analysis_bytes = fs::read(&analysis_path).unwrap();
    let analysis: Value = serde_json::from_slice(&analysis_bytes).unwrap();
    let provenance: Value =
        serde_json::from_slice(&fs::read(&provenance_path).unwrap()).unwrap();
    let provider = provenance["provider"].as_str().unwrap().to_string();
    let model = provenance["model"].as_str().unwrap().to_string();
    let host = provenance["base_url_host"].as_str().unwrap().to_string();
    let calls = provenance["calls"].as_u64().unwrap();
    let input_tokens = provenance["input_tokens"].as_u64();
    let output_tokens = provenance["output_tokens"].as_u64();
    let provider_version = provenance["provider_version"]
        .as_str()
        .unwrap()
        .to_string();
    let codeboarding_commit = provenance["codeboarding_commit"]
        .as_str()
        .unwrap()
        .to_string();

    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        P,
        &target_repo,
        &SnapshotInputs {
            build_config: json!({"kind": "python", "variant": "codeboarding-model"}),
            environment: json!({"os": std::env::consts::OS}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let entities_before = store.entity_ids(P, &snapshot.snapshot_id).unwrap().len();
    let relations_before = store
        .relation_payloads(P, &snapshot.snapshot_id)
        .unwrap()
        .len();
    let architecture = import_codeboarding(
        &mut store,
        &CodeBoardingRequest {
            project_id: P,
            snapshot_id: &snapshot.snapshot_id,
            expected_revision: 1,
            idempotency_key: "b017-model-1",
            provider_version: &provider_version,
            run_id: "run.b017.model",
            commands: vec![],
            model: Some(ModelProvenance {
                provider: &provider,
                model: &model,
                base_url_host: &host,
                calls,
                input_tokens,
                output_tokens,
            }),
        },
        &analysis,
    )
    .unwrap();
    assert_eq!(architecture["status"], "candidate");
    assert_eq!(architecture["layer"], "as_is");
    let components = architecture["components"].as_array().unwrap();
    assert!(!components.is_empty(), "the model produced no components");
    assert!(
        components
            .iter()
            .all(|component| !component["purpose"].as_str().unwrap_or("").is_empty()),
        "every top-level component has a model description"
    );
    let review = architecture_review(&store, P, architecture["id"].as_str().unwrap()).unwrap();
    assert_eq!(review["candidate_only"], true);
    assert_eq!(review["certain_calls_added"], 0);
    assert_eq!(review["commands_executed"], 0);
    let producer = architecture_producer(&store, P, architecture["id"].as_str().unwrap()).unwrap();
    assert_eq!(producer["model"]["provider"], provider);
    assert_eq!(producer["model"]["model"], model);
    assert_eq!(producer["model"]["base_url_host"], host);
    assert_eq!(producer["model"]["calls"], calls);
    assert!(!producer.to_string().to_lowercase().contains("api_key"));
    assert_eq!(
        store.entity_ids(P, &snapshot.snapshot_id).unwrap().len(),
        entities_before
    );
    assert_eq!(
        store.relation_payloads(P, &snapshot.snapshot_id).unwrap().len(),
        relations_before
    );

    let analysis_digest = hash::digest_bytes(&analysis_bytes);
    let evidence = json!({
        "schema_version": "1.0",
        "generated_by": "crates/archmeld-core/tests/b017_model_analysis.rs",
        "platform": std::env::consts::OS,
        "analysis": {
            "path": analysis_path,
            "sha256": analysis_digest,
            "size_bytes": analysis_bytes.len(),
            "components": analysis["components"].as_array().map(Vec::len).unwrap_or(0),
            "relations": analysis["components_relations"].as_array().map(Vec::len).unwrap_or(0),
        },
        "provider": {
            "provider": provider,
            "model": model,
            "base_url_host": host,
            "calls": calls,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "provider_version": provider_version,
            "codeboarding_commit": codeboarding_commit,
            "key_source": "ZCode provider configuration; the key is never read by this test",
        },
        "target_repo": target_repo,
        "import": {
            "architecture_id": architecture["id"],
            "status": architecture["status"],
            "components": components.len(),
            "dependencies": architecture["dependencies"].as_array().map(Vec::len).unwrap_or(0),
            "review": {
                "candidate_only": review["candidate_only"],
                "certain_calls_added": review["certain_calls_added"],
                "source_references": review["source_references"],
                "commands_executed": review["commands_executed"],
            },
            "entities_added": store.entity_ids(P, &snapshot.snapshot_id).unwrap().len() as i64 - entities_before as i64,
            "relations_added": store.relation_payloads(P, &snapshot.snapshot_id).unwrap().len() as i64 - relations_before as i64,
            "producer": producer,
        },
    });
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/release-evidence/B017-model-interpretation.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&evidence).unwrap() + "\n").unwrap();
    }
}
