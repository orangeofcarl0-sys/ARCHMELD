#!/usr/bin/env python3
"""Assert the exported graph binds checkout -> pricing.apply_discount only."""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--export", required=True)
    args = parser.parse_args()
    nodes = {}
    edges = []
    for line in Path(args.export).read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        item = json.loads(line)
        if item.get("type") == "node":
            nodes[item["id"]] = item
        elif item.get("type") == "edge":
            edges.append(item)

    def matches(node_id, name, path_suffix):
        node = nodes.get(node_id, {})
        props = node.get("properties", {})
        return (
            node.get("node_type") == "Function"
            and props.get("name") == name
            and str(props.get("path", "")).replace("\\", "/").endswith(path_suffix)
        )

    checkout = [node_id for node_id, node in nodes.items() if matches(node_id, "checkout", "checkout.py")]
    pricing = [node_id for node_id, node in nodes.items() if matches(node_id, "apply_discount", "pricing.py")]
    legacy = [node_id for node_id, node in nodes.items() if matches(node_id, "apply_discount", "legacy.py")]
    assert len(checkout) == 1, checkout
    assert len(pricing) == 1, pricing
    assert len(legacy) == 1, legacy
    calls = [
        edge
        for edge in edges
        if edge.get("edge_type") == "Calls"
        and edge.get("source") == checkout[0]
    ]
    targets = {edge.get("target") for edge in calls}
    assert pricing[0] in targets, calls
    assert legacy[0] not in targets, calls
    print(
        json.dumps(
            {
                "checkout": checkout[0],
                "pricing_apply_discount": pricing[0],
                "legacy_apply_discount": legacy[0],
                "calls": sorted(targets),
                "state": "ok",
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
