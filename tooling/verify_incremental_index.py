# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Two-run I1 check: incremental parse, deletion sweep, rename, fallback.

This drives the real archmeld-index helper; it is not a unit test and does not
touch any user workspace. Output is one JSON object for an evidence record.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path


def run_helper(helper: Path, repo: Path, out: Path, state: Path, graph: Path) -> tuple[dict, set, set]:
    env = dict(os.environ)
    env["ARCHMELD_INDEX_GRAPH"] = str(graph)
    proc = subprocess.run(
        [str(helper), str(repo), str(out), str(state), "500"],
        env=env,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise SystemExit(
            f"helper failed ({proc.returncode}): {proc.stdout} {proc.stderr}"
        )
    summary = json.loads(proc.stdout.strip().splitlines()[-1])
    nodes: dict[int, tuple[str, str, str]] = {}
    edges: set[tuple[str, str, str, str]] = set()
    for line in out.read_text(encoding="utf-8").splitlines():
        value = json.loads(line)
        if value.get("type") == "node":
            props = value["properties"]
            nodes[value["id"]] = (
                str(props.get("path", "")).replace("\\", "/"),
                str(value["node_type"]),
                str(props.get("name")),
            )
        elif value.get("type") == "edge":
            edges.add(
                (
                    value["source"],
                    value["target"],
                    str(value["edge_type"]),
                    json.dumps(value.get("properties", {}), sort_keys=True),
                )
            )
    node_keys = {(path, kind, name) for path, kind, name in nodes.values()}
    edge_keys = {
        (nodes[source][0], nodes[target][0], kind, props)
        for source, target, kind, props in edges
        if source in nodes and target in nodes
    }
    return summary, node_keys, edge_keys


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--helper", required=True)
    parser.add_argument("--work", required=True)
    args = parser.parse_args()

    helper = Path(args.helper)
    work = Path(args.work)
    if work.exists():
        shutil.rmtree(work)
    repo = work / "repo"
    (repo / "src").mkdir(parents=True)
    (repo / "src" / "a.py").write_text("def alpha():\n    return beta()\n", encoding="utf-8")
    (repo / "src" / "b.py").write_text("def beta():\n    return 1\n", encoding="utf-8")
    (repo / "src" / "c.py").write_text("def gamma():\n    return 2\n", encoding="utf-8")
    state = work / "state"
    graph = work / "graph"

    first, first_nodes, first_edges = run_helper(
        helper, repo, work / "run1.ndjson", state, graph
    )
    (repo / "src" / "a.py").write_text(
        "def alpha():\n    return beta() + 1\n", encoding="utf-8"
    )
    second, second_nodes, second_edges = run_helper(
        helper, repo, work / "run2.ndjson", state, graph
    )

    (repo / "src" / "c.py").unlink()
    third, third_nodes, _ = run_helper(
        helper, repo, work / "run3.ndjson", state, graph
    )

    (repo / "src" / "b.py").rename(repo / "src" / "d.py")
    fourth, fourth_nodes, _ = run_helper(
        helper, repo, work / "run4.ndjson", state, graph
    )

    bad_graph = work / "not-a-graph"
    bad_graph.write_text("not a database", encoding="utf-8")
    fallback, fallback_nodes, _ = run_helper(
        helper, repo, work / "run5.ndjson", work / "state2", bad_graph
    )

    result = {
        "schema_version": "1.0",
        "helper": str(helper),
        "runs": {
            "first_full": {
                "index_mode": first.get("index_mode"),
                "storage": first.get("storage"),
                "files_parsed": first.get("files_parsed"),
                "files_reused": first.get("files_reused"),
                "files_deleted": first.get("files_deleted"),
            },
            "second_incremental": {
                "index_mode": second.get("index_mode"),
                "storage": second.get("storage"),
                "files_parsed": second.get("files_parsed"),
                "files_reused": second.get("files_reused"),
                "files_deleted": second.get("files_deleted"),
            },
            "third_after_delete": {
                "index_mode": third.get("index_mode"),
                "files_parsed": third.get("files_parsed"),
                "files_reused": third.get("files_reused"),
                "files_deleted": third.get("files_deleted"),
            },
            "fourth_after_rename": {
                "index_mode": fourth.get("index_mode"),
                "files_parsed": fourth.get("files_parsed"),
                "files_reused": fourth.get("files_reused"),
                "files_deleted": fourth.get("files_deleted"),
            },
            "fifth_fallback": {
                "index_mode": fallback.get("index_mode"),
                "storage": fallback.get("storage"),
                "files_parsed": fallback.get("files_parsed"),
                "files_reused": fallback.get("files_reused"),
                "storage_note": fallback.get("storage_note"),
            },
        },
        "checks": {
            "first_was_full": first.get("index_mode") == "full_parse"
            and first.get("files_parsed") == 3,
            "second_parsed_only_changed": second.get("index_mode")
            == "incremental_parse"
            and second.get("files_parsed") == 1
            and second.get("files_reused") == 2,
            "node_set_equal_to_full_parse": first_nodes == second_nodes,
            "edge_set_equal_to_full_parse": first_edges == second_edges,
            "deleted_file_nodes_absent": not any(
                path.endswith("/src/c.py") for path, _, _ in third_nodes
            ),
            "deletion_counted": third.get("files_deleted") == 1,
            "renamed_old_path_absent": not any(
                path.endswith("/src/b.py") for path, _, _ in fourth_nodes
            ),
            "renamed_new_path_present": any(
                path.endswith("/src/d.py") for path, _, _ in fourth_nodes
            ),
            "fallback_is_full_parse": fallback.get("index_mode") == "full_parse"
            and fallback.get("storage") == "in_memory_fallback"
            and fallback.get("files_skipped") == 0,
            "fallback_has_no_half_graph": len(fallback_nodes)
            == len(fourth_nodes),
        },
        "counts": {
            "first_nodes": len(first_nodes),
            "second_nodes": len(second_nodes),
            "third_nodes": len(third_nodes),
            "fourth_nodes": len(fourth_nodes),
            "fallback_nodes": len(fallback_nodes),
            "first_edges": len(first_edges),
            "second_edges": len(second_edges),
        },
    }
    result["ok"] = all(result["checks"].values())
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
