#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Self-tests for tooling/public_snapshot.py.

Marker strings are assembled at runtime so this test file itself never contains
a literal private path.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLING = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLING))

import public_snapshot  # noqa: E402

DRIVE_PATH = "X" + ":" + "\\" + "private" + "\\" + "repo"
MSYS_PATH = "/x/" + "Users/" + "someone/repo"
REMOTE = "operator@" + "<private-ip>.13" + ":2222"


class RedactTests(unittest.TestCase):
    def test_drive_path(self) -> None:
        result = public_snapshot.redact_text(f"cwd={DRIVE_PATH}", [], [])
        self.assertNotIn(DRIVE_PATH, result)
        self.assertIn("<path>", result)

    def test_msys_path(self) -> None:
        result = public_snapshot.redact_text(f"root={MSYS_PATH}", [], [])
        self.assertNotIn(MSYS_PATH, result)
        self.assertIn("<path>", result)

    def test_remote_and_private_ip(self) -> None:
        result = public_snapshot.redact_text(f"host={REMOTE}", [], [])
        self.assertNotIn("<private-ip>.13", result)
        self.assertIn("<remote-host>", result)

    def test_user_json_field(self) -> None:
        result = public_snapshot.redact_text('{"user":"<identity>"}', [], [])
        self.assertIn("<identity>", result)
        self.assertNotIn("someone", result)

    def test_private_map(self) -> None:
        result = public_snapshot.redact_text("secret-value", [("secret-value", "<redacted>")], [])
        self.assertEqual("<redacted>", result)


class ScanTests(unittest.TestCase):
    def test_public_mode_fails_on_unclassified_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "leak.txt").write_text(f"cwd={DRIVE_PATH}\n", encoding="utf-8")
            result = public_snapshot.scan_tree(root, mode="public")
            self.assertEqual("failed", result["status"])
            self.assertGreaterEqual(result["unclassified"], 1)

    def test_private_mode_warns(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "leak.txt").write_text(f"cwd={DRIVE_PATH}\n", encoding="utf-8")
            result = public_snapshot.scan_tree(root, mode="private")
            self.assertEqual("passed_with_warnings", result["status"])

    def test_policy_allow_and_omit(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "leak.txt").write_text(f"cwd={DRIVE_PATH}\n", encoding="utf-8")
            policy = {
                "schema_version": "1.0",
                "omit": [{"glob": "ignored.txt", "reason": "test"}],
                "allow": [{"glob": "leak.txt", "reason": "synthetic allow"}],
            }
            result = public_snapshot.scan_tree(root, mode="public", policy=policy)
            self.assertEqual("passed", result["status"])
            self.assertEqual(0, result["unclassified"])


class ExportTests(unittest.TestCase):
    def test_export_omits_and_redacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary) / "repo"
            repo.mkdir()
            subprocess.run(["git", "init"], cwd=repo, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.name", "test"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.email", "test@example.invalid"], cwd=repo, check=True)
            (repo / "keep.txt").write_text(f"cwd={DRIVE_PATH}\n", encoding="utf-8")
            (repo / "omit.txt").write_text("omit me\n", encoding="utf-8")
            (repo / "policy.json").write_text(
                json.dumps({"schema_version": "1.0", "omit": [{"glob": "omit.txt", "reason": "test"}], "allow": []}),
                encoding="utf-8",
            )
            subprocess.run(["git", "add", "-A"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-m", "fixture"], cwd=repo, check=True, capture_output=True)

            old_root = public_snapshot.ROOT
            old_policy = public_snapshot.POLICY
            public_snapshot.ROOT = repo
            public_snapshot.POLICY = repo / "policy.json"
            try:
                out = Path(temporary) / "out"
                result = public_snapshot.export_tree(out, "HEAD", None, None)
            finally:
                public_snapshot.ROOT = old_root
                public_snapshot.POLICY = old_policy
            self.assertGreater(result["file_count"], 0)
            self.assertTrue((out / "keep.txt").is_file())
            self.assertFalse((out / "omit.txt").exists())
            self.assertNotIn(DRIVE_PATH, (out / "keep.txt").read_text(encoding="utf-8"))
            self.assertTrue((out / "public-snapshot-manifest.json").is_file())


if __name__ == "__main__":
    unittest.main(verbosity=2)
