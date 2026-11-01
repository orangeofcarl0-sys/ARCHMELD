// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Hash vectors from `examples/hash-vectors.json` must match byte for byte.

use archmeld_core::hash::{canonical_json, digest_bytes, digest_value, parse_strict};
use serde_json::Value;

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/archmeld/examples/hash-vectors.json"
    );
    let text = std::fs::read_to_string(path).expect("hash vectors");
    serde_json::from_str(&text).expect("valid json")
}

#[test]
fn canonical_forms_and_digests_match_vectors() {
    let data = vectors();
    for vector in data["vectors"].as_array().expect("vectors") {
        let input = parse_strict(&vector["input"].to_string()).expect("strict parse");
        let canonical = canonical_json(&input).expect("canonical");
        assert_eq!(
            canonical,
            vector["canonical_utf8"].as_str().unwrap(),
            "canonical form for {:?}",
            vector["input"]
        );
        assert_eq!(
            digest_value(&input).unwrap(),
            vector["digest"].as_str().unwrap()
        );
    }
}

#[test]
fn reject_cases_are_rejected() {
    let data = vectors();
    for case in data["reject_cases"].as_array().expect("cases") {
        let text = case["input"].to_string();
        let parsed = parse_strict(&text);
        let value_result = parsed
            .as_ref()
            .map_err(|err| err.clone())
            .and_then(|value| canonical_json(&value).map_err(|err| err));
        assert!(
            value_result.is_err(),
            "input {} must be outside the hash domain",
            case["name"]
        );
    }
}

#[test]
fn blob_digests_hash_raw_bytes() {
    let data = vectors();
    for vector in data["blob_vectors"].as_array().expect("blob vectors") {
        let relative = vector["path"].as_str().unwrap();
        let path = format!(
            "{}/../../docs/archmeld/{}",
            env!("CARGO_MANIFEST_DIR"),
            relative
        );
        let bytes = std::fs::read(&path).expect("blob");
        assert_eq!(digest_bytes(&bytes), vector["digest"].as_str().unwrap());
    }
}
