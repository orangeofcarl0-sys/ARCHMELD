#!/usr/bin/env python3
"""Run the fast or evidence test lane with one canonical environment.

The fast lane excludes the two wall-clock budget tests and the four AT067
language tests from the routine loop, then runs the remaining core tests,
archmeld-api lib tests, the parser suite and the stable server tests. It can
optionally use the prebuilt RocksDB archive, the lld linker and sccache; each
switch is recorded in the JSON fingerprint.

The evidence lane runs the four AT067 tests through baseline/run_cmd.py and
then verify_b030_evidence.py. It always uses source-built RocksDB, BFD and
-j 2 so the acceptance evidence stays canonical.

Examples:
  py -3.11 tooling/test_lane.py --lane fast --dry-run
  py -3.11 tooling/test_lane.py --lane fast
  py -3.11 tooling/test_lane.py --lane server
  py -3.11 tooling/test_lane.py --lane evidence
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tooling"))
import build_helper  # noqa: E402
import portable_env  # noqa: E402

CORE_FAST_EXCLUDED = {
    "benchmark",
    "t3_stress",
    "language_matrix",
    "language_runtime_linkage",
    "language_medium_projects",
    "language_rename_apply",
}
CORE_TIMING = {"benchmark", "t3_stress"}
# These tests spawn real language servers / nested cargo checks. They took
# 84s + 56s + 37s on the reference run, so the routine lane leaves them to
# `full` or an explicit `--include-external`.
CORE_EXTERNAL = {
    "rust_pack",
    "java_pack",
    "go_pack",
    "csharp_pack",
    "cpp_pack",
    "language_packs",
    "lsp_roundtrip",
}
LANGUAGE_TESTS = [
    "language_runtime_linkage",
    "language_medium_projects",
    "language_rename_apply",
    "language_matrix",
]
SERVER_TESTS = [
    "index_coverage",
    "cross_file_trait",
    "archmeld_state_root",
    "parser_graph_properties_test",
]
PARSER_PACKAGES = [
    "codegraph-rust",
    "codegraph-python",
    "codegraph-php",
    "codegraph-parser-api",
]


class LaneError(RuntimeError):
    pass


def core_tests(include_timing: bool, include_external: bool) -> list[str]:
    directory = ROOT / "crates" / "archmeld-core" / "tests"
    names = sorted(path.stem for path in directory.glob("*.rs"))
    if not names:
        raise LaneError(f"no core integration tests found in {directory}")
    excluded = set(CORE_FAST_EXCLUDED)
    if include_timing:
        excluded -= CORE_TIMING
    if not include_external:
        excluded |= CORE_EXTERNAL
    selected = [name for name in names if name not in excluded]
    if not selected:
        raise LaneError("core lane selection is empty")
    return selected


def cargo(env: dict[str, str], args: list[str]) -> list[str]:
    return build_helper.resolve_executable(["cargo", "test", *args], env)


def base_cargo_args(args: argparse.Namespace, jobs: int) -> list[str]:
    result = ["--offline", "--locked", "-j", str(jobs)]
    if args.target_dir:
        result.extend(["--target-dir", args.target_dir])
    return result


def lane_commands(
    lane: str, args: argparse.Namespace, jobs: int
) -> list[tuple[str, list[str]]]:
    common = base_cargo_args(args, jobs)
    if lane == "core":
        return [
            (
                "core",
                ["-p", "archmeld-core"]
                + [item for name in core_tests(args.include_timing, args.include_external) for item in ("--test", name)]
                + ["--no-fail-fast"]
                + common,
            )
        ]
    if lane == "server":
        command = ["-p", "codegraph-server", "--lib"]
        for name in SERVER_TESTS:
            command.extend(["--test", name])
        return [("server", command + ["--no-fail-fast"] + common)]
    if lane == "parser":
        parser_packages = [item for name in PARSER_PACKAGES for item in ("-p", name)]
        return [("parser", parser_packages + ["--no-fail-fast"] + common)]
    if lane == "fast":
        return [
            (
                "core",
                ["-p", "archmeld-core"]
                + [item for name in core_tests(args.include_timing, args.include_external) for item in ("--test", name)]
                + ["--no-fail-fast"]
                + common,
            ),
            ("api", ["-p", "archmeld-api", "--lib", "--no-fail-fast"] + common),
            (
                "parser",
                [item for name in PARSER_PACKAGES for item in ("-p", name)]
                + ["--no-fail-fast"]
                + common,
            ),
            (
                "server",
                ["-p", "codegraph-server", "--lib"]
                + [item for name in SERVER_TESTS for item in ("--test", name)]
                + ["--no-fail-fast"]
                + common,
            ),
        ]
    if lane == "full":
        return [
            (
                "full",
                ["-p", "archmeld-core", "-p", "archmeld-api", "--no-fail-fast"]
                + common,
            ),
            (
                "server",
                ["-p", "codegraph-server", "--lib", "--no-fail-fast"] + common,
            ),
        ]
    raise LaneError(f"lane {lane} has no cargo groups")


def fast_overrides(args: argparse.Namespace, env: dict[str, str]) -> tuple[dict[str, str | None], dict[str, object]]:
    """Build fast-lane cache/linker/sccache overrides with explicit provenance."""
    overrides: dict[str, str | None] = {}
    provenance: dict[str, object] = {
        "rocksdb": "source",
        "linker": "bfd",
        "sccache": "off",
    }
    link_args: list[str] = []

    if args.rocksdb != "source":
        import rocksdb_cache  # noqa: PLC0415

        cache = rocksdb_cache.resolve(mode=args.rocksdb, env=env)
        overrides.update(cache["env"])
        link_args.extend(cache["link_args"])
        provenance["rocksdb"] = "prebuilt"
        provenance["rocksdb_cache"] = cache["dir"]
        provenance["rocksdb_sha256"] = cache["sha256"]

    if args.linker != "bfd":
        import link_probe  # noqa: PLC0415

        probe = link_probe.resolve(mode=args.linker, env=env)
        link_args.extend(probe["link_args"])
        provenance["linker"] = probe["linker"]
        provenance["lld"] = probe.get("version")

    if args.sccache != "off":
        import install_sccache  # noqa: PLC0415

        cache = install_sccache.resolve(mode=args.sccache, env=env)
        if cache.get("enabled"):
            overrides.update(cache["env"])
            provenance["sccache"] = "on"
            provenance["sccache_version"] = cache.get("version")
            provenance["sccache_dir"] = cache.get("cache_dir")
        else:
            provenance["sccache"] = "off"
            provenance["sccache_reason"] = cache.get("reason")

    if link_args:
        overrides["RUSTFLAGS"] = " ".join(f"-Clink-arg={item}" for item in link_args)
    return overrides, provenance


def lane_env(
    lane: str, args: argparse.Namespace
) -> tuple[dict[str, str], dict[str, object]]:
    if lane in {"fast", "core", "server", "parser"}:
        base = build_helper.cargo_env(mode="fast")
        overrides, provenance = fast_overrides(args, base)
        env = build_helper.cargo_env(mode="fast", env=base, overrides=overrides)
    else:
        # Canonical/evidence lanes deliberately drop every fast-lane knob.
        env = build_helper.cargo_env(mode="canonical")
        provenance = {
            "rocksdb": "source",
            "linker": "bfd",
            "sccache": "off",
        }
    if os.name == "nt" and not (
        env.get("ARCHMELD_MINGW_BIN") or env.get("ARCHMELD_STRAWBERRY_BIN")
    ):
        raise LaneError(
            "MinGW is not resolved; set ARCHMELD_MINGW_BIN to the UCRT64 bin "
            "directory so gcc/g++ and the runtime DLLs are found"
        )
    # Tests resolve these through support::require_tool. The tools root has
    # nested JDK/dotnet/Go trees but deliberately not gcc/g++, so fill them
    # from the canonical MinGW PATH instead of making every operator repeat it.
    gcc = portable_env.which("gcc", env=env)
    gxx = portable_env.which("g++", env=env)
    if gcc is not None and not env.get("ARCHMELD_GCC"):
        env["ARCHMELD_GCC"] = str(gcc)
    if gxx is not None and not env.get("ARCHMELD_GXX"):
        env["ARCHMELD_GXX"] = str(gxx)
    if not env.get("ARCHMELD_PYTHON"):
        env["ARCHMELD_PYTHON"] = sys.executable
    return env, provenance


def run_command(label: str, command: list[str], env: dict[str, str]) -> dict[str, object]:
    print(f"$ {' '.join(command)}", flush=True)
    started = time.monotonic()
    result = subprocess.run(command, cwd=ROOT, env=env)
    return {
        "label": label,
        "argv": command,
        "exit_code": result.returncode,
        "duration_s": round(time.monotonic() - started, 3),
    }


def git_head() -> str:
    result = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=30,
    )
    return result.stdout.strip() if result.returncode == 0 else "unknown"


def run_evidence(args: argparse.Namespace) -> int:
    required = ["ARCHMELD_TOOLS_ROOT", "ARCHMELD_GCC", "ARCHMELD_GXX"]
    missing = [name for name in required if not os.environ.get(name)]
    if missing:
        print(
            "evidence lane requires strict AT067 environment; missing: "
            + ", ".join(missing),
            file=sys.stderr,
        )
        return 2
    python = os.environ.get("ARCHMELD_PYTHON") or sys.executable
    strict = {
        "ARCHMELD_TOOLS_ROOT": os.environ["ARCHMELD_TOOLS_ROOT"],
        "ARCHMELD_REQUIRE_TOOLS": "1",
        "ARCHMELD_WRITE_EVIDENCE": "1",
        "ARCHMELD_GCC": os.environ["ARCHMELD_GCC"],
        "ARCHMELD_GXX": os.environ["ARCHMELD_GXX"],
        "ARCHMELD_PYTHON": python,
    }
    run_start = int(time.time())
    head = git_head()
    stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    groups: list[dict[str, object]] = []
    for test in LANGUAGE_TESTS:
        record_id = f"b030-{test}-perf-{stamp}"
        command = [
            sys.executable,
            str(ROOT / "baseline" / "run_cmd.py"),
            "--id",
            record_id,
            "--note",
            "AT067 evidence lane",
        ]
        for key, value in strict.items():
            command.extend(["--env", f"{key}={value}"])
        command.extend(
            [
                "--",
                "cargo",
                "test",
                "-p",
                "archmeld-core",
                "--test",
                test,
                "--offline",
                "--locked",
                "-j",
                "2",
            ]
        )
        result = run_command(test, command, dict(os.environ))
        groups.append(result)
        if result["exit_code"] != 0:
            break
    verify = run_command(
        "verify",
        [
            sys.executable,
            str(ROOT / "baseline" / "run_cmd.py"),
            "--id",
            f"b030-verify-perf-{stamp}",
            "--note",
            "AT067 verifier",
            "--",
            sys.executable,
            str(ROOT / "tooling" / "verify_b030_evidence.py"),
            "--since",
            str(run_start),
            "--expect-head",
            head,
        ],
        dict(os.environ),
    )
    groups.append(verify)
    return 0 if all(group["exit_code"] == 0 for group in groups) else 1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--lane",
        choices=["fast", "core", "server", "parser", "full", "evidence"],
        default="fast",
    )
    parser.add_argument("--jobs", type=int, default=None)
    parser.add_argument("--target-dir", default=None)
    parser.add_argument("--include-timing", action="store_true")
    parser.add_argument(
        "--include-external",
        action="store_true",
        help="include LSP/nested-cargo pack tests (rust/java/go/csharp/cpp/language_packs/lsp_roundtrip)",
    )
    parser.add_argument("--rocksdb", choices=["auto", "prebuilt", "source"], default="auto")
    parser.add_argument("--linker", choices=["auto", "bfd", "lld"], default="auto")
    parser.add_argument("--sccache", choices=["auto", "on", "off"], default="auto")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    started = time.monotonic()
    if args.lane == "evidence":
        result = run_evidence(args)
        if args.json:
            (ROOT / "target" / "test-lane-latest.json").write_text(
                json.dumps(
                    {
                        "lane": "evidence",
                        "duration_s": round(time.monotonic() - started, 3),
                        "exit_code": result,
                        "head": git_head(),
                    },
                    ensure_ascii=False,
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
        return result

    jobs = args.jobs if args.jobs is not None else (
        4 if args.lane in {"fast", "core", "server", "parser"} else 2
    )
    env, provenance = lane_env(args.lane, args)
    commands = lane_commands(args.lane, args, jobs)
    fingerprint = {
        "lane": args.lane,
        "jobs": jobs,
        "target_dir": args.target_dir,
        "include_timing": args.include_timing,
        "include_external": args.include_external,
        "provenance": provenance,
        "rust_toolchain": env.get("RUSTUP_TOOLCHAIN"),
        "mingw_bin": env.get("ARCHMELD_MINGW_BIN") or env.get("ARCHMELD_STRAWBERRY_BIN"),
        "bindgen_args": env.get("BINDGEN_EXTRA_CLANG_ARGS"),
        "cxxflags": env.get("CXXFLAGS"),
        "cxxflags_target": env.get("CXXFLAGS_x86_64_pc_windows_gnu"),
        "temp": env.get("TMP"),
        "cargo_incremental": env.get("CARGO_INCREMENTAL"),
        "rocksdb_lib_dir": env.get("ROCKSDB_LIB_DIR"),
        "rustflags": env.get("RUSTFLAGS"),
        "rustc_wrapper": env.get("RUSTC_WRAPPER"),
    }
    payload = {
        "fingerprint": fingerprint,
        "commands": [{"label": label, "argv": command} for label, command in commands],
    }
    if args.dry_run:
        print(json.dumps(payload, ensure_ascii=False, indent=2))
        return 0

    groups: list[dict[str, object]] = []
    failed = False
    for label, command in commands:
        result = run_command(label, cargo(env, command), env)
        groups.append(result)
        if result["exit_code"] != 0:
            failed = True
            break

    summary = {
        "lane": args.lane,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "duration_s": round(time.monotonic() - started, 3),
        "head": git_head(),
        "fingerprint": fingerprint,
        "groups": groups,
        "exit_code": 1 if failed else 0,
    }
    if args.json:
        target = ROOT / "target" / "test-lane-latest.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: summary[key] for key in ("lane", "duration_s", "exit_code")}, ensure_ascii=False))
    return summary["exit_code"]


if __name__ == "__main__":
    raise SystemExit(main())
