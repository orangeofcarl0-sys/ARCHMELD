// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Entity identity (S02 §5).
//!
//! Identity is content-derived, not backend-derived: a syntax entity is
//! identified by its file blob, node kind, byte range and a structural
//! discriminator; a semantic entity by provider + symbol + build config +
//! scope. Backend integer IDs never become public identity.

use crate::error::{CoreError, Result};
use crate::hash;
use serde_json::json;

const ID_VERSION: &str = "1.0";

pub fn syntax_entity_id(
    file_blob_digest: &str,
    node_kind: &str,
    byte_start: u64,
    byte_end: u64,
    discriminator: &str,
) -> Result<String> {
    if file_blob_digest.is_empty() || node_kind.is_empty() {
        return Err(CoreError::invalid(
            "syntax identity requires a file blob digest and a node kind",
        ));
    }
    if byte_start > byte_end {
        return Err(CoreError::invalid(format!(
            "syntax identity range is inverted: {byte_start} > {byte_end}"
        )));
    }
    id_with_prefix(
        "syn",
        &json!({
            "version": ID_VERSION,
            "class": "syntax",
            "file_blob": file_blob_digest,
            "kind": node_kind,
            "byte_start": byte_start,
            "byte_end": byte_end,
            "discriminator": discriminator,
        }),
    )
}

/// Identity for an entity with no source anchor (external module, imported
/// crate, unresolved boundary). Scoped to the snapshot and adapter: the same
/// name in another snapshot is a different entity.
pub fn external_entity_id(
    snapshot_id: &str,
    adapter_id: &str,
    qualified_name: &str,
    kind: &str,
) -> Result<String> {
    if snapshot_id.is_empty() || adapter_id.is_empty() || qualified_name.is_empty() {
        return Err(CoreError::invalid(
            "external identity requires snapshot, adapter and qualified name",
        ));
    }
    id_with_prefix(
        "ext",
        &json!({
            "version": ID_VERSION,
            "class": "external",
            "snapshot": snapshot_id,
            "adapter": adapter_id,
            "qualified_name": qualified_name,
            "kind": kind,
        }),
    )
}

/// Syntax identity for line-only anchors: upstream parsers expose line ranges
/// but not byte ranges. The unit is explicit so a line ID can never be mistaken
/// for a byte-range ID.
pub fn line_entity_id(
    file_blob_digest: &str,
    node_kind: &str,
    line_start: u64,
    line_end: u64,
    discriminator: &str,
) -> Result<String> {
    if file_blob_digest.is_empty() || node_kind.is_empty() {
        return Err(CoreError::invalid(
            "line identity requires a file blob digest and a node kind",
        ));
    }
    if line_start > line_end {
        return Err(CoreError::invalid(format!(
            "line identity range is inverted: {line_start} > {line_end}"
        )));
    }
    id_with_prefix(
        "syn",
        &json!({
            "version": ID_VERSION,
            "class": "syntax",
            "unit": "line",
            "file_blob": file_blob_digest,
            "kind": node_kind,
            "line_start": line_start,
            "line_end": line_end,
            "discriminator": discriminator,
        }),
    )
}

pub fn semantic_entity_id(
    provider: &str,
    symbol: &str,
    config_digest: &str,
    scope: &str,
) -> Result<String> {
    if provider.is_empty() || symbol.is_empty() || config_digest.is_empty() {
        return Err(CoreError::invalid(
            "semantic identity requires provider, symbol and config digest",
        ));
    }
    id_with_prefix(
        "sem",
        &json!({
            "version": ID_VERSION,
            "class": "semantic",
            "provider": provider,
            "symbol": symbol,
            "config_digest": config_digest,
            "scope": scope,
        }),
    )
}

fn id_with_prefix(prefix: &str, value: &serde_json::Value) -> Result<String> {
    let digest = hash::digest_value(value)
        .map_err(|err| CoreError::invalid(format!("identity is outside the hash domain: {err}")))?;
    let hex = digest
        .strip_prefix("sha256:")
        .expect("digest_value always returns the sha256 prefix");
    Ok(format!("{prefix}.{hex}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOB: &str = "sha256:7594a8b29d2ab0c109c276a6d390fb2509c6cf761e97bc3124bc0f54f86dc413";

    #[test]
    fn syntax_identity_is_stable_and_discriminates() {
        let first = syntax_entity_id(BLOB, "Function", 4, 11, "def compute").unwrap();
        let again = syntax_entity_id(BLOB, "Function", 4, 11, "def compute").unwrap();
        assert_eq!(first, again);
        assert!(first.starts_with("syn."));
        assert_eq!(first.len(), 68);
        let other = syntax_entity_id(BLOB, "Function", 4, 11, "def other").unwrap();
        assert_ne!(first, other);
    }

    #[test]
    fn semantic_identity_tracks_scope_and_config() {
        let base = semantic_entity_id("scip-python", "example.compute()", "sha256:aa", "module").unwrap();
        let other_config =
            semantic_entity_id("scip-python", "example.compute()", "sha256:bb", "module").unwrap();
        assert_ne!(base, other_config);
        assert!(base.starts_with("sem."));
    }
}
