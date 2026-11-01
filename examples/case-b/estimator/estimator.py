#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""First-order estimator consuming the C++ capture CSV.

units: input V, output V; shape: N x 1; state: previous output in V.
update order: y[n] = alpha*x[n] + (1-alpha)*y[n-1].
"""
import csv
import json
import sys


def run_filter(samples, alpha=0.5, initial=0.0):
    output = []
    previous = initial
    for sample in samples:
        current = alpha * sample + (1.0 - alpha) * previous
        output.append(current)
        previous = current
    return output


def main(argv):
    if len(argv) != 3:
        print("usage: estimator.py <capture.csv> <estimate.json>", file=sys.stderr)
        return 2
    samples = []
    with open(argv[1], newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            samples.append(float(row["voltage_v"]))
    output = run_filter(samples)
    with open(argv[2], "w", encoding="utf-8") as handle:
        json.dump(
            {
                "columns": ["voltage"],
                "units": ["V"],
                "data": [[value] for value in output],
                "metadata": {
                    "dtype": "float64",
                    "alpha": "0.5",
                    "initial": "0.0",
                    "input_samples": len(samples),
                    "state_update_order": "y[n]=alpha*x[n]+(1-alpha)*y[n-1]",
                },
            },
            handle,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
