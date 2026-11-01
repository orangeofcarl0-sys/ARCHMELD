#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Static UI example consuming the real estimator output. No remote assets."""
import html
import json
import sys


def main(argv):
    if len(argv) != 3:
        print("usage: render.py <estimate.json> <report.html>", file=sys.stderr)
        return 2
    with open(argv[1], encoding="utf-8") as handle:
        estimate = json.load(handle)
    rows = "".join(
        f"<tr><td>{index}</td><td>{html.escape(str(row[0]))}</td></tr>"
        for index, row in enumerate(estimate["data"])
    )
    document = (
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">"
        "<title>Case B estimator output</title>"
        "<style>body{font:14px system-ui;margin:24px}table{border-collapse:collapse}"
        "td,th{border:1px solid #ccc;padding:4px 8px}</style></head><body>"
        "<h1>Case B estimator output</h1>"
        "<p>Unit: V · shape: [32,1] · simulated C++ capture input</p>"
        "<table><thead><tr><th>n</th><th>voltage (V)</th></tr></thead>"
        f"<tbody>{rows}</tbody></table>"
        "<p>Static report; no scripts or remote assets.</p>"
        "</body></html>"
    )
    with open(argv[2], "w", encoding="utf-8") as handle:
        handle.write(document)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
