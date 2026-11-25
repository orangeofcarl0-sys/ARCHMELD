#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Self-tests for tooling/upstream.py.

Run from the repository root:

    python tooling/test_upstream.py

The tests never touch the network: `git ls-remote` is injected where needed.
"""

from __future__ import annotations

import json
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
    def test_check_reports_update(self) -> None:
        registry = upstream.load_registry()
        original = upstream.ls_remote
        upstream.ls_remote = lambda remote, ref: {"remote": remote, "ref": ref, "commit": "c" * 40, "command": {}}
        try:
            result = upstream.check(registry, "codegraph")
        finally:
            upstream.ls_remote = original
        self.assertTrue(result["update_available"])
        self.assertEqual("c" * 40, result["latest"])

    def test_check_reports_up_to_date(self) -> None:
        registry = upstream.load_registry()
        pin = upstream.upstreams_by_id(registry)["codegraph"]["pin"]["commit"]
        original = upstream.ls_remote
        upstream.ls_remote = lambda remote, ref: {"remote": remote, "ref": ref, "commit": pin, "command": {}}
        try:
            result = upstream.check(registry, "codegraph")
        finally:
            upstream.ls_remote = original
        self.assertFalse(result["update_available"])


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
    def test_diff_current_upstream_is_clean(self) -> None:
        registry = upstream.load_registry()
        result = upstream.upstream_diff(registry, "codegraph", "upstream/main")
        self.assertFalse(result["update_available"])
        self.assertEqual([], result["conflicts"])
        self.assertEqual(0, result["merge_exit_code"])


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
