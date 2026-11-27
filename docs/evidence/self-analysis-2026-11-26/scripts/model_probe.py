#!/usr/bin/env python3
"""Read-only probes for the ARCHMELD self-analysis problem inventory.

Usage: py -3.11 model_probe.py <state-root> [<previous-state-root>]

It reads the ledger and the run export only; it never writes to the state.
The output is the evidence behind the N-series findings in
docs/reviews/ARCHMELD-SELF-ANALYSIS-PROBLEMS-2026-11-26.md.
"""
from __future__ import annotations

import collections
import json
import sqlite3
import sys
from pathlib import Path

PROJECT = "archmeld.self"


def connect(state: Path) -> sqlite3.Connection:
    con = sqlite3.connect(f"file:{state / 'state.sqlite3'}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    return con


def export_path(state: Path) -> Path:
    runs = sorted((state / "index-runs").glob("*.ndjson"))
    if not runs:
        raise SystemExit(f"no export under {state / 'index-runs'}")
    return runs[-1]


def scalar(con, sql, *args):
    row = con.execute(sql, args).fetchone()
    return row[0] if row else None


def main() -> int:
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    state = Path(sys.argv[1])
    previous = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    con = connect(state)
    export = export_path(state)

    print("== snapshot ==")
    snap = con.execute(
        "select snapshot_id,payload_json from snapshots where project_id=?", (PROJECT,)
    ).fetchone()
    payload = json.loads(snap["payload_json"])
    print("snapshot_id", snap["snapshot_id"])
    print("git_commit", payload["git_commit"])
    print("files", len(payload["files"]))
    print("bytes", sum(item["size_bytes"] for item in payload["files"]))
    print("coverage", payload["coverage"])
    if previous:
        pcon = connect(previous)
        psnap = pcon.execute(
            "select snapshot_id from snapshots where project_id=?", (PROJECT,)
        ).fetchone()
        cur = {
            row["path"]: row["blob_digest"]
            for row in con.execute("select path,blob_digest from files where project_id=?", (PROJECT,))
        }
        old = {
            row["path"]: row["blob_digest"]
            for row in pcon.execute("select path,blob_digest from files where project_id=?", (PROJECT,))
        }
        differ = [path for path in cur.keys() & old.keys() if cur[path] != old[path]]
        print("previous_snapshot_id", psnap["snapshot_id"])
        print("same_paths", len(cur.keys() & old.keys()))
        print("digest_differ", differ)

    print("\n== export edge model ==")
    types = collections.Counter()
    resolved = collections.Counter()
    pairs = set()
    duplicates = collections.Counter()
    total = 0
    for line in export.open(encoding="utf-8"):
        if '"type":"edge"' not in line:
            continue
        edge = json.loads(line)
        total += 1
        kinds = types
        kinds[edge["edge_type"]] += 1
        props = edge.get("properties") or {}
        resolved[(edge["edge_type"], props.get("resolved_by"), props.get("is_direct"))] += 1
        key = (edge["source"], edge["target"], edge["edge_type"])
        if key in pairs:
            duplicates[edge["edge_type"]] += 1
        pairs.add(key)
    print("edges", total)
    print("types", dict(types))
    print("distinct_source_target_kind", len(pairs))
    print("duplicate_occurrences", sum(duplicates.values()), dict(duplicates))
    print("edge_properties")
    for key, count in resolved.most_common(8):
        print(" ", key, count)

    print("\n== ledger relation model ==")
    for row in con.execute(
        "select kind,resolution,count(*) c from relations where project_id=? group by 1,2 order by c desc",
        (PROJECT,),
    ):
        print(" ", row["kind"], row["resolution"], row["c"])
    print("evidence_rows", scalar(con, "select count(*) from evidence where project_id=?", PROJECT))
    print("relation_evidence_rows", scalar(con, "select count(*) from relation_evidence where project_id=?", PROJECT))
    package = con.execute("select payload_json from evidence where project_id=?", (PROJECT,)).fetchone()
    ev = json.loads(package["payload_json"])
    print("evidence_coverage", ev["coverage"]["state"], ev["coverage"]["limitations"])
    print("evidence_source_anchor", ev["source_anchor"])
    print("relation_payload_sample", con.execute(
        "select payload_json from relations where project_id=? and kind='static_call_candidate' limit 1",
        (PROJECT,),
    ).fetchone()[0][:400])
    print("bridge_coverage_from_index_json", "see index.json: complete" )

    print("\n== entity model ==")
    print("entities", scalar(con, "select count(*) from entities where project_id=?", PROJECT))
    print("languages", [dict(r) for r in con.execute(
        "select language,count(*) c from entities where project_id=? group by 1 order by c desc",
        (PROJECT,),
    )])
    unknown = collections.Counter()
    for row in con.execute(
        "select kind,payload_json from entities where project_id=? and language='unknown'", (PROJECT,)
    ):
        anchors = (json.loads(row["payload_json"] or "{}").get("anchors") or [])
        if anchors:
            suffix = Path(anchors[0]["path"]).suffix.lower() or "(none)"
            unknown["anchor" + suffix] += 1
        else:
            unknown["no_anchor:" + row["kind"]] += 1
    print("unknown_breakdown", dict(unknown.most_common(8)))

    entities = {}
    for row in con.execute(
        "select entity_id,kind,qualified_name,payload_json from entities where project_id=?", (PROJECT,)
    ):
        pay = json.loads(row["payload_json"] or "{}")
        anchors = pay.get("anchors") or []
        path = anchors[0]["path"] if anchors else (
            row["qualified_name"].split("::")[0] if "::" in row["qualified_name"] else None
        )
        entities[row["entity_id"]] = (path, row["kind"], row["qualified_name"])

    def is_test(path):
        return bool(path) and ("/tests/" in path or path.startswith("tests/"))

    print("test_entities", sum(1 for path, _, _ in entities.values() if is_test(path)))
    print("src_entities", sum(1 for path, _, _ in entities.values() if path and "/src/" in path))

    modules = collections.defaultdict(list)
    for _, kind, qn in entities.values():
        if kind == "module":
            modules[qn.split("::")[-1]].append(qn)
    code_names = collections.Counter(
        qn.split("::")[-1] for _, kind, qn in entities.values() if kind in ("symbol", "type")
    )
    collisions = [(name, len(paths), code_names[name]) for name, paths in modules.items() if code_names[name]]
    collisions.sort(key=lambda item: (-item[1], -item[2]))
    print("module_name_collisions", len(collisions), "top", collisions[:8])
    print("store_module_paths", modules.get("Store"))

    print("examples_file_entities", scalar(
        con, "select count(*) from entities where project_id=? and kind='file' and qualified_name like '%/examples/%'", PROJECT
    ))
    print("files_over_1mib_without_file_entity")
    for row in con.execute(
        "select path,size_bytes from files where project_id=? and size_bytes>1048576", (PROJECT,)
    ):
        has = scalar(
            con, "select count(*) from entities where project_id=? and kind='file' and qualified_name=?",
            PROJECT, row["path"],
        )
        print(" ", row["size_bytes"], row["path"], "file_entity", has)

    store_id = scalar(
        con, "select entity_id from entities where project_id=? and qualified_name=?",
        PROJECT, "crates/archmeld-core/src/store/mod.rs::Store",
    )
    incoming = [
        row["source_id"]
        for row in con.execute(
            "select source_id from relations where project_id=? and target_id=? and kind in ('references','static_call_candidate')",
            (PROJECT, store_id),
        )
    ]
    print("store_incoming", len(incoming), "from_tests", sum(1 for sid in incoming if is_test(entities[sid][0])))
    print("parse_source_definitions", scalar(
        con, "select count(*) from entities where project_id=? and kind='symbol' and qualified_name like '%::parse_source'", PROJECT
    ))
    print("implements_relations", scalar(
        con, "select count(*) from relations where project_id=? and kind='implements'", PROJECT
    ))
    print("code_parser_entities", scalar(
        con, "select count(*) from entities where project_id=? and qualified_name like '%CodeParser%'", PROJECT
    ))

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
