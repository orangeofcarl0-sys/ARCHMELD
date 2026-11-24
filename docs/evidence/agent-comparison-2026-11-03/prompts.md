# Same-agent comparison prompts (2026-11-03)

Both arms received the common block below verbatim. Only the final
`TOOL CONDITION` paragraph differed. The host launched two `general-purpose`
subagents in parallel on the same session model.

## Common task block

```text
You are participating in a controlled code-localization experiment. Work read-only; do not modify or create any file.

Repository: <workspace>\repo (small Python project).

TASK (identical for all conditions):
Q1. In src/late.py, the string literal 'E_UNIQUE_7F3' appears in a comment and in an assignment. Locate the assignment. Report: file path, 1-based line number, enclosing function name, and the 1-based line of that function's def.
Q2. What computation does late_target perform? Report the exact expression that produces the returned value and the 1-based line where that expression is evaluated.
Q3. Which function calls compute_metric, and at which file and 1-based line does that call occur?
Q4. The git working tree contains exactly one uncommitted modification. Report: changed file, changed line (1-based), enclosing function, old expression, new expression, the function that calls the changed function, and that caller function's file and 1-based call line.

Return ONLY one JSON object (no markdown fence, no prose before or after) with this shape:
{
  "localizations": [
    {"id":"Q1","file":"...","line":0,"symbol":"...","symbol_line":0},
    {"id":"Q2","expression":"...","line":0},
    {"id":"Q3","caller":"...","file":"...","line":0},
    {"id":"Q4","file":"...","line":0,"symbol":"...","old":"...","new":"...","caller":"...","caller_file":"...","caller_line":0}
  ],
  "conclusions": [
    {"id":"C1","statement":"...","evidence":[{"kind":"file_line","path":"...","line":0}]}
  ],
  "info_requests": [
    {"id":"I1","needed":"...","why":"..."}
  ],
  "uncertainties": [
    {"id":"U1","reason":"..."}
  ]
}

Rules: every line number must be supported by evidence you actually inspected. Do not invent or round numbers. For Q4 use git diff. If something cannot be determined with the tools you are allowed to use, list it in info_requests instead of guessing; do not pad info_requests with things you already answered. Put every claim that goes beyond the requested fields in conclusions, each with direct evidence. If no conclusions beyond the localizations are needed, use an empty array.
```

## Condition A (no ARCHMELD)

```text
TOOL CONDITION A (no ARCHMELD): You may use read-only shell commands (rg, grep, sed, awk, find, git, py/python) and file reads inside the repository. You must NOT use ARCHMELD in any form: no archmeld binary/CLI/MCP, no HTTP calls to localhost, and do not read <workspace>\state or serve.log. Do not inspect any directory outside the repository except standard tool binaries.
```

## Condition B (ARCHMELD available)

```text
TOOL CONDITION B (ARCHMELD available): ARCHMELD is an evidence service already running for this repo. Use it as your primary evidence source. Base URL http://127.0.0.1:6778, no authentication.
- GET /v1/status?project_id=project.agentbench&workspace_handle=ws.1f4df45471cbbd9f
- POST /v1/reviews/workspace-diff with JSON {"project_id":"project.agentbench","workspace_handle":"ws.1f4df45471cbbd9f"}
- POST /v1/query with JSON containing exactly: project_id "project.agentbench", snapshot_id "sha256:5c78e50c0dda756e52e1a6edc0ec55cac5e4576c6ccd479995fe9049a7728b21", kind ("symbols", "source", "impact", or "neighbors"), query, entity_ids, relation_kinds (use [] for all), max_nodes, max_depth (<=8), cursor null.
  Example source search: {"project_id":"project.agentbench","snapshot_id":"sha256:5c78e50c0dda756e52e1a6edc0ec55cac5e4576c6ccd479995fe9049a7728b21","kind":"source","query":"E_UNIQUE_7F3","entity_ids":[],"relation_kinds":[],"max_nodes":100,"max_depth":1}
  To find callers, query kind="symbols" for the callee name, then kind="impact" with its entity id and max_depth 1 (the reverse traversal returns incoming edges; static_call_candidate edges are candidates, not resolved calls).
- POST /v1/source with JSON {"project_id":"project.agentbench","snapshot_id":"sha256:5c78e50c0dda756e52e1a6edc0ec55cac5e4576c6ccd479995fe9049a7728b21","entity_id":"...","anchor_index":0,"max_bytes":200000}
You may use read-only file inspection (rg/sed/Read) to confirm exact source text, but every localization must be corroborated by an ARCHMELD response or a direct file line. Do not modify anything. Do not read or write the state directory directly; go through the API.
```
