#!/usr/bin/env bash
# Reproduce the M1 first-run and brief measurements on this workstation.
# The fixture and state are outside the repository so no source is uploaded.
set -euo pipefail

ROOT=/workspace/ARCHMELD
FIX=/workspace/m1-fixture
OUT="$ROOT/docs/evidence/m1-onboarding-2026-11-11"
BIN="$ROOT/target/debug/archmeld.exe"
MCP="$ROOT/target/debug/archmeld-mcp.exe"
HELPER="$ROOT/dist/build/archmeld-cli-0.1.0-windows-x64/bin/archmeld-index.exe"

export ARCHMELD_INDEX_HELPER="$HELPER"
export ARCHMELD_MCP_BINARY="$MCP"

# Reset the fixture so the change brief always sees a real modification.
printf 'def b():\n    return 2\n' > "$FIX/repo/src/b.py"

rm -rf "$FIX/state" "$FIX/state-rules" "$FIX/repo-rules"
cp -R "$FIX/repo" "$FIX/repo-rules"

start=$(date +%s%3N)
"$BIN" init --workspace "$FIX/repo" --state "$FIX/state" --project project.m1 --client codex \
  > "$OUT/init.json"
end=$(date +%s%3N)
printf '{"elapsed_ms": %s}\n' "$((end - start))" > "$OUT/init-timing.json"

"$BIN" brief --state "$FIX/state" --project project.m1 --markdown > "$OUT/brief-project.md"
"$BIN" init --workspace "$FIX/repo" --state "$FIX/state" --project project.m1 --client codex \
  > "$OUT/init-again.json"
printf 'def b():\n    return 5\n' > "$FIX/repo/src/b.py"
"$BIN" brief --state "$FIX/state" --project project.m1 --markdown > "$OUT/brief-change.md"
"$BIN" mcp install --client codex --state "$FIX/state" --project project.m1 --check \
  > "$OUT/mcp-install-check.json"

start=$(date +%s%3N)
"$BIN" init --workspace "$FIX/repo-rules" --state "$FIX/state-rules" --project project.m1r \
  --write --client generic > "$OUT/init-write-rules.json"
end=$(date +%s%3N)
printf '{"elapsed_ms": %s}\n' "$((end - start))" > "$OUT/init-write-timing.json"

"$BIN" init --workspace "$FIX/repo-rules" --state "$FIX/state-rules" --project project.m1r \
  --write --client generic > "$OUT/init-write-rules-2.json"
"$BIN" init --workspace "$FIX/repo-rules" --state "$FIX/state-rules" --project project.m1r \
  --check > "$OUT/init-check.json"

echo "M1 evidence written to $OUT"
