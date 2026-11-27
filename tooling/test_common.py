#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Self-tests for tooling/common.py."""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import common


class HashTests(unittest.TestCase):
    def test_bytes_and_prefix(self) -> None:
        self.assertEqual(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            common.sha256_bytes(b""),
        )
        self.assertEqual(
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            common.sha256_bytes(b"abc", prefix="sha256:"),
        )
        self.assertEqual(
            common.sha256_bytes("hé".encode("utf-8")),
            common.sha256_text("hé"),
        )

    def test_file_stream_and_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "artifact.bin"
            path.write_bytes(b"payload")
            self.assertEqual(common.sha256_bytes(b"payload"), common.sha256_file(path))
            self.assertEqual(
                "sha256:" + common.sha256_bytes(b"payload"),
                common.sha256_file(path, prefix="sha256:"),
            )
            missing = Path(temporary) / "missing.bin"
            with self.assertRaises(FileNotFoundError):
                common.sha256_file(missing)
            self.assertIsNone(common.sha256_file(missing, missing_ok=True))


class JsonTests(unittest.TestCase):
    def test_round_trip_and_mkdir(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "nested" / "record.json"
            value = {"b": 2, "a": "hé"}
            common.write_json(path, value)
            self.assertEqual(value, common.read_json(path))
            text = path.read_text(encoding="utf-8")
            self.assertTrue(text.endswith("\n"))
            self.assertIn('"hé"', text)

    def test_no_mkdir_fails_when_parent_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "missing" / "record.json"
            with self.assertRaises(FileNotFoundError):
                common.write_json(path, {"a": 1}, mkdir=False)


class RunTests(unittest.TestCase):
    def test_success_strip_and_tail(self) -> None:
        result = common.run_capture(
            [sys.executable, "-c", "print('  hello  ')"], strip=True
        )
        self.assertEqual(0, result["exit"])
        self.assertEqual("hello", result["stdout"])
        self.assertFalse(result["timed_out"])
        tailed = common.run_capture(
            [sys.executable, "-c", "print('abcdefghij')"], tail=4
        )
        self.assertEqual("hij\n", tailed["stdout"])

    def test_nonzero_and_check(self) -> None:
        result = common.run_capture(
            [sys.executable, "-c", "import sys; sys.exit(3)"]
        )
        self.assertEqual(3, result["exit"])
        with self.assertRaises(subprocess.CalledProcessError):
            common.run_capture(
                [sys.executable, "-c", "import sys; sys.exit(3)"], check=True
            )

    def test_timeout(self) -> None:
        result = common.run_capture(
            [sys.executable, "-c", "import time; time.sleep(2)"], timeout=0.05
        )
        self.assertTrue(result["timed_out"])
        self.assertIsNone(result["exit"])

    def test_missing_executable(self) -> None:
        result = common.run_capture(["definitely-not-a-real-binary-archmeld"])
        self.assertEqual(127, result["exit"])
        self.assertFalse(result["timed_out"])

    def test_run_checked(self) -> None:
        self.assertEqual(
            "ok",
            common.run_checked(sys.executable, "-c", "print('ok')"),
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
