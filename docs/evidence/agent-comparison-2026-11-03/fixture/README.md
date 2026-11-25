# Synthetic fixture for the 2026-11-03 same-agent comparison

This directory is a copy of the working tree the agents inspected. It is
synthetic test data, not production code.

- Origin repository: `<workspace>/agent-bench-vizIZe/repo`
- Origin git HEAD: `df2a5d13e7a67c9e460517b7f6a859bc909d16d2`
- Origin status at experiment time: `M src/change.py`
- `src/change.py` here is the live (modified) file: `value * 0.8`
- `base/change.py` is the HEAD version: `value * 0.9`
- `live.patch` is the uncommitted diff shown to both agents

The file copies retain the CRLF endings used by the origin repository, so the
digests in `docs/release-evidence/AGENT-COMPARISON-2026-11-03.json` remain
reproducible.
