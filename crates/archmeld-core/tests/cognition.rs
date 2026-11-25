// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

use archmeld_core::brief::render_markdown;
use archmeld_core::cognition::{affected, summarize};
use serde_json::json;
use std::fs;
use std::path::Path;

fn write_root(root: &Path, code_path: &str) {
    fs::write(
        root.join("aoci.txt"),
        format!(
            "#AOCI-ROOT-MANIFEST: 1\n#Format-Version: cognition-volumes/v1\n\
             #Volume: id=meta kind=meta path=aoci.meta.txt format=meta-v1 depends=- state=enabled\n\
             #Volume: id=code kind=code path={code_path} format=object-fras-v2 depends=meta state=enabled\n"
        ),
    )
    .unwrap();
}

#[test]
fn neutral_root_maps_entries_and_affected_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_root(root, "aoci.code.txt");
    fs::write(
        root.join("aoci.code.txt"),
        "#AOCI-CODE-VOLUME: 1\n\
         ===/<repository>/===\n\
         README.md[CG3T]: F:Project readme | R:- | A:- | S:-\n\
         .gitignore[CG3T]: F:Ignore rules | R:- | A:- | S:-\n\
         ===/<repository>/src/===\n\
         a.py[CG5T]: F:Entry A | R:code:src/b.py | A:run | S:keep the order\n\
         b.py[CG7M]: F:Entry B | R:- | A:- | S:-\n",
    )
    .unwrap();

    let summary = summarize(Some(root));
    assert_eq!(summary["present"], true, "{summary}");
    assert_eq!(summary["entries_total"], 4, "{summary}");
    let code = summary["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|volume| volume["kind"] == "code")
        .expect("code volume");
    assert_eq!(code["path"], "aoci.code.txt");

    let dotfile = affected(Some(root), &[".gitignore".into()]);
    assert_eq!(dotfile["affected"][0]["path"], ".gitignore", "{dotfile}");

    let changed = affected(Some(root), &["src/a.py".into(), "src/new.py".into()]);
    assert_eq!(changed["present"], true, "{changed}");
    let affected_entries = changed["affected"].as_array().unwrap();
    assert_eq!(affected_entries.len(), 1, "{changed}");
    assert_eq!(affected_entries[0]["path"], "src/a.py");
    assert_eq!(affected_entries[0]["tag"], "CG5T");
    assert!(affected_entries[0]["function"]
        .as_str()
        .unwrap()
        .contains("Entry A"));
    assert_eq!(
        changed["unmapped_changed_files"],
        serde_json::json!(["src/new.py"])
    );
}

#[test]
fn missing_map_is_reported_as_absent_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let summary = summarize(Some(dir.path()));
    assert_eq!(summary["present"], false, "{summary}");

    let changed = affected(Some(dir.path()), &["src/a.py".into()]);
    assert_eq!(changed["present"], false, "{changed}");
    assert_eq!(
        changed["unmapped_changed_files"],
        serde_json::json!(["src/a.py"])
    );

    let no_workspace = summarize(None);
    assert_eq!(no_workspace["present"], false, "{no_workspace}");
}

#[test]
fn crlf_and_malformed_lines_are_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_root(root, "aoci.code.txt");
    fs::write(
        root.join("aoci.code.txt"),
        "#AOCI-CODE-VOLUME: 1\r\n\
         ===/<repository>/src/===\r\n\
         a.py[CG5T]: F:Entry A | R:- | A:- | S:-\r\n\
         this line is not an entry\r\n\
         b.py[CG7M]: F:Entry B | R:- | A:- | S:-\r\n",
    )
    .unwrap();

    let summary = summarize(Some(root));
    assert_eq!(summary["present"], true, "{summary}");
    assert_eq!(summary["entries_total"], 2, "{summary}");
    let warnings = summary["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|warning| warning
        .as_str()
        .unwrap_or_default()
        .contains("Entry grammar")));
}

#[test]
fn an_absolute_section_root_still_matches_by_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_root(root, "aoci.code.txt");
    fs::write(
        root.join("aoci.code.txt"),
        "#AOCI-CODE-VOLUME: 1\n\
         ===/work/another-clone/src/===\n\
         x.py[CG5T]: F:Foreign root | R:- | A:- | S:-\n",
    )
    .unwrap();

    let changed = affected(Some(root), &["src/x.py".into()]);
    assert_eq!(changed["present"], true, "{changed}");
    assert_eq!(changed["affected"].as_array().unwrap().len(), 1, "{changed}");
    assert_eq!(changed["affected"][0]["path"], "/work/another-clone/src/x.py");
    assert_eq!(changed["affected"][0]["relocated"], false);
    assert!(changed["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("absolute or foreign root")));
}

#[test]
fn markdown_card_mentions_the_cognition_map() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_root(root, "aoci.code.txt");
    fs::write(
        root.join("aoci.code.txt"),
        "#AOCI-CODE-VOLUME: 1\n\
         ===/<repository>/src/===\n\
         a.py[CG5T]: F:Entry A | R:- | A:- | S:-\n",
    )
    .unwrap();
    let cognition = affected(Some(root), &["src/a.py".into()]);
    let brief = json!({
        "schema_version": "1.0",
        "state": "ok",
        "level": "change",
        "summary": "one file changed",
        "scope": {"project_id": "p", "snapshot_id": "s", "coverage": "complete"},
        "counts": {"confirmed": 0, "candidate": 0, "unknown": 0, "stale": 0, "findings": 0},
        "findings": [],
        "unknowns": [],
        "next_actions": [],
        "cognition": cognition,
    });
    let markdown = render_markdown(&brief);
    assert!(markdown.contains("Cognition map (AOCI, optional)"), "{markdown}");
    assert!(markdown.contains("`src/a.py` [CG5T]"), "{markdown}");
}
