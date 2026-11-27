#!/usr/bin/env python3
"""Run the ARCHMELD self-analysis acceptance checks against a real ledger.

The checks are read-only: they start the MCP stdio server on an existing state
directory, call the read tools, and inspect the SQLite ledger for counts that
the protocol summaries intentionally page. Output is one JSON report with every
measurement and the pass/fail reason, so a run can be attached to evidence.

Usage:
  py -3.11 tooling/self_analysis_acceptance.py \
    --state <ledger-dir> --project <project-id> \
    --mcp <archmeld-mcp-binary> --out <report.json>
"""
from __future__ import annotations

import argparse
import json
import sqlite3
import subprocess
import sys
from pathlib import Path


class Mcp:
    def __init__(self, binary: Path, state: Path, project: str):
        self.proc = subprocess.Popen(
            [str(binary), "--state", str(state), "--project", project],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
        self.next_id = 1
        init = self.send(
            {
                "jsonrpc": "2.0",
                "id": self.next_id,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "self-analysis-acceptance"},
                },
            }
        )
        if "result" not in init:
            raise RuntimeError(f"initialize failed: {init}")
        self.notify({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, request: dict) -> dict:
        self.next_id += 1
        self.proc.stdin.write(json.dumps(request) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            stderr = self.proc.stderr.read() if self.proc.stderr else ""
            raise RuntimeError(f"MCP closed stdout: {stderr}")
        return json.loads(line)

    def notify(self, request: dict) -> None:
        self.proc.stdin.write(json.dumps(request) + "\n")
        self.proc.stdin.flush()

    def call(self, tool: str, arguments: dict) -> tuple[int, dict]:
        response = self.send(
            {
                "jsonrpc": "2.0",
                "id": self.next_id,
                "method": "tools/call",
                "params": {"name": tool, "arguments": arguments},
            }
        )
        if response.get("result", {}).get("isError"):
            raise RuntimeError(f"{tool} failed: {response}")
        raw = json.dumps(response, ensure_ascii=False).encode("utf-8")
        text = response["result"]["content"][0]["text"]
        return len(raw), json.loads(text)

    def close(self) -> None:
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def ledger_metrics(state: Path, project: str) -> dict:
    database = state / "state.sqlite3"
    connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
    try:
        snapshot = connection.execute(
            "SELECT snapshot_id FROM snapshots WHERE project_id=? ORDER BY created_at DESC LIMIT 1",
            (project,),
        ).fetchone()
        if not snapshot:
            return {"error": "no snapshot"}
        snapshot_id = snapshot[0]
        run = connection.execute(
            "SELECT cli_commit, helper_commit, export_schema, parse_json, coverage_json "
            "FROM index_runs WHERE project_id=? AND snapshot_id=? "
            "ORDER BY created_at DESC, rowid DESC LIMIT 1",
            (project, snapshot_id),
        ).fetchone()
        implements = connection.execute(
            "SELECT COUNT(*) FROM relations WHERE project_id=? AND snapshot_id=? AND kind='implements'",
            (project, snapshot_id),
        ).fetchone()[0]
        examples = connection.execute(
            "SELECT COUNT(*) FROM entities WHERE project_id=? AND snapshot_id=? "
            "AND payload_json LIKE '%/examples/%'",
            (project, snapshot_id),
        ).fetchone()[0]
        return {
            "snapshot_id": snapshot_id,
            "implements_relations": implements,
            "examples_entities": examples,
            "cli_commit": run[0] if run else None,
            "helper_commit": run[1] if run else None,
            "export_schema": run[2] if run else None,
            "parse": json.loads(run[3]) if run else None,
            "coverage": json.loads(run[4]) if run else None,
        }
    finally:
        connection.close()


def filter_seed(state: Path, project: str) -> dict:
    """Pick a real call-candidate seed with a site and cross-file resolution."""
    database = state / "state.sqlite3"
    connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
    try:
        row = connection.execute(
            "SELECT r.source_id, r.producer_resolution, r.call_site_line "
            "FROM relations r JOIN entities e "
            "ON e.project_id=r.project_id AND e.snapshot_id=r.snapshot_id "
            "AND e.entity_id=r.source_id "
            "WHERE r.project_id=? AND r.kind='static_call_candidate' "
            "AND r.call_site_line IS NOT NULL "
            "AND r.producer_resolution='direct' LIMIT 1",
            (project,),
        ).fetchone()
        if not row:
            return {}
        return {
            "entity_id": row[0],
            "producer_resolution": row[1],
            "call_site_line": row[2],
        }
    finally:
        connection.close()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--state", required=True)
    parser.add_argument("--project", required=True)
    parser.add_argument("--mcp", required=True)
    parser.add_argument("--helper", default=None)
    parser.add_argument("--out", required=True)
    parser.add_argument("--max-status-bytes", type=int, default=256 * 1024)
    parser.add_argument("--max-review-bytes", type=int, default=1024 * 1024)
    args = parser.parse_args()

    state = Path(args.state)
    metrics = ledger_metrics(state, args.project)
    checks: list[dict] = []

    def check(name: str, ok: bool, detail) -> None:
        checks.append({"name": name, "ok": bool(ok), "detail": detail})

    if "error" in metrics:
        check("ledger", False, metrics["error"])
        report = {"checks": checks}
        Path(args.out).write_text(json.dumps(report, indent=2), encoding="utf-8")
        return 1

    parse = metrics.get("parse") or {}
    check(
        "export_schema_1_2",
        metrics.get("export_schema") == "1.2",
        metrics.get("export_schema"),
    )
    errors = parse.get("parser_errors") or []
    kotlin_paths = [
        item.get("path", "")
        for item in errors
        if "jetbrains/" in item.get("path", "")
    ]
    check("parser_errors_have_paths", len(errors) >= 3 and len(kotlin_paths) >= 3, errors)
    check(
        "implements_at_least_40",
        metrics["implements_relations"] >= 40,
        metrics["implements_relations"],
    )
    check(
        "examples_entities_positive",
        metrics["examples_entities"] > 0,
        metrics["examples_entities"],
    )
    if args.helper:
        helper = Path(args.helper)
        schema = subprocess.run(
            [str(helper), "--schema"], capture_output=True, text=True, timeout=60
        )
        version = subprocess.run(
            [str(helper), "--version"], capture_output=True, text=True, timeout=60
        )
        check(
            "helper_schema_1_2",
            schema.returncode == 0 and schema.stdout.strip() == "1.2",
            {"exit": schema.returncode, "stdout": schema.stdout.strip()},
        )
        check(
            "helper_version_launches",
            version.returncode == 0 and "schema 1.2" in version.stdout,
            {"exit": version.returncode, "stdout": version.stdout.strip()},
        )

    client = Mcp(Path(args.mcp), state, args.project)
    try:
        status_bytes, status = client.call(
            "archmeld_status", {"detail": "summary", "max_files": 100}
        )
        project = next(
            (item for item in status.get("projects", []) if item.get("project_id") == args.project),
            (status.get("projects") or [{}])[0],
        )
        freshness = project.get("freshness") or {}
        check(
            "status_summary_under_256kb",
            status_bytes < args.max_status_bytes,
            status_bytes,
        )
        check(
            "status_clean",
            freshness.get("state") == "clean" and freshness.get("needs_review") is False,
            freshness,
        )
        languages = project.get("languages") or {}
        if isinstance(languages, list):
            languages = {
                item.get("language"): item.get("entities", 0) for item in languages
            }
        for language in ("kotlin", "toml", "bash"):
            check(f"language_{language}", languages.get(language, 0) > 0, languages)
        snapshot = project.get("snapshot") or {}
        origin = snapshot.get("files_origin") or {}
        check(
            "status_files_origin_visible",
            origin.get("tracked", 0) > 0
            and origin.get("tracked", 0)
            + origin.get("untracked", 0)
            + sum(
                value for key, value in origin.items()
                if key not in ("tracked", "untracked")
            )
            == snapshot.get("file_count"),
            {"files_origin": origin, "file_count": snapshot.get("file_count")},
        )

        review_bytes, review = client.call(
            "archmeld_review", {"detail": "summary", "max_files": 500}
        )
        check(
            "review_summary_under_1mb",
            review_bytes < args.max_review_bytes,
            review_bytes,
        )
        check(
            "review_agrees_with_status",
            review.get("state") == freshness.get("state")
            and (review.get("summary") or {}).get("needs_review")
            == freshness.get("needs_review"),
            {
                "state": review.get("state"),
                "needs_review": (review.get("summary") or {}).get("needs_review"),
            },
        )
        review_paths = [item.get("path") for item in review.get("files", [])]
        page = review.get("page") or {}
        cursor = page.get("next_cursor")
        while cursor:
            page_bytes, page_review = client.call(
                "archmeld_review",
                {"detail": "summary", "max_files": 500, "cursor": cursor},
            )
            check("review_page_under_1mb", page_bytes < args.max_review_bytes, page_bytes)
            review_paths.extend(item.get("path") for item in page_review.get("files", []))
            cursor = (page_review.get("page") or {}).get("next_cursor")

        query = client.call(
            "archmeld_query", {"kind": "symbols", "query": "::Store", "entity_kinds": ["type"], "max_nodes": 20}
        )[1]
        entities = query.get("entities") or []
        store_entity = next(
            (
                item
                for item in entities
                if item.get("qualified_name", "").endswith("store/mod.rs::Store")
            ),
            entities[0] if entities else None,
        )
        check("store_symbol_found", store_entity is not None, entities[:3])
        if store_entity is not None:
            context = client.call(
                "archmeld_context",
                {
                    "objective": "self-analysis acceptance for the Store type",
                    "entity_ids": [store_entity.get("entity_id") or store_entity["id"]],
                    "budget_tokens": 4000,
                },
            )[1]
            check(
                "context_4000_non_empty",
                bool(context.get("selected_entities")) and bool(context.get("source_snippets")),
                {
                    "selected_entities": len(context.get("selected_entities") or []),
                    "source_snippets": len(context.get("source_snippets") or []),
                    "budget": context.get("budget"),
                },
            )

        # P04: producer-resolution and call-site filters must narrow a real
        # traversal, and every returned relation must carry the promised fields.
        seed = filter_seed(state, args.project)
        if not seed:
            check("p04_filter_seed", False, "no cross-file call candidate with a site")
        else:
            filtered = client.call(
                "archmeld_query",
                {
                    "kind": "neighbors",
                    "entity_ids": [seed["entity_id"]],
                    "relation_kinds": ["static_call_candidate"],
                    "min_producer_resolution": seed["producer_resolution"],
                    "require_call_site": True,
                    "max_nodes": 2000,
                    "max_depth": 1,
                },
            )[1]
            relations = filtered.get("relations") or []
            check(
                "p04_producer_and_site_filter",
                bool(relations)
                and all(
                    relation.get("producer_resolution")
                    in ("cross_file_resolution", "direct")
                    and (relation.get("call_site") or {}).get("line", 0) >= 1
                    and relation.get("resolution") != "resolved"
                    for relation in relations
                ),
                {
                    "seed": seed,
                    "returned": len(relations),
                    "resolutions": sorted(
                        {relation.get("producer_resolution") for relation in relations}
                    ),
                },
            )
            rejected = client.send(
                {
                    "jsonrpc": "2.0",
                    "id": client.next_id,
                    "method": "tools/call",
                    "params": {
                        "name": "archmeld_query",
                        "arguments": {
                            "kind": "symbols",
                            "query": "Store",
                            "require_call_site": True,
                        },
                    },
                }
            )
            check(
                "p04_rejects_relation_filter_on_symbols",
                rejected.get("result", {}).get("isError") is True,
                rejected,
            )

        # P11/P16: exclusion removes test and vendored/config references.
        if store_entity is not None:
            store_id = store_entity.get("entity_id") or store_entity["id"]
            base = {
                "kind": "impact",
                "entity_ids": [store_id],
                "relation_kinds": [],
                "max_nodes": 2000,
                "max_depth": 1,
            }
            default_impact = client.call("archmeld_query", base)[1]
            excluded_impact = client.call(
                "archmeld_query", {**base, "exclude_entity_roles": ["test"]}
            )[1]
            default_entities = default_impact.get("entities") or []
            excluded_entities = excluded_impact.get("entities") or []
            check(
                "p11_exclude_test_from_impact",
                any(entity.get("role") == "test" for entity in default_entities)
                and not any(entity.get("role") == "test" for entity in excluded_entities),
                {
                    "default_relations": len(default_impact.get("relations") or []),
                    "excluded_relations": len(excluded_impact.get("relations") or []),
                    "default_has_test": any(
                        entity.get("role") == "test" for entity in default_entities
                    ),
                    "excluded_has_test": any(
                        entity.get("role") == "test" for entity in excluded_entities
                    ),
                },
            )

        cytoscape = client.call(
            "archmeld_query", {"kind": "symbols", "query": "cytoscape", "max_nodes": 20}
        )[1]
        check(
            "p16_vendored_excluded_by_default",
            not (cytoscape.get("entities") or []),
            {"default": len(cytoscape.get("entities") or [])},
        )
        config = client.call(
            "archmeld_query", {"kind": "symbols", "query": "Cargo.toml", "max_nodes": 20}
        )[1]
        config_excluded = client.call(
            "archmeld_query",
            {
                "kind": "symbols",
                "query": "Cargo.toml",
                "max_nodes": 20,
                "exclude_entity_roles": ["config"],
            },
        )[1]
        check(
            "p16_exclude_config",
            bool(config.get("entities"))
            and not (config_excluded.get("entities") or []),
            {
                "default": len(config.get("entities") or []),
                "excluded": len(config_excluded.get("entities") or []),
            },
        )

        # P12: kind separation and type-first ordering on the real Store name.
        store_all = client.call(
            "archmeld_query", {"kind": "symbols", "query": "::Store", "max_nodes": 50}
        )[1]
        store_kinds = [
            entity.get("kind") for entity in (store_all.get("entities") or [])
        ]
        store_modules = client.call(
            "archmeld_query",
            {
                "kind": "symbols",
                "query": "::Store",
                "max_nodes": 50,
                "entity_kinds": ["module"],
            },
        )[1]
        module_entities = store_modules.get("entities") or []
        check(
            "p12_kind_separation_and_order",
            {"type", "symbol", "module", "external"}.issubset(set(store_kinds))
            and store_kinds[:1] == ["type"]
            and bool(module_entities)
            and all(
                entity.get("kind") == "module"
                and entity.get("external") is True
                and not (entity.get("anchors") or [])
                for entity in module_entities
            ),
            {"kinds": store_kinds, "module_count": len(module_entities)},
        )

        # P23: auto brief must perform exactly one workspace review.
        brief = client.call("archmeld_brief", {"recipe": "auto"})[1]
        check(
            "p23_single_review",
            (brief.get("timings") or {}).get("review_calls") == 1,
            brief.get("timings"),
        )

        full_paths = None
        if review_paths:
            full = client.call("archmeld_review", {"detail": "full", "max_files": 5000})[1]
            full_paths = [item.get("path") for item in full.get("files", [])]
            check(
                "review_cursor_covers_full",
                sorted(set(review_paths)) == sorted(set(full_paths)),
                {"paged": len(review_paths), "full": len(full_paths)},
            )
        else:
            check("review_cursor_covers_full", True, "clean tree: no changed files")
    finally:
        client.close()

    report = {
        "state": str(state),
        "project": args.project,
        "metrics": metrics,
        "checks": checks,
        "passed": sum(1 for item in checks if item["ok"]),
        "failed": sum(1 for item in checks if not item["ok"]),
    }
    Path(args.out).write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(json.dumps({"passed": report["passed"], "failed": report["failed"]}))
    return 0 if report["failed"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
