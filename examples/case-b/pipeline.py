#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Run the simulated capture, estimator and UI; optionally emit a trace."""
import argparse
import json
import os
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.abspath(__file__))


def run_timed(argv, entity_id, spans, root_span, start):
    child_start = time.monotonic_ns()
    subprocess.run(argv, check=True)
    child_end = time.monotonic_ns()
    if entity_id:
        spans.append({
            "span_id": f"span.{len(spans) + 1}",
            "parent_span_id": root_span,
            "name": os.path.basename(argv[0]),
            "kind": "function",
            "start_ns": str(child_start),
            "end_ns": str(max(child_end, child_start + 1)),
            "process_id": str(os.getpid()),
            "thread_id": "main",
            "attributes": {"archmeld.entity_id": entity_id},
        })
    return child_end


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--capture", required=True)
    parser.add_argument("--work", required=True)
    parser.add_argument("--snapshot-id")
    parser.add_argument("--config-digest")
    parser.add_argument("--entities")
    parser.add_argument("--trace")
    args = parser.parse_args()
    os.makedirs(args.work, exist_ok=True)
    artifacts = os.path.join(ROOT, "artifacts")
    os.makedirs(artifacts, exist_ok=True)
    capture_csv = os.path.join(args.work, "capture.csv")
    estimate_json = os.path.join(args.work, "estimate.json")
    ui_html = os.path.join(args.work, "ui-report.html")
    root_start = time.monotonic_ns()
    root_span = None
    spans = []
    entities = {}
    if args.entities:
        with open(args.entities, encoding="utf-8") as handle:
            entities = json.load(handle)
    run_timed([args.capture, capture_csv], entities.get("capture"), spans, root_span, root_start)
    run_timed(
        [sys.executable, os.path.join(ROOT, "estimator", "estimator.py"), capture_csv, estimate_json],
        entities.get("estimator"),
        spans,
        root_span,
        root_start,
    )
    run_timed(
        [sys.executable, os.path.join(ROOT, "ui", "render.py"), estimate_json, ui_html],
        entities.get("ui"),
        spans,
        root_span,
        root_start,
    )
    shutil.copyfile(estimate_json, os.path.join(artifacts, "estimate.json"))
    shutil.copyfile(ui_html, os.path.join(artifacts, "ui-report.html"))
    with open(os.path.join(artifacts, "report.json"), "w", encoding="utf-8") as handle:
        json.dump({"tests": 3, "failures": 0, "errors": 0, "skipped": 0}, handle)
    if args.trace:
        trace = {
            "version": "1.0",
            "snapshot_id": args.snapshot_id,
            "config_digest": args.config_digest,
            "build_identity": {"kind": "pipeline", "variant": "case-b"},
            "capture": {
                "tool": "case-b-pipeline",
                "version": "1",
                "lossy": False,
                "loss_notes": [],
            },
            "clock": {"kind": "monotonic", "unit": "ns", "origin_ns": "0"},
            "spans": spans,
        }
        with open(args.trace, "w", encoding="utf-8") as handle:
            json.dump(trace, handle)
    print(json.dumps({"capture": capture_csv, "estimate": estimate_json, "ui": ui_html}))
    return
    0


if __name__ == "__main__":
    raise SystemExit(main())

