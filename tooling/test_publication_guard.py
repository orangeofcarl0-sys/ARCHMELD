#!/usr/bin/env python3
"""Tests for the public-remote branch guard."""
from __future__ import annotations

import contextlib
import io
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import publication_guard as guard  # noqa: E402

PUBLIC = "https://github.com/<identity>-sys/ARCHMELD.git"
PRIVATE = "https://example.invalid/private/ARCHMELD.git"
SHA = "a" * 40
ZERO = "0" * 40


class PublicRemoteTests(unittest.TestCase):
    def test_public_url_detection(self) -> None:
        self.assertTrue(guard.is_public_remote(PUBLIC))
        self.assertTrue(guard.is_public_remote("git@github.com:<identity>-sys/ARCHMELD.git"))
        self.assertFalse(guard.is_public_remote(PRIVATE))
        self.assertFalse(guard.is_public_remote(""))

    def test_allowed_refs(self) -> None:
        self.assertTrue(guard.allowed_ref("refs/heads/main"))
        self.assertTrue(guard.allowed_ref("refs/heads/public-snapshot-2026-11-27"))
        self.assertFalse(guard.allowed_ref("refs/heads/archmeld/initial"))
        self.assertFalse(guard.allowed_ref("refs/tags/v1.0.0"))

    def test_public_branch_policy(self) -> None:
        ok, _ = guard.check_ref("refs/heads/main", PUBLIC)
        self.assertTrue(ok)
        ok, _ = guard.check_ref("refs/heads/public-snapshot-2026-11-27", PUBLIC)
        self.assertTrue(ok)
        ok, reason = guard.check_ref("refs/heads/archmeld/initial", PUBLIC)
        self.assertFalse(ok)
        self.assertIn("only refs/heads/main", reason)

    def test_private_remote_is_unrestricted(self) -> None:
        ok, _ = guard.check_ref("refs/heads/archmeld/initial", PRIVATE)
        self.assertTrue(ok)

    def test_delete_policy(self) -> None:
        ok, _ = guard.check_ref(
            "refs/heads/archmeld/initial", PUBLIC, local_sha=ZERO, remote_sha=SHA
        )
        self.assertTrue(ok)
        ok, reason = guard.check_ref(
            "refs/heads/main", PUBLIC, local_sha=ZERO, remote_sha=SHA
        )
        self.assertFalse(ok)
        self.assertIn("delete public main", reason)


class HookTests(unittest.TestCase):
    def test_hook_blocks_private_branch(self) -> None:
        lines = [f"refs/heads/archmeld/initial {SHA} refs/heads/archmeld/initial {ZERO}"]
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            code = guard.hook("origin", PUBLIC, lines)
        self.assertEqual(1, code)
        self.assertIn("public-snapshot-*", stderr.getvalue())

    def test_hook_allows_snapshot_branch(self) -> None:
        lines = [
            f"refs/heads/public-snapshot-2026-11-27 {SHA} "
            f"refs/heads/public-snapshot-2026-11-27 {ZERO}"
        ]
        self.assertEqual(0, guard.hook("origin", PUBLIC, lines))

    def test_hook_allows_private_remote(self) -> None:
        lines = [f"refs/heads/archmeld/initial {SHA} refs/heads/archmeld/initial {ZERO}"]
        self.assertEqual(0, guard.hook("private", PRIVATE, lines))

    def test_override_allows_with_warning(self) -> None:
        lines = [
            f"refs/heads/archmeld/initial {SHA} refs/heads/archmeld/initial {ZERO}"
        ]
        stderr = io.StringIO()
        with mock.patch.dict(os.environ, {"ARCHMELD_ALLOW_PUBLIC_BRANCH_PUSH": "1"}):
            with contextlib.redirect_stderr(stderr):
                code = guard.hook("origin", PUBLIC, lines)
        self.assertEqual(0, code)
        self.assertIn("overridden", stderr.getvalue())


class CiTests(unittest.TestCase):
    def test_ci_ref_from_github_ref(self) -> None:
        with mock.patch.dict(os.environ, {"GITHUB_REF": "refs/heads/main"}):
            self.assertEqual("refs/heads/main", guard.ci_ref())

    def test_ci_remote(self) -> None:
        with mock.patch.dict(
            os.environ,
            {
                "GITHUB_REPOSITORY": "<identity>-sys/ARCHMELD",
                "GITHUB_SERVER_URL": "https://github.com",
            },
        ):
            self.assertEqual(PUBLIC, guard.ci_remote_url())

    def test_ci_rejects_private_branch(self) -> None:
        with mock.patch.dict(
            os.environ,
            {
                "GITHUB_REF": "refs/heads/archmeld/initial",
                "GITHUB_REPOSITORY": "<identity>-sys/ARCHMELD",
                "GITHUB_SERVER_URL": "https://github.com",
            },
        ):
            self.assertFalse(guard.check_ref(guard.ci_ref(), guard.ci_remote_url())[0])


if __name__ == "__main__":
    unittest.main()
