#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Self-tests for tooling/portable_env.py."""

from __future__ import annotations

import os
import re
import sys
import tempfile
import unittest
from pathlib import Path

TOOLING = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLING))

import portable_env  # noqa: E402


class TempRootTests(unittest.TestCase):
    def test_env_override(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = portable_env.temp_root(env={portable_env.ENV_TEMP: temporary})
            self.assertEqual(Path(temporary), root)

    def test_explicit_wins(self) -> None:
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            root = portable_env.temp_root(explicit=a, env={portable_env.ENV_TEMP: b})
            self.assertEqual(Path(a), root)

    def test_temp_dir_is_created(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = portable_env.temp_dir("unit-", root=temporary)
            self.assertTrue(path.is_dir())
            self.assertEqual(Path(temporary).resolve(), path.parent.resolve())

    def test_repository_root_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            portable_env.assert_safe_temp_root(portable_env.ROOT)


class ToolsRootTests(unittest.TestCase):
    def test_env_override(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            self.assertEqual(
                Path(temporary),
                portable_env.tools_root(env={portable_env.ENV_TOOLS: temporary}),
            )

    def test_explicit_wins(self) -> None:
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            self.assertEqual(
                Path(a),
                portable_env.tools_root(explicit=a, env={portable_env.ENV_TOOLS: b}),
            )


class ResolveToolTests(unittest.TestCase):
    def test_explicit_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "tool.exe"
            path.write_text("", encoding="utf-8")
            resolved = portable_env.resolve_tool("tool", explicit=path)
            self.assertEqual("explicit", resolved.source)
            self.assertEqual(path, resolved.path)
            self.assertTrue(resolved.exists)

    def test_env_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "tool.exe"
            path.write_text("", encoding="utf-8")
            resolved = portable_env.resolve_tool(
                "tool",
                env_var="ARCHMELD_TEST_TOOL",
                env={"ARCHMELD_TEST_TOOL": str(path)},
            )
            self.assertEqual("env", resolved.source)
            self.assertEqual(path, resolved.path)

    def test_tools_root_relative(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "bin" / "tool.exe"
            path.parent.mkdir(parents=True)
            path.write_text("", encoding="utf-8")
            resolved = portable_env.resolve_tool(
                "tool",
                relative="bin/tool.exe",
                env={portable_env.ENV_TOOLS: temporary},
            )
            self.assertEqual("tools_root", resolved.source)
            self.assertEqual(path, resolved.path)

    def test_missing(self) -> None:
        resolved = portable_env.resolve_tool(
            "definitely-not-a-real-tool",
            env={"PATH": ""},
        )
        self.assertEqual("missing", resolved.source)
        self.assertIsNone(resolved.path)


class SubprocessEnvTests(unittest.TestCase):
    def test_temp_and_no_machine_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            env = portable_env.subprocess_env(
                env={portable_env.ENV_TEMP: temporary, "PATH": ""},
            )
            self.assertEqual(temporary, env["TMP"])
            self.assertEqual(temporary, env["TEMP"])
            self.assertEqual(temporary, env["TMPDIR"])
            self.assertIn(str(Path(sys.executable).resolve().parent), env["PATH"])
            self.assertNotIn(chr(68) + ":/", env["PATH"])

    def test_rust_toolchain_on_windows(self) -> None:
        env = portable_env.subprocess_env(env={"PATH": ""})
        if os.name == "nt":
            self.assertEqual(portable_env.DEFAULT_RUST_TOOLCHAIN, env.get("RUSTUP_TOOLCHAIN"))

    def test_bindgen_args_use_forward_slashes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            include = root / "lib" / "gcc" / "x86_64-w64-mingw32" / "13.2.0" / "include"
            include.mkdir(parents=True)
            env = portable_env.subprocess_env(
                env={portable_env.ENV_GCC_ROOT: str(root), "PATH": ""},
            )
            args = env.get("BINDGEN_EXTRA_CLANG_ARGS", "")
            self.assertIn(include.as_posix(), args)
            self.assertNotIn("\\", args)

    def test_pinned_python_is_prepended(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            python = Path(temporary) / "python.exe"
            python.write_text("", encoding="utf-8")
            env = portable_env.subprocess_env(
                env={portable_env.ENV_PYTHON: str(python), "PATH": ""},
            )
            self.assertEqual(str(Path(temporary).resolve()), env["PATH"].split(os.pathsep)[0])


class GccIncludeTests(unittest.TestCase):
    def test_discovers_include_dirs_from_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            gcc_include = root / "lib" / "gcc" / "x86_64-w64-mingw32" / "13.2.0" / "include"
            target_include = root / "x86_64-w64-mingw32" / "include"
            gcc_include.mkdir(parents=True)
            target_include.mkdir(parents=True)
            dirs = portable_env.gcc_include_dirs(env={portable_env.ENV_GCC_ROOT: str(root)})
            self.assertIn(gcc_include.resolve(), dirs)
            self.assertIn(target_include.resolve(), dirs)


class LiteralTests(unittest.TestCase):
    def test_no_drive_absolute_literal_in_module(self) -> None:
        text = (TOOLING / "portable_env.py").read_text(encoding="utf-8")
        self.assertIsNone(re.search(r"[A-Za-z]:[\\/]", text))


if __name__ == "__main__":
    unittest.main(verbosity=2)
