// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! U3: the LSP initialize probe records serverInfo and advertised capabilities.
//! Downgrade is evidence-only; the sealed runtime profile is never changed.

use archmeld_core::lsp::probe_initialize_result;
use serde_json::json;

#[test]
fn complete_initialize_probe_matches_the_expected_version() {
    let result = json!({
        "serverInfo": {"name": "rust-analyzer", "version": "1.94.1"},
        "capabilities": {
            "definitionProvider": true,
            "referencesProvider": true,
            "renameProvider": {"prepareProvider": false}
        }
    });
    let probe = probe_initialize_result(&result, Some("1.94.1"));
    assert_eq!(probe.status, "ok");
    assert_eq!(probe.version_match, Some(true));
    assert!(probe.definition);
    assert!(probe.references);
    assert!(probe.rename);
    assert!(probe.degraded.is_empty(), "{:?}", probe.degraded);
    let info = probe.server_info.expect("serverInfo");
    assert_eq!(info.name.as_deref(), Some("rust-analyzer"));
    assert_eq!(info.version.as_deref(), Some("1.94.1"));
}

#[test]
fn version_mismatch_degrades_the_server_version() {
    let result = json!({
        "serverInfo": {"name": "clangd", "version": "17.0.0"},
        "capabilities": {
            "definitionProvider": true,
            "referencesProvider": true,
            "renameProvider": true
        }
    });
    let probe = probe_initialize_result(&result, Some("18.1.8"));
    assert_eq!(probe.status, "version_mismatch");
    assert_eq!(probe.version_match, Some(false));
    assert!(probe.degraded.contains(&"server_version"));
    assert!(probe.definition && probe.references && probe.rename);
}

#[test]
fn missing_rename_capability_degrades_bounded_refactor() {
    let result = json!({
        "serverInfo": {"name": "gopls", "version": "0.20.0"},
        "capabilities": {
            "definitionProvider": true,
            "referencesProvider": true
        }
    });
    let probe = probe_initialize_result(&result, Some("0.20.0"));
    assert_eq!(probe.status, "capability_missing");
    assert!(!probe.rename);
    assert_eq!(probe.degraded, vec!["bounded_refactor"]);
}

#[test]
fn missing_capabilities_are_unparsed_and_downgrade_all() {
    let result = json!({"serverInfo": {"name": "mystery-server"}});
    let probe = probe_initialize_result(&result, Some("1.0.0"));
    assert_eq!(probe.status, "unparsed");
    assert_eq!(probe.version_match, None);
    assert!(!probe.definition && !probe.references && !probe.rename);
    assert!(probe.degraded.contains(&"definition_reference"));
    assert!(probe.degraded.contains(&"bounded_refactor"));
    assert!(probe.degraded.contains(&"server_version"));
}
