#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""B029 real-data evidence harness.

Reads one explicitly authorized recorded dataset, derives small numeric tables
outside the repository, evaluates the real `archmeld numeric-check` engine on
them, and writes an evidence record with source/artifact hashes. The raw data
and derived tables stay in the work directory and are never uploaded.

The positive checks are reproducibility gates on two independent extractions of
the same recorded values (statistical distribution and ULP distance), plus a
dimensional-equivalence check on the recorded millimetre unit. The model versus
Monte-Carlo difference is reported as an observation with no gate: they are
different estimators, so inventing a tolerance for them would be dishonest.
"""

import argparse
import hashlib
import json
import math
import os
import subprocess
import sys
from pathlib import Path

DEFAULT_SOURCE_SHA256 = "2b7cda9a4ab9f7d8672c888c5c803b872a98125dbaec8194d24ce14db8de0201"


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def write_text(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")
    return {"sha256": sha256(path), "size": path.stat().st_size}


def csv_table(values):
    return "value\n" + "".join(f"{value!r}\n" for value in values)


def csv_table_17g(values):
    return "value\n" + "".join(f"{value:.17g}\n" for value in values)


def run_check(binary, state, root, predicate_path, log_dir):
    argv = [
        str(binary),
        "numeric-check",
        "--state",
        str(state),
        "--root",
        str(root),
        "--predicate",
        str(predicate_path),
    ]
    completed = subprocess.run(argv, capture_output=True, text=True)
    stem = predicate_path.stem
    (log_dir / f"{stem}.stdout.json").write_text(completed.stdout, encoding="utf-8")
    (log_dir / f"{stem}.stderr.txt").write_text(completed.stderr, encoding="utf-8")
    if completed.returncode != 0:
        raise SystemExit(
            f"{predicate_path.name} exited {completed.returncode}: {completed.stderr.strip()}"
        )
    result = json.loads(completed.stdout)
    return {
        "argv": argv,
        "cwd": os.getcwd(),
        "exit_code": completed.returncode,
        "stdout_sha256": "sha256:" + hashlib.sha256(completed.stdout.encode()).hexdigest(),
        "stderr_sha256": "sha256:" + hashlib.sha256(completed.stderr.encode()).hexdigest(),
        "result": result["result"],
    }


def distribution(expected, actual):
    errors = [abs(left - right) for left, right in zip(expected, actual)]
    ordered = sorted(errors)
    count = len(errors)
    p95 = ordered[max(0, math.ceil(0.95 * count) - 1)] if count else 0.0
    return {
        "count": count,
        "mean_abs_error": repr(sum(errors) / count) if count else "0",
        "rms_error": repr(math.sqrt(sum(error * error for error in errors) / count))
        if count
        else "0",
        "p95_abs_error": repr(p95),
        "max_abs_error": repr(max(errors)) if errors else "0",
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, help="Authorized recorded JSON file")
    parser.add_argument("--workdir", required=True, help="Scratch directory outside the repository")
    parser.add_argument("--binary", required=True, help="Built archmeld executable")
    parser.add_argument("--evidence", required=True, help="Evidence JSON output path")
    parser.add_argument("--expected-sha256", default=DEFAULT_SOURCE_SHA256)
    args = parser.parse_args()

    source = Path(args.source).resolve()
    workdir = Path(args.workdir).resolve()
    artifacts = workdir / "artifacts"
    predicates = workdir / "predicates"
    logs = workdir / "logs"
    state = workdir / "state"
    for directory in (artifacts, predicates, logs):
        directory.mkdir(parents=True, exist_ok=True)

    source_bytes = source.read_bytes()
    source_sha256 = "sha256:" + hashlib.sha256(source_bytes).hexdigest()
    expected = args.expected_sha256
    if not expected.startswith("sha256:"):
        expected = "sha256:" + expected
    if source_sha256 != expected:
        raise SystemExit(f"source digest {source_sha256} does not match the authorized {expected}")
    payload = json.loads(source_bytes)
    rows = payload["rows"]
    if len(rows) != 342:
        raise SystemExit(f"expected 342 recorded rows, got {len(rows)}")

    def column(path):
        values = []
        for row in rows:
            current = row
            for key in path:
                current = current[key]
            value = float(current)
            if not math.isfinite(value):
                raise SystemExit(f"non-finite recorded value in {'.'.join(path)}")
            values.append(value)
        return values

    model_sd = column(("model", "conditional_SD_mm"))
    mc_sd = column(("MC", "all_output_SD_mm"))
    model_bias = column(("model", "bias_mm"))
    mc_bias = column(("MC", "all_output_bias_mm"))

    sd_a = csv_table(model_sd)
    sd_b = csv_table_17g(model_sd)
    if csv_table([float(text) for text in sd_b.strip().splitlines()[1:]]) != sd_a:
        raise SystemExit("two extraction paths do not round-trip to the same float bits")

    files = {
        "sd_a.csv": write_text(artifacts / "sd_a.csv", sd_a),
        "sd_b.csv": write_text(artifacts / "sd_b.csv", sd_b),
        "units_a.json": write_text(
            artifacts / "units_a.json",
            json.dumps(
                {
                    "columns": ["sd"],
                    "units": {"sd": "mm"},
                    "data": [[value] for value in model_sd],
                },
                ensure_ascii=False,
            )
            + "\n",
        ),
        "units_b.json": write_text(
            artifacts / "units_b.json",
            json.dumps(
                {
                    "columns": ["sd"],
                    "units": {"sd": "1e-3*m"},
                    "data": [[value] for value in model_sd],
                },
                ensure_ascii=False,
            )
            + "\n",
        ),
        "units_m.json": write_text(
            artifacts / "units_m.json",
            json.dumps(
                {
                    "columns": ["sd"],
                    "units": {"sd": "m"},
                    "data": [[value] for value in model_sd],
                },
                ensure_ascii=False,
            )
            + "\n",
        ),
        "units_custom.json": write_text(
            artifacts / "units_custom.json",
            json.dumps(
                {
                    "columns": ["sd"],
                    "units": {"sd": "mm_rf"},
                    "data": [[value] for value in model_sd],
                },
                ensure_ascii=False,
            )
            + "\n",
        ),
    }

    statistical = {
        "expected_artifact": "sd_a.csv",
        "actual_artifact": "sd_b.csv",
        "mode": "statistical",
        "statistics": {
            "mean_abs_error_max": "0",
            "rms_error_max": "0",
            "max_abs_error_max": "0",
            "p95_abs_error_max": "0",
        },
        "nonfinite_policy": "reject",
    }
    ulp = {
        "expected_artifact": "sd_a.csv",
        "actual_artifact": "sd_b.csv",
        "mode": "ulp",
        "max_ulps": "0",
        "nonfinite_policy": "reject",
    }
    dimension = {
        "expected_artifact": "units_a.json",
        "actual_artifact": "units_b.json",
        "mode": "absolute_relative",
        "atol": "0",
        "rtol": "0",
        "unit_mode": "dimension",
        "nonfinite_policy": "reject",
    }
    dimension_negative = {
        "expected_artifact": "units_a.json",
        "actual_artifact": "units_m.json",
        "mode": "absolute_relative",
        "atol": "0",
        "rtol": "0",
        "unit_mode": "dimension",
        "nonfinite_policy": "reject",
    }
    dimension_custom = {
        "expected_artifact": "units_custom.json",
        "actual_artifact": "units_b.json",
        "mode": "absolute_relative",
        "atol": "0",
        "rtol": "0",
        "unit_mode": "dimension",
        "unit_definitions": {"mm_rf": "mm"},
        "nonfinite_policy": "reject",
    }
    statistical_sample = {
        "expected_artifact": "sd_a.csv",
        "actual_artifact": "sd_b.csv",
        "mode": "statistical",
        "normalization": "sample",
        "statistics": {"variance_max": "0", "stddev_max": "0"},
        "nonfinite_policy": "reject",
    }
    matlab_eps = {
        "expected_artifact": "sd_a.csv",
        "actual_artifact": "sd_b.csv",
        "tolerance_model": "matlab_eps",
        "eps_multiplier": "0",
        "atol": "0",
        "rtol": "0",
        "nonfinite_policy": "reject",
    }
    predicate_files = {}
    for name, predicate in (
        ("statistical", statistical),
        ("statistical_sample", statistical_sample),
        ("ulp", ulp),
        ("matlab_eps", matlab_eps),
        ("dimension", dimension),
        ("dimension_custom", dimension_custom),
        ("dimension_negative", dimension_negative),
    ):
        path = predicates / f"{name}.json"
        write_text(path, json.dumps(predicate, indent=2, ensure_ascii=False) + "\n")
        predicate_files[name] = path

    binary = Path(args.binary).resolve()
    checks = {}
    for name in (
        "statistical",
        "statistical_sample",
        "ulp",
        "matlab_eps",
        "dimension",
        "dimension_custom",
    ):
        checks[name] = run_check(binary, state, artifacts, predicate_files[name], logs)
        if checks[name]["result"]["state"] != "passed":
            raise SystemExit(f"{name} expected passed, got {checks[name]['result']['state']}")
    negative = run_check(binary, state, artifacts, predicate_files["dimension_negative"], logs)
    if negative["result"]["state"] != "failed":
        raise SystemExit(f"dimension negative control expected failed, got {negative['result']['state']}")
    checks["dimension_negative_control"] = negative

    evidence = {
        "schema_version": "1.0",
        "generated_by": "tooling/b029_real_data.py",
        "platform": {
            "system": sys.platform,
            "python": sys.version.split()[0],
        },
        "authorization": {
            "basis": "the user replied 授权 to the B029 authorization request",
            "scope": "read-only local processing of the named recorded file; no upload; raw data and derived tables stay outside the repository",
            "recorded_at": "2026-11-01",
        },
        "source": {
            "name": source.name,
            "sha256": source_sha256,
            "bytes": len(source_bytes),
            "rows": len(rows),
            "selected_columns": [
                "model.conditional_SD_mm",
                "MC.all_output_SD_mm",
                "model.bias_mm",
                "MC.all_output_bias_mm",
            ],
        },
        "extraction": {
            "code": "tooling/b029_real_data.py",
            "code_sha256": sha256(Path(__file__).resolve()),
            "artifacts": files,
            "note": "sd_a uses Python repr; sd_b uses .17g and is asserted to reparse to the same floats",
        },
        "checks": checks,
        "observed_difference": {
            "gate": "none",
            "reason": "the model and the Monte Carlo estimator are different estimators; no frozen tolerance exists, so the difference is reported only",
            "sd_mm_model_vs_mc": distribution(model_sd, mc_sd),
            "bias_mm_model_vs_mc": distribution(model_bias, mc_bias),
        },
        "limitations": [
            "the positive checks prove extraction/serialization reproducibility on real recorded values, not scientific agreement between the model and Monte Carlo",
            "dimensional analysis covers the declared SI table and refuses affine/logarithmic units",
            "MATLAB semantics are explicitly unsupported and rejected by the predicate engine",
            "the raw data was not copied into the repository or uploaded",
        ],
    }
    evidence_path = Path(args.evidence).resolve()
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    evidence_path.write_text(json.dumps(evidence, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"evidence": str(evidence_path), "sha256": sha256(evidence_path)}))


if __name__ == "__main__":
    main()
