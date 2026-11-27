#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Self-tests for tooling/upstream.py.

Run from the repository root:

    python tooling/test_upstream.py

The tests never touch the network: `git ls-remote` is injected where needed.
"""

from __future__ import annotations

import contextlib
import hashlib
import io
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

TOOLING = Path(__file__).resolve().parent
ROOT = TOOLING.parent
sys.path.insert(0, str(TOOLING))

import upstream  # noqa: E402


class RegistryValidationTests(unittest.TestCase):
    def test_current_registry_is_valid(self) -> None:
        registry = upstream.load_registry()
        checks = upstream.validate_registry(registry)
        failures = [check for check in checks if check["status"] == "fail"]
        self.assertEqual([], failures)

    def test_duplicate_id_is_rejected(self) -> None:
        registry = {
            "schema_version": "1.0",
            "upstreams": [
                {
                    "id": "x",
                    "reuse_mode": "thin_fork",
                    "source": {"url": "https://example.invalid/x"},
                    "pin": {"kind": "commit", "commit": "a" * 40},
                    "contract": {"id": "c", "version": "1"},
                    "update_policy": {
                        "mode": "manual_merge_with_drill",
                        "auto_follow": False,
                        "auto_merge": False,
                        "auto_pin_update": False,
                    },
                },
                {
                    "id": "x",
                    "reuse_mode": "thin_fork",
                    "source": {"url": "https://example.invalid/x"},
                    "pin": {"kind": "commit", "commit": "b" * 40},
                    "contract": {"id": "c", "version": "1"},
                    "update_policy": {
                        "mode": "manual_merge_with_drill",
                        "auto_follow": False,
                        "auto_merge": False,
                        "auto_pin_update": False,
                    },
                },
            ],
        }
        checks = upstream.validate_registry(registry)
        self.assertTrue(any(check["status"] == "fail" and "duplicate" in check["detail"] for check in checks))

    def test_unknown_reuse_mode_is_rejected(self) -> None:
        registry = {
            "schema_version": "1.0",
            "upstreams": [
                {
                    "id": "x",
                    "reuse_mode": "magic",
                    "source": {"url": "https://example.invalid/x"},
                    "pin": {"kind": "version", "version": "1"},
                    "contract": {"id": "c", "version": "1"},
                    "update_policy": {
                        "mode": "manual_merge_with_drill",
                        "auto_follow": False,
                        "auto_merge": False,
                        "auto_pin_update": False,
                    },
                }
            ],
        }
        checks = upstream.validate_registry(registry)
        self.assertTrue(any("unknown reuse_mode" in check["detail"] for check in checks if check["status"] == "fail"))

    def test_auto_pin_update_must_be_false(self) -> None:
        registry = {
            "schema_version": "1.0",
            "upstreams": [
                {
                    "id": "x",
                    "reuse_mode": "thin_fork",
                    "source": {"url": "https://example.invalid/x"},
                    "pin": {"kind": "version", "version": "1"},
                    "contract": {"id": "c", "version": "1"},
                    "update_policy": {
                        "mode": "manual_merge_with_drill",
                        "auto_follow": False,
                        "auto_merge": False,
                        "auto_pin_update": True,
                    },
                }
            ],
        }
        checks = upstream.validate_registry(registry)
        self.assertTrue(any("auto_pin_update" in check["detail"] for check in checks if check["status"] == "fail"))


class DependencyIsolationTests(unittest.TestCase):
    def test_no_codegraph_dependency_passes(self) -> None:
        metadata = {
            "packages": [
                {"id": "core", "name": "archmeld-core"},
                {"id": "serde", "name": "serde"},
            ],
            "resolve": {
                "nodes": [
                    {"id": "core", "deps": [{"pkg": "serde"}]},
                    {"id": "serde", "deps": []},
                ]
            },
        }
        checks = upstream.check_dependency_isolation(metadata)
        self.assertEqual("pass", checks[0]["status"])

    def test_transitive_codegraph_dependency_fails(self) -> None:
        metadata = {
            "packages": [
                {"id": "core", "name": "archmeld-core"},
                {"id": "mid", "name": "mid"},
                {"id": "graph", "name": "codegraph"},
            ],
            "resolve": {
                "nodes": [
                    {"id": "core", "deps": [{"pkg": "mid"}]},
                    {"id": "mid", "deps": [{"pkg": "graph"}]},
                    {"id": "graph", "deps": []},
                ]
            },
        }
        checks = upstream.check_dependency_isolation(metadata)
        self.assertEqual("fail", checks[0]["status"])
        self.assertIn("codegraph", checks[0]["detail"])

    def test_missing_package_fails(self) -> None:
        checks = upstream.check_dependency_isolation({"packages": [], "resolve": {"nodes": []}})
        self.assertEqual("fail", checks[0]["status"])


class PinRecordTests(unittest.TestCase):
    def test_resolve_pointer(self) -> None:
        value = {"a": [{"b": "c"}]}
        self.assertEqual("c", upstream.resolve_pointer(value, "/a/0/b"))
        with self.assertRaises(KeyError):
            upstream.resolve_pointer(value, "/a/1/b")

    def test_python_const_regex(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "tool.py"
            path.write_text('"codegraph_commit": "' + "a" * 40 + '",\n', encoding="utf-8")
            record = {
                "path": str(path),
                "kind": "python_const",
                "regex": '"codegraph_commit":\\s*"([0-9a-f]{40})"',
            }
            # The helper reads relative to ROOT, so exercise the regex directly.
            import re

            match = re.search(record["regex"], path.read_text(encoding="utf-8"))
            self.assertIsNotNone(match)
            self.assertEqual("a" * 40, match.group(1))


class CheckCommandTests(unittest.TestCase):
    def test_check_release_only_untagged_pin_is_not_branch_tracked(self) -> None:
        registry = upstream.load_registry()
        pin = upstream.upstreams_by_id(registry)["codegraph"]["pin"]["commit"]
        original = upstream.ls_remote

        def fail(remote: str, ref: str) -> dict:
            raise AssertionError("release-only check must not query a branch")

        upstream.ls_remote = fail
        try:
            result = upstream.check(registry, "codegraph")
        finally:
            upstream.ls_remote = original
        self.assertFalse(result["update_available"])
        self.assertEqual(pin, result["latest"])
        self.assertTrue(result["release_only"])
        self.assertIn("release:0.21.2", result["ref"])

    def test_check_release_tag_reports_update(self) -> None:
        registry = {
            "upstreams": [
                {
                    "id": "demo",
                    "source": {
                        "url": "https://example.invalid/demo.git",
                        "remote": "upstream",
                    },
                    "release": {"version": "1.0.0", "tag": "v1.0.0"},
                    "pin": {"kind": "commit", "commit": "a" * 40},
                    "update_policy": {
                        "mode": "manual_release_merge_with_drill",
                        "release_only": True,
                    },
                }
            ]
        }
        original = upstream.ls_remote
        upstream.ls_remote = lambda remote, ref: {
            "remote": remote,
            "ref": ref,
            "commit": "c" * 40,
            "command": {},
        }
        try:
            result = upstream.check(registry, "demo")
        finally:
            upstream.ls_remote = original
        self.assertTrue(result["update_available"])
        self.assertEqual("c" * 40, result["latest"])
        self.assertEqual("refs/tags/v1.0.0", result["ref"])


class RemoteFallbackTests(unittest.TestCase):
    def _registry(self) -> dict:
        return {
            "upstreams": [
                {
                    "id": "demo",
                    "reuse_mode": "artifact_import",
                    "source": {
                        "url": "https://example.invalid/demo.git",
                        "remote": "missing-remote",
                        "ref": None,
                    },
                    "pin": {"kind": "commit", "commit": "a" * 40},
                    "contract": {"id": "demo", "version": "1"},
                    "update_policy": {
                        "mode": "regenerate_artifact_and_run_contract_tests",
                        "auto_follow": False,
                        "auto_merge": False,
                        "auto_pin_update": False,
                    },
                }
            ]
        }

    def test_status_falls_back_from_remote_name_to_url(self) -> None:
        calls: list[tuple[str, str]] = []

        def fake(remote: str, ref: str) -> dict:
            calls.append((remote, ref))
            if remote == "missing-remote":
                raise RuntimeError("no such remote")
            return {"remote": remote, "ref": ref, "commit": "b" * 40, "command": {}}

        original = upstream.ls_remote
        upstream.ls_remote = fake
        try:
            result = upstream.status(self._registry(), remote=True)
        finally:
            upstream.ls_remote = original
        row = result["upstreams"][0]
        self.assertEqual("b" * 40, row["latest"])
        self.assertEqual(
            {"target": "https://example.invalid/demo.git", "ref": "HEAD"}, row["probe"]
        )
        self.assertEqual(
            [("missing-remote", "HEAD"), ("https://example.invalid/demo.git", "HEAD")],
            calls,
        )

    def test_check_uses_url_and_head_when_registry_has_no_remote_or_ref(self) -> None:
        calls: list[tuple[str, str]] = []

        def fake(remote: str, ref: str) -> dict:
            calls.append((remote, ref))
            if remote == "missing-remote":
                raise RuntimeError("no such remote")
            return {"remote": remote, "ref": ref, "commit": "b" * 40, "command": {}}

        original = upstream.ls_remote
        upstream.ls_remote = fake
        try:
            result = upstream.check(self._registry(), "demo")
        finally:
            upstream.ls_remote = original
        self.assertTrue(result["update_available"])
        self.assertEqual("HEAD", result["ref"])
        self.assertEqual(
            [("missing-remote", "HEAD"), ("https://example.invalid/demo.git", "HEAD")],
            calls,
        )

    def test_check_reports_all_failed_targets(self) -> None:
        def failing(remote: str, ref: str) -> dict:
            raise RuntimeError("offline")

        original = upstream.ls_remote
        upstream.ls_remote = failing
        try:
            with self.assertRaises(SystemExit) as raised:
                upstream.check(self._registry(), "demo")
        finally:
            upstream.ls_remote = original
        self.assertIn("offline", str(raised.exception))


class StatusOutputTests(unittest.TestCase):
    def test_status_out_writes_json(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "status.json"
            with contextlib.redirect_stdout(io.StringIO()):
                code = upstream.main(["status", "--json", "--out", str(out)])
            self.assertEqual(0, code)
            data = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual("tooling/upstream.py status", data["generated_by"])
        self.assertEqual(8, len(data["upstreams"]))

    def test_status_fail_on_update_returns_three(self) -> None:
        original = upstream.status
        upstream.status = lambda registry, remote=False: {
            "schema_version": "1.0",
            "upstreams": [{"id": "demo", "update_available": True}],
            "warnings": [],
        }
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                code = upstream.main(["status", "--json", "--fail-on-update"])
        finally:
            upstream.status = original
        self.assertEqual(3, code)


class WorkflowTests(unittest.TestCase):
    def test_weekly_workflow_is_read_only(self) -> None:
        path = ROOT / ".github/workflows/archmeld-upstream.yml"
        text = path.read_text(encoding="utf-8")
        for needle in (
            "schedule:",
            "workflow_dispatch:",
            "contents: read",
            "--fail-on-update",
            "upload-artifact",
            "upstream-probes.json",
        ):
            self.assertIn(needle, text)
        self.assertNotIn("git push", text)
        self.assertNotIn("secrets.", text)


class ProbeVersionTests(unittest.TestCase):
    def test_version_key_pads_and_orders(self) -> None:
        self.assertEqual(upstream.version_key("v0.3"), upstream.version_key("0.3.0"))
        self.assertLess(upstream.version_key("1.2.3"), upstream.version_key("1.10.0"))
        self.assertLess(upstream.version_key("1.2.3-rc1"), upstream.version_key("1.2.3"))
        self.assertIsNone(upstream.version_key("banana"))

    def test_latest_tag_filters_prefix_and_prerelease(self) -> None:
        tags = [
            "refs/tags/v1.0.0",
            "refs/tags/v1.2.0-rc1",
            "refs/tags/v1.10.0",
            "refs/tags/other-9.9.9",
        ]
        probe = {
            "tag_prefix": "v",
            "version_regex": r"^\d+\.\d+\.\d+$",
            "include_prerelease": False,
        }
        self.assertEqual("1.10.0", upstream.latest_tag(tags, probe))


class ProbeMetadataTests(unittest.TestCase):
    def test_current_registry_probes_validate(self) -> None:
        registry = upstream.load_registry()
        checks = upstream.validate_registry(registry)
        failures = [check for check in checks if check["status"] == "fail"]
        self.assertEqual([], failures)

    def test_pin_from_resolves_lock_versions(self) -> None:
        registry = upstream.load_registry()
        lsp = upstream.upstreams_by_id(registry)["lsp"]
        pins = {probe["id"]: upstream.probe_pin(probe) for probe in lsp["probes"]}
        self.assertEqual("1.1.408", pins["pyright"])
        self.assertEqual("4.3.4", pins["typescript-language-server"])
        self.assertEqual("0.20.0", pins["gopls"])
        self.assertEqual("1.64.0", pins["jdtls"])


class ProbeCommandTests(unittest.TestCase):
    def _registry(self) -> dict:
        return {
            "upstreams": [
                {
                    "id": "demo",
                    "probes": [
                        {"id": "a", "kind": "npm", "package": "a", "pin": "1.0.0"},
                        {"id": "skip", "kind": "none", "note": "not probed"},
                    ],
                }
            ]
        }

    def test_probes_report_update_skip_and_no_behind(self) -> None:
        original_pin = upstream.probe_pin
        original_latest = upstream.probe_latest
        upstream.probe_pin = lambda probe: "1.0.0"
        upstream.probe_latest = lambda probe: ("1.1.0", "npm:a")
        try:
            result = upstream.probes(self._registry())
        finally:
            upstream.probe_pin = original_pin
            upstream.probe_latest = original_latest
        probes = result["upstreams"][0]["probes"]
        self.assertEqual("update", probes[0]["status"])
        self.assertTrue(probes[0]["update_available"])
        self.assertIsNone(probes[0]["behind"])
        self.assertEqual("skipped", probes[1]["status"])
        self.assertIsNone(probes[1]["latest"])

    def test_probe_error_is_a_warning(self) -> None:
        def failing(probe: dict) -> tuple[str, str]:
            raise RuntimeError("offline")

        original_pin = upstream.probe_pin
        original_latest = upstream.probe_latest
        upstream.probe_pin = lambda probe: "1.0.0"
        upstream.probe_latest = failing
        try:
            result = upstream.probes(self._registry())
        finally:
            upstream.probe_pin = original_pin
            upstream.probe_latest = original_latest
        self.assertEqual("error", result["upstreams"][0]["probes"][0]["status"])
        self.assertTrue(any("offline" in warning for warning in result["warnings"]))

    def test_unsupported_version_format_is_not_false(self) -> None:
        original_pin = upstream.probe_pin
        original_latest = upstream.probe_latest
        upstream.probe_pin = lambda probe: "1.0.0"
        upstream.probe_latest = lambda probe: ("release-one", "npm:a")
        try:
            result = upstream.probes(self._registry())
        finally:
            upstream.probe_pin = original_pin
            upstream.probe_latest = original_latest
        probe = result["upstreams"][0]["probes"][0]
        self.assertEqual("unsupported", probe["status"])
        self.assertIsNone(probe["update_available"])

    def test_probes_out_writes_json(self) -> None:
        original = upstream.probes
        upstream.probes = lambda registry, only_id=None: {
            "schema_version": "1.0",
            "upstreams": [{"id": "demo", "probes": []}],
            "warnings": [],
        }
        try:
            with tempfile.TemporaryDirectory() as tmp:
                out = Path(tmp) / "probes.json"
                with contextlib.redirect_stdout(io.StringIO()):
                    code = upstream.main(["probes", "--json", "--out", str(out)])
                self.assertEqual(0, code)
                data = json.loads(out.read_text(encoding="utf-8"))
        finally:
            upstream.probes = original
        self.assertEqual("demo", data["upstreams"][0]["id"])


class StaleLiteralTests(unittest.TestCase):
    ALLOWLIST = {"test_upstream.py"}

    def test_no_stale_pin_literal_in_tooling(self) -> None:
        offenders = []
        for path in sorted(TOOLING.glob("*.py")):
            if path.name in self.ALLOWLIST:
                continue
            text = path.read_text(encoding="utf-8")
            if "590c2ca7dd7b37bd1d41caf45fac535b6522f969" in text:
                offenders.append(path.name)
        self.assertEqual([], offenders)

    def test_no_stale_at077_partial_literal_in_tooling(self) -> None:
        offenders = []
        for path in sorted(TOOLING.glob("*.py")):
            if path.name in self.ALLOWLIST:
                continue
            text = path.read_text(encoding="utf-8")
            if "AT077 partial" in text:
                offenders.append(path.name)
        self.assertEqual([], offenders)

    def test_no_drive_absolute_literal_in_tooling(self) -> None:
        pattern = re.compile(r"(?<![A-Za-z0-9])[A-Za-z]:[\\/]")
        offenders = []
        for path in sorted(TOOLING.glob("*.py")):
            text = path.read_text(encoding="utf-8")
            for number, line in enumerate(text.splitlines(), 1):
                if pattern.search(line):
                    offenders.append(f"{path.name}:{number}")
        self.assertEqual([], offenders)


class TraceFixtureAuditTests(unittest.TestCase):
    def _root(self, tmp: str) -> tuple[Path, dict]:
        root = Path(tmp)
        (root / "adapters/traces/fixtures").mkdir(parents=True)
        (root / "crates/archmeld-core/tests").mkdir(parents=True)
        (root / "crates/archmeld-core/tests/perfetto_native.rs").write_text("", encoding="utf-8")
        data = b"trace fixture"
        fixture = {
            "id": "fixture-a",
            "path": "adapters/traces/fixtures/a.pftrace",
            "sha256": "sha256:" + hashlib.sha256(data).hexdigest(),
            "size": len(data),
            "producer": "test producer",
            "limitations": ["test limitation"],
            "tests": ["perfetto_native"],
        }
        (root / fixture["path"]).write_bytes(data)
        (root / "adapters/traces/fixtures.lock.json").write_text(
            json.dumps({"fixtures": [fixture]}), encoding="utf-8"
        )
        return root, fixture

    def test_matching_lock_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._root(tmp)
            checks = upstream.check_trace_fixtures(root)
        self.assertEqual("pass", checks[0]["status"], checks[0])

    def test_hash_mismatch_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root, fixture = self._root(tmp)
            (root / fixture["path"]).write_bytes(b"changed")
            checks = upstream.check_trace_fixtures(root)
        self.assertEqual("fail", checks[0]["status"], checks[0])
        self.assertIn("sha256 mismatch", checks[0]["detail"])

    def test_missing_provenance_and_test_are_warnings(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root, fixture = self._root(tmp)
            lock_path = root / "adapters/traces/fixtures.lock.json"
            fixture.pop("producer")
            fixture.pop("tests")
            lock_path.write_text(json.dumps({"fixtures": [fixture]}), encoding="utf-8")
            checks = upstream.check_trace_fixtures(root)
        self.assertEqual("warn", checks[0]["status"], checks[0])
        self.assertIn("no producer", checks[0]["detail"])
        self.assertIn("no consuming tests", checks[0]["detail"])


class VerifyCurrentTreeTests(unittest.TestCase):
    def test_verify_has_no_failures_without_cargo(self) -> None:
        registry = upstream.load_registry()
        result = upstream.verify(registry, skip_cargo=True)
        failures = [check for check in result["checks"] if check["status"] == "fail"]
        self.assertEqual([], failures)

    def test_sealed_pack_digest_check_is_a_warning_not_a_failure(self) -> None:
        checks = upstream.check_sealed_pack()
        self.assertTrue(checks)
        self.assertTrue(all(check["status"] in {"pass", "warn", "info"} for check in checks))

    def test_public_snapshot_check_does_not_fail_a_private_tree(self) -> None:
        checks = upstream.check_public_snapshot()
        self.assertTrue(checks)
        self.assertEqual([], [check for check in checks if check["status"] == "fail"])


@unittest.skipUnless((ROOT / ".git").exists(), "requires a git checkout")
class PatchManifestTests(unittest.TestCase):
    def test_current_manifest_is_complete(self) -> None:
        manifest = json.loads((ROOT / "patches/codegraph/manifest.json").read_text(encoding="utf-8"))
        result = upstream.patch_check(manifest["base_commit"], manifest)
        self.assertTrue(result["manifest_complete"], result)
        self.assertEqual([], result["unaccounted_paths"])
        self.assertEqual([], result["missing_paths"])

    def test_incomplete_manifest_is_detected(self) -> None:
        manifest = json.loads((ROOT / "patches/codegraph/manifest.json").read_text(encoding="utf-8"))
        reduced = dict(manifest)
        reduced["entries"] = [entry for entry in manifest["entries"] if entry["id"] == "P001"]
        result = upstream.patch_check(manifest["base_commit"], reduced)
        self.assertFalse(result["manifest_complete"])
        self.assertTrue(result["unaccounted_paths"])


@unittest.skipUnless((ROOT / ".git").exists(), "requires a git checkout")
class DiffTests(unittest.TestCase):
    def test_diff_current_release_commit_is_clean(self) -> None:
        registry = upstream.load_registry()
        pin = upstream.upstreams_by_id(registry)["codegraph"]["pin"]["commit"]
        result = upstream.upstream_diff(registry, "codegraph", pin)
        self.assertFalse(result["update_available"])
        self.assertEqual([], result["conflicts"])
        self.assertEqual(0, result["merge_exit_code"])

    def test_diff_refuses_branch_ref_for_release_only(self) -> None:
        registry = upstream.load_registry()
        with self.assertRaises(SystemExit) as raised:
            upstream.upstream_diff(registry, "codegraph", "upstream/main")
        self.assertIn("release-only", str(raised.exception))


class RollbackDrillTests(unittest.TestCase):
    def _registry(self) -> dict:
        return {
            "upstreams": [
                {
                    "id": "codegraph",
                    "pin": {"kind": "commit", "commit": "a" * 40},
                }
            ]
        }

    def _fake_run(self, calls: list[list[str]], ancestor_ok: bool, conflicts: str) -> object:
        def fake(argv: list[str], cwd: object = None, timeout: int = 120, text_limit: int | None = 65536) -> dict:
            calls.append(argv)
            if argv[:3] == ["git", "status", "--porcelain"]:
                return {"exit_code": 0, "stdout": "", "stderr": ""}
            if argv[:3] == ["git", "rev-parse", "HEAD"]:
                return {"exit_code": 0, "stdout": "a" * 40 + "\n", "stderr": ""}
            if argv[:3] == ["git", "rev-parse", "--verify"]:
                return {"exit_code": 0, "stdout": "b" * 40 + "\n", "stderr": ""}
            if argv[:3] == ["git", "merge-base", "--is-ancestor"]:
                return {"exit_code": 0 if ancestor_ok else 1, "stdout": "", "stderr": ""}
            if argv[:4] == ["git", "worktree", "add", "--detach"]:
                return {"exit_code": 0, "stdout": "", "stderr": ""}
            if argv[:4] == ["git", "worktree", "remove", "--force"]:
                return {"exit_code": 0, "stdout": "", "stderr": ""}
            if argv[:5] == ["git", "revert", "--no-commit", "-m", "1"]:
                return {"exit_code": 1 if conflicts else 0, "stdout": "", "stderr": ""}
            if argv[:3] == ["git", "diff", "--name-only"]:
                return {"exit_code": 0, "stdout": conflicts, "stderr": ""}
            if argv[:3] == ["git", "revert", "--abort"]:
                return {"exit_code": 0, "stdout": "", "stderr": ""}
            return {"exit_code": 0, "stdout": "", "stderr": ""}

        return fake

    def test_full_matrix_and_revert_conflicts_are_recorded(self) -> None:
        calls: list[list[str]] = []
        original_run = upstream.run
        original_temp = upstream.portable.temp_dir
        original_matrix = upstream.drill_matrix
        upstream.run = self._fake_run(
            calls,
            ancestor_ok=True,
            conflicts="crates/codegraph-server/src/watcher.rs\n",
        )
        upstream.portable.temp_dir = lambda prefix: Path(tempfile.gettempdir()) / "archmeld-rollback-test"
        upstream.drill_matrix = lambda worktree, env, matrix: [{"exit_code": 0, "matrix": matrix}]
        try:
            result = upstream.rollback_drill(
                self._registry(),
                "codegraph",
                "903bfe0a",
                run_matrix=True,
                matrix="full",
                probe_revert="1ca0ae8b",
            )
        finally:
            upstream.run = original_run
            upstream.portable.temp_dir = original_temp
            upstream.drill_matrix = original_matrix
        self.assertEqual("passed", result["status"])
        self.assertEqual("full", result["matrix"])
        self.assertEqual(1, len(result["tests"]))
        self.assertTrue(result["target_is_ancestor"])
        self.assertEqual("conflicted", result["revert_probe"]["status"])
        self.assertEqual(
            ["crates/codegraph-server/src/watcher.rs"],
            result["revert_probe"]["conflicts"],
        )
        self.assertTrue(any(argv[:3] == ["git", "revert", "--abort"] for argv in calls))

    def test_non_ancestor_target_is_rejected(self) -> None:
        calls: list[list[str]] = []
        original_run = upstream.run
        upstream.run = self._fake_run(calls, ancestor_ok=False, conflicts="")
        try:
            with self.assertRaises(SystemExit) as raised:
                upstream.rollback_drill(
                    self._registry(), "codegraph", "deadbeef", run_matrix=False
                )
        finally:
            upstream.run = original_run
        self.assertIn("not an ancestor", str(raised.exception))

    def test_clean_revert_is_recorded_without_abort(self) -> None:
        calls: list[list[str]] = []
        original_run = upstream.run
        original_temp = upstream.portable.temp_dir
        upstream.run = self._fake_run(calls, ancestor_ok=True, conflicts="")
        upstream.portable.temp_dir = lambda prefix: Path(tempfile.gettempdir()) / "archmeld-rollback-test-clean"
        try:
            result = upstream.rollback_drill(
                self._registry(),
                "codegraph",
                "903bfe0a",
                run_matrix=False,
                probe_revert="1ca0ae8b",
            )
        finally:
            upstream.run = original_run
            upstream.portable.temp_dir = original_temp
        self.assertEqual("clean", result["revert_probe"]["status"])
        self.assertEqual([], result["revert_probe"]["conflicts"])
        self.assertFalse(any(argv[:3] == ["git", "revert", "--abort"] for argv in calls))


class RecordPlannerTests(unittest.TestCase):
    def make_root(self, temp: Path) -> Path:
        (temp / "third_party").mkdir(parents=True)
        (temp / "patches/codegraph").mkdir(parents=True)
        old = "a" * 40
        new = "b" * 40
        registry = {
            "schema_version": "1.0",
            "upstreams": [
                {
                    "id": "codegraph",
                    "pin": {"kind": "commit", "commit": old, "version": "0.21.2"},
                    "last_drill": None,
                    "last_checked": None,
                }
            ],
        }
        (temp / "upstreams.lock.json").write_text(json.dumps(registry), encoding="utf-8")
        (temp / "toolchains.lock.json").write_text(
            json.dumps({"pinned": {"codegraph_commit": old}}), encoding="utf-8"
        )
        (temp / "AGENTS.md").write_text(f"pin {old}\n", encoding="utf-8")
        (temp / "UPSTREAM_PATCHES.md").write_text(f"Base: {old}\n", encoding="utf-8")
        (temp / "third_party/NOTICE.md").write_text(f"Pinned commit: `{old}`\nprevious pin `{'c' * 40}`\n", encoding="utf-8")
        (temp / "patches/codegraph/manifest.json").write_text(
            json.dumps({"base_commit": old}), encoding="utf-8"
        )
        return temp

    def test_plan_updates_all_pin_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(Path(temporary))
            evidence = {"status": "passed", "to_commit": "b" * 40, "to_version": "0.22.0"}
            plan = upstream.plan_record_updates(root, evidence, "docs/release-evidence/UPSTREAM-test.json")
            self.assertTrue(plan["changed"])
            paths = {update["path"] for update in plan["updates"]}
            self.assertEqual(
                {
                    "upstreams.lock.json",
                    "toolchains.lock.json",
                    "AGENTS.md",
                    "UPSTREAM_PATCHES.md",
                    "third_party/NOTICE.md",
                    "patches/codegraph/manifest.json",
                },
                paths,
            )
            registry = next(update for update in plan["updates"] if update["path"] == "upstreams.lock.json")
            self.assertIn("b" * 40, registry["content"])
            notice = next(update for update in plan["updates"] if update["path"] == "third_party/NOTICE.md")
            self.assertEqual(1, notice["content"].count("b" * 40))
            self.assertIn("c" * 40, notice["content"])

    def test_same_pin_is_no_change(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(Path(temporary))
            evidence = {"status": "passed", "to_commit": "a" * 40}
            plan = upstream.plan_record_updates(root, evidence, "evidence.json")
            self.assertFalse(plan["changed"])
            self.assertEqual([], plan["updates"])

    def test_non_passed_evidence_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(Path(temporary))
            evidence = {"status": "conflicted", "to_commit": "b" * 40}
            with self.assertRaises(SystemExit):
                upstream.plan_record_updates(root, evidence, "evidence.json")


if __name__ == "__main__":
    unittest.main(verbosity=2)
