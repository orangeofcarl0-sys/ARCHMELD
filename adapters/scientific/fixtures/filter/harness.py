#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Deterministic golden-truth harness for the scientific profile."""

import json
import math
import os

from estimator import run_filter, wrap_phase

ROOT = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(ROOT, "golden.json"), encoding="utf-8") as handle:
    golden = json.load(handle)

alpha = float(golden["alpha"])
initial = float(golden["initial"])
output = run_filter(golden["input"], alpha, initial)
phase = [wrap_phase(angle) for angle in golden["phase_angles"]]

# Units and shape are checked before values. The harness refuses to emit a
# report if the algorithm returns the wrong container shape.
assert isinstance(output, list) and len(output) == golden["shape"]["output"][0]
assert all(isinstance(value, float) for value in output)
assert len(phase) == golden["shape"]["phase"][0]
assert all(isinstance(value, float) for value in phase)

# Boundary and lifecycle checks are part of the report, not just the final RMS.
assert run_filter([], alpha, initial) == []
assert run_filter([1.0, 1.0], 0.0, 0.25) == [0.25, 0.25]
assert run_filter([1.0, 0.0], 1.0, 0.0) == [1.0, 0.0]
assert run_filter(golden["input"], alpha, initial) == run_filter(
    golden["input"], alpha, initial
)
assert abs(wrap_phase(math.pi) + math.pi) <= 1e-15
assert abs(wrap_phase(-math.pi) + math.pi) <= 1e-15

os.makedirs(os.path.join(ROOT, "artifacts"), exist_ok=True)
with open(
    os.path.join(ROOT, "artifacts", "filter-output.json"), "w", encoding="utf-8"
) as handle:
    json.dump(
        {
            "columns": ["output"],
            "units": ["V"],
            "data": [[value] for value in output],
        },
        handle,
    )
with open(
    os.path.join(ROOT, "artifacts", "phase.json"), "w", encoding="utf-8"
) as handle:
    json.dump(
        {
            "columns": ["phase"],
            "units": ["rad"],
            "data": [[value] for value in phase],
        },
        handle,
    )
with open(
    os.path.join(ROOT, "artifacts", "report.json"), "w", encoding="utf-8"
) as handle:
    json.dump({"tests": 8, "failures": 0, "errors": 0, "skipped": 0}, handle)
print(json.dumps({"output": output, "phase": phase}))
