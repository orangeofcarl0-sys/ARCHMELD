# ARCHMELD self-analysis, 2026-11-26

Method: index the ARCHMELD development tree with the product's own CLI
(`archmeld index`, helper `archmeld_export`), then read it back only through the
product surfaces: `status`, `review-diff`, `brief`, `map status`,
`archmeld-mcp` (`archmeld_query`, `archmeld_inspect`, `archmeld_context`,
`archmeld_source`, `archmeld_capabilities`). Raw ledger aggregates are used only
to quantify what those surfaces return.

This is static evidence. No test, build or runtime command was executed against
the analyzed snapshot, no SCIP/LSP/trace artifact was imported, and candidate
relations are not resolved calls.

## 1. Scope and reproducibility

| Item | Value |
|---|---|
| Product | `target/debug/archmeld.exe`, `archmeld 0.1.0` |
| Product hashes | archmeld `be189df6…`, archmeld-mcp `109c19e6…`, helper `3deb42d2…` (rebuilt at HEAD) |
| Source tree | `<path> HEAD `7b1dc280`, `git status` clean at capture |
| Index helper | `target/debug/examples/archmeld_export.exe` built from the same tree |
| State | isolated temp ledger under `_archmeld_temp/self-analysis/state` |
| Snapshot | `sha256:66aaec6db1355ce1f7b4259a37ea417bdc84cafe1e38716c9f955a336d62a896` |
| Snapshot commit | `7b1dc2805214b287525109bf124bdf13fa12d92a` |
| Snapshot files | 1,828 files / 34,042,848 bytes |
| Export | 18 MB NDJSON, `sha256:7e53e3ab083a9b0b0c2599e00ddd9c9e9b5f742e4833b3f113fb67faf2c9f48e` |
| Export manifest | schema 1.1, `producer_commit` `7b1dc280`, `producer_dirty=true` (the helper was compiled while this report was untracked; the analyzed tree was clean) |
| Index wall time | 2 m 52 s total; helper 35.5 s |
| Host | Windows 11, Git Bash, MinGW runtime on PATH; Python 3.11 for the MCP driver |

An initial run used the pre-HEAD debug builds; it was discarded after a
binary/source timestamp check. The HEAD-built rerun reproduced the same
entity/relation counts and every finding below.

Commands (exact argv in `docs/evidence/self-analysis-2026-11-26/COMMANDS.json`):

```text
archmeld index --workspace <path> --state <state>
  --project archmeld.self --index-helper <archmeld_export.exe> --max-files 5000
archmeld status --state <state> --project archmeld.self --workspace <path>
archmeld review-diff --state <state> --project archmeld.self --workspace <path>
archmeld brief --state <state> --project archmeld.self --markdown
archmeld brief --state <state> --project archmeld.self --recipe project --markdown
archmeld map status --state <state> --project archmeld.self --workspace <path>
archmeld-mcp --state <state> --project archmeld.self   # JSON-RPC session, 8 tools
```

## 2. What the product built

### 2.1 Snapshot capture

Coverage is reported as **partial**, and the omissions are enumerated:

| Reason | Files |
|---|---|
| gitignored | 6,552 |
| binary | 24 |
| too_large (> 8 MiB) | 47 |
| excluded_dir | 7 |
| **captured** | **1,828** |

The 8,458 walked files are the whole workspace minus `.git`/`target`/etc. This
is the intended policy: capture content, exclude ignored and non-text files,
and say so. Three generated tree-sitter `parser.c` files (31 MB, 16 MB, 8.8 MB)
are the visible `too_large` cost.

### 2.2 Parse and import

| Stage | Result |
|---|---|
| Files offered to parsers | 742 / 742 |
| Nodes in export | 13,979 |
| Edges in export | 51,518 |
| Ledger entities | 11,043 (2,936 nodes merged by content identity) |
| Ledger relations | 46,244 |
| Relation evidence rows | 46,244 |
| Diagnostics | 0 |
| Unresolved | 0 |
| Parser errors | 3 (Kotlin) |

Entity kinds: 7,918 symbols, 1,226 types, 742 files, 634 modules, 523
externals. Resolution: 13,232 declared and 33,012 candidate relations; every
relation has `basis=syntax`.

The parse coverage by extension shows what the graph does and does not contain:

| Extension | Tracked | Parsed |
|---|---|---|
| `.rs` | 579 | 538 |
| `.py` | 112 | 49 |
| `.toml` | 53 | 49 |
| `.kt` | 41 | 37 |
| `.ts` | 36 | 32 |
| `.js` | 31 | 22 |
| `.sh` | 11 | 8 |
| `.cs` | 8 | 2 |
| `.java` | 8 | 0 |
| `.php` | 6 | 0 |
| `.go` | 4 | 0 |
| `.sql` | 18 | 0 |
| `.json` / `.md` / `.yml` | 320 / 177 / 171 | 0 |

196 tracked files under `fixtures/`, 76 under `examples/`, 18 under `benches/`
and 6 under `bin/` are excluded by the indexer's default directory list. That
is why the Python number is 49/112 and the Rust number is 538/579: product
Python (tooling) is parsed; fixture and example code is not.

## 3. Architecture the graph shows

### 3.1 Size and ownership

| Area | Entities | Notes |
|---|---|---|
| `crates/archmeld-core` | 1,768 | 1,410 symbols, 247 types, 111 files |
| `crates/codegraph-server` | 1,535 | upstream |
| `vscode` | 418 | extension source |
| `crates/codegraph` | 391 | upstream core |
| `crates/codegraph-c` | 350 | upstream parser |
| `crates/archmeld-api` | 337 | 256 symbols, 61 types, 20 files |
| `jetbrains` | 325 | plugin source |
| `tooling` | 310 | Python governance scripts |
| `crates/codegraph-memory` | 302 | upstream |
| `archmeld-vscode` | 286 | webview assets |

Private domain = `archmeld-core` + `archmeld-api` = **2,105 entities, 19.1% of
the graph**. `codegraph-*` crates are 6,317 entities (57.2%); the rest
(2,621, 23.7%) is tooling, IDE plug-ins, docs assets and webviews.

### 3.2 The one-way boundary holds, at source and import level

- `crates/archmeld-core/Cargo.toml` and `crates/archmeld-api/Cargo.toml`
  declare no `codegraph-*` dependency.
- `rg '^use codegraph'` over `crates/archmeld-core/src` and
  `crates/archmeld-api/src` returns 0.
- In the graph, imports from core/API files target only `archmeld_core`
  (354), `std` (325), `crate` (145), `serde_json` (123), `serde` (19),
  `axum` (7), `rusqlite` (4) and local test-support modules.

So the L2 → L0 rule is empirically true. But it is **not** what the default
candidate graph shows; see F3.

### 3.3 Core API surface (curated)

Incoming references from distinct files, after removing generic method names,
give a credible view of the private core's public surface:

| Target | Referencing files |
|---|---|
| `error.rs::invalid` (`CoreError::invalid`) | 42 |
| `store/mod.rs::Store` | 33 |
| `hash.rs::digest_bytes` | 26 |
| `store/validate.rs::require_id` | 24 |
| `error.rs::conflict` | 22 |
| `source.rs::line_start` | 22 |
| `hash.rs::digest_value` | 21 |
| `snapshot.rs::extension` | 21 |
| `error.rs::unsupported` | 17 |
| `store/validate.rs::require_digest` | 16 |
| `hash.rs::canonical_json` | 15 |
| `cognition.rs::Map` | 13 |

Hot files inside core: `predicate.rs` (65 symbols), `store/mod.rs` (41),
`apply.rs` (39), `changeset.rs` (39), `interpret.rs` (36), `runtime.rs` (36),
`trace_adapters.rs` (35), `units.rs` (30).

This is the expected shape for an evidence ledger: errors, hashes, validation,
snapshot and store dominate, and the domain-specific modules sit one layer out.

## 4. Product surfaces, read back

| Surface | Result |
|---|---|
| `status` (no workspace) | snapshot counts and languages, no mutation |
| `status --workspace` | `state=changed`, 6,623 added, `needs_review=false` — see F2 |
| `review-diff` | `state=changed`, 6,623 added, `needs_review=true`, 2 m 38 s, 5.4 MB JSON |
| `brief --recipe auto` | change brief: "6623 file(s) changed", 1 MEDIUM finding, 0 affected entities |
| `brief --recipe project` | 1,828 files, 11,043 entities, 46,244 relations, 7 languages; confirmed 13,232 / candidate 33,012; 2 MEDIUM findings (one false) |
| `map status` | `state=absent`, no `archmeld.map.txt` in the repo (F8) |
| MCP `tools/list` | 8 read-only tools, all implemented |
| MCP `query symbols` | works; returns anchors, backend refs, config and snapshot IDs |
| MCP `query impact` (Store) | 139 incoming relations at depth 1, all candidate/declared, with evidence IDs |
| MCP `query path` | bounded 2-hop path `api/lib.rs → contains → lock_mut → references → Store` |
| MCP `query source` | literal byte search found `scip_tool_info_missing` at `scip.rs:96` with the owning entity |
| MCP `query text` | 0 hits for `archmeld-export-v1`; text search covers names/payloads, not source bytes |
| MCP `source` (Store) | exact declaration range `store/mod.rs:55-59`, 100 bytes |
| MCP `context` (Store) | **empty pack at 4k/8k/16k tokens** — see F1 |
| MCP `inspect snapshot` | same snapshot ID, commit and created_at |

Latency on this host: `status` without a workspace is instant; any surface that
compares the workspace re-hashes 2.5 GB. Measured: `review-diff` 2 m 38 s,
MCP `status` 2 m 50 s, `status --workspace` 3 m 57 s, `brief --recipe
project` about 3.5 min, `brief --recipe auto` about 6 min because it runs the
same review twice (once to choose the recipe, once in `change_brief`).

## 5. Findings

Severity is about product behavior, not code style. Each finding has a
reproduction in the evidence directory.

### F1 — HIGH — the context pack can drop the explicit seed and return nothing

`archmeld_context` with the central `Store` type as `entity_ids`, objective
"explain how the SQLite ledger is opened, validated and used by ARCHMELD", and
budget 4,000 / 8,000 / 16,000 tokens returns:

```json
{"selected_entities": [], "source_snippets": [], "graph_slices": [],
 "budget": {"truncated": true},
 "omissions": [{"kind": "omitted", "count": 473, "reason": "final_budget_trim"}],
 "agent_summary": "context pack: 0 selected entities; 0 source snippets; truncated=true"}
```

Pack size after trimming is 2,241 bytes — far below every budget. Cause, from
`crates/archmeld-core/src/context.rs`:

1. relation omissions are appended to `pack["omissions"]` after the per-item
   `append_fits` check, and the omissions array itself is never budgeted
   (473–848 entries, each carrying a 70-character relation ID);
2. the final trim loop pops `graph_slices`, `evidence_refs`,
   `source_snippets`, `non_goals` and finally `selected_entities` until the
   JSON fits, then compacts the omissions into one aggregate entry;
3. nothing re-adds the explicitly requested entity afterwards.

Net effect: the more connected the requested entity, the more likely the pack
is empty. The flagship `archmeld_context` tool fails exactly where it matters.
Recommendation: make explicitly requested entity metadata and its first anchor
snippet a mandatory header (same rule as `objective`), cap/aggregate omission
records before counting them against the budget, and trim optional slices
before the seed. Add a regression test that a connected type yields a non-empty
pack at the default budget.

### F2 — HIGH — drift uses a weaker policy than snapshot capture

On a tree where `git status` is clean:

- `status --workspace` reports `state=changed`, 0 changed, 0 deleted,
  **6,623 added**, `needs_review=false`, and hashes **2,514 MB**.
- Of those 6,623: **6,595 are gitignored** (`target-linux` 5,698,
  `baseline/logs` 544, `docs` 220, `dist` 74, `upstream-probes` 71 …), 27 are
  tracked files that the snapshot omitted as binary/too-large, and **0 are
  genuine new untracked, non-ignored files**.
- `review-diff` on the same bytes says `needs_review=true` with
  `unmapped_files=6623`.
- `brief --recipe auto` therefore abandons the project brief and prints
  "6623 file(s) changed" with a MEDIUM finding "changed files without a base
  entity mapping"; `brief --recipe project` adds "workspace differs from the
  indexed snapshot — the working tree has uncommitted changes", which is false.
- Three surfaces give three different answers (`needs_review` false / true /
  change recipe).

Cause: `crates/archmeld-core/src/drift.rs::walk_disk` excludes only `.git`,
`.archmeld`, `node_modules`, `target`, `.venv`, `__pycache__`; it does not apply
gitignore, `binary`, `too_large` or the snapshot's other excluded directories
(so `target-linux` is walked in full). Snapshot capture applies all of them.
Recommendation: share one workspace-policy function between capture and drift;
report ignored/binary/too-large files as `omitted`, not `added`; derive
`state` and `needs_review` from eligible files only; add a test that a clean
tree with a large `target-linux/` and ignored logs is `clean`. Until then the
`brief` wording "uncommitted changes" must not be emitted for ignored files.

### F3 — MEDIUM — candidate edges are name-collision dominated

28,818 of 46,244 relations are `static_call_candidate`; 4,194 more are
`references`, all `resolution=candidate`. Read naively, they claim coupling
that cannot exist:

| Candidate edges | Count |
|---|---|
| `archmeld-core` → `codegraph-server` | 995 |
| `archmeld-core` → `codegraph` | 648 |
| `archmeld-core` → `codegraph-harness` | 280 |
| `archmeld-core` → `archmeld-vscode` | 269 |
| `archmeld-core` → `codegraph-memory` | 164 |

No such dependency exists. Of the 1,643 edges into `codegraph-server`/
`codegraph`, 1,630 target generic names: `new` (307), `len` (240), `iter`
(229), `contains` (171), `path` (129), `insert` (109), `default` (82), `read`
(72), `open` (71). `new` is defined in 135 distinct files. The raw fan-in top
list is therefore `index_state.rs::len` (1,679), `branch_watcher.rs::new`
(1,596), `harness/report.rs::is_empty` (980) and the vendored
`cytoscape.min.js::map` (912) — none is an architectural hotspot.

The contract already says these are candidates; the default presentations
(fan-in ranking, cross-area counts, impact) still invite the wrong reading.
Recommendation: add a same-file / same-crate / imported-name prior or a
`min_resolution` filter for ranking and traversal, and never compute
cross-crate coupling from candidate edges without that filter.

### F4 — MEDIUM — language provenance is lost for Kotlin, TOML and Bash

`status` reports 7 languages: rust 8,164; unknown 1,762; typescript 413;
python 382; javascript 305; csharp 11; cpp 6. But the parser did parse Kotlin
(39 files), TOML (49) and Bash (8). The 1,762 `unknown` entities decompose
exactly into:

| Unknown source | Entities |
|---|---|
| module (no anchor) | 634 |
| external (no anchor) | 523 |
| Kotlin (`kt` anchors) | 316 |
| TOML (`toml` anchors) | 273 |
| Bash (`sh` anchors) | 14 |
| Kotlin script (`kts`) | 2 |

Cause: `bridge::language_for(path)` re-derives language from a 12-extension
whitelist (py, rs, ts/tsx, js/jsx/mjs/cjs, c/h, cc/cpp/cxx/hpp/hh/hxx, java, cs,
go) instead of using the export's `language` property. The export does carry
`"language":"toml"` for Cargo.toml nodes and the Kotlin parser produces nodes
without a language property; both end up `unknown`. Result: the graph contains
316 Kotlin entities but `status`/`brief` say the project has no Kotlin, and
Cargo.toml config keys are counted as code symbols.

Recommendation: prefer the export language, fall back to a complete extension
map (kt/kts, toml, sh, rb, php, swift, scala…), and classify config files with a
`config` kind/role rather than `type`/`symbol`.

### F5 — MEDIUM — vendored and config nodes pollute ranking

- `archmeld-vscode/media/cytoscape.min.js` (373,304 B, `role=source`) produces
  104 entities (103 symbols + 1 file), **1,133 incoming** and 219 outgoing
  relations; `map` is the fourth-highest fan-in in the graph (912).
- 273 TOML entities come from manifest files: 222 `type` nodes named
  `dependencies`, `package`, `dev-dependencies`, `lib`, `config`, …, 47
  `Cargo.toml` file nodes, and 4 nodes from `pyproject.toml` /
  `rust-toolchain.toml`. `package` appears in 46 distinct files,
  `dependencies` in 45.
- The vendor registry already lists Cytoscape as `vendored_asset`; the indexer
  does not use that information.

Recommendation: exclude `**/*.min.js`, vendored `media/` assets and lock/config
files from symbol ranking (or tag them `vendored`/`config` and filter by
default), and add a self-index regression check on the top fan-in list.

### F6 — MEDIUM — parse failures are counted but not attributable

The helper reports `parser_errors: {kotlin: 3}`; the ledger records
`diagnostics: 0`. Three JetBrains sources have no file entity:

```text
jetbrains/src/main/kotlin/ai/codegraph/jetbrains/server/CodeGraphServerResolver.kt
jetbrains/src/main/kotlin/ai/codegraph/jetbrains/server/EngineLifecycle.kt
jetbrains/src/test/kotlin/ai/codegraph/jetbrains/mcp/McpRegistrationTest.kt
```

A fourth missing Kotlin file is the excluded `tests/fixtures/sample_app.kt`.
The ledger cannot name the three failures. This violates "partial coverage must
be explicit" at the file level: the counts are honest, the paths are not
recoverable. Recommendation: emit a diagnostic per failed file into the
export/bridge (or at least list paths in the helper summary).

### F7 — LOW/MEDIUM — every brief/status pays a full workspace hash

`status --workspace`, `review-diff`, both `brief` recipes and the MCP
`status`/`brief`/`review` tools all call the same drift walk. On this tree that
is 2.5 GB per scan: 3 m 57 s for `status --workspace`, 2 m 50 s for MCP `status`, 2 m 38 s for `review-diff`, and
about 6 min for `brief --recipe auto`, which calls the review twice (recipe
choice and `change_brief`). `status` without a workspace is instant. There is
no `--no-freshness` flag and no mtime/size cache. Recommendation: add a cheap
mode, cache per-path (size, mtime, digest), reuse the auto-recipe review, and
document the cost in the MCP tool description.

### F8 — LOW — the repo does not dogfood the cognition map

`archmeld map status` returns `state=absent`; there is no `archmeld.map.txt` in
the repository. The E1/E2 map, `map_entry` inspect and `brief(recipe=map)`
surfaces are therefore unexercised by the project's own tree. Recommendation:
commit a small, reviewed map for `archmeld-core` and run `map check` in CI; or
explicitly record that the map remains opt-in.

### F9 — INFO — coverage is honest but the two scopes are easy to conflate

`status` says snapshot coverage `partial` (1,828/8,458) while the index summary
says bridge coverage `complete` (13,979/13,979). Both are correct: one is the
workspace manifest, the other is the declared export. The brief prints
"1828 files … 7 language(s)" without the "742 parsed" step, and counts
`unknown` as a language. Recommendation: show all three numbers
(captured / parsed / walked) and separate structural `unknown` from real code.

### F10 — MEDIUM — the product cannot name the build that produced a ledger

`archmeld --version` prints only `0.1.0`, and MCP `serverInfo.version` is the
same crate version. Neither exposes the build commit. During this analysis the
debug binaries were built on 2026-11-25 13:56, before commits that changed
`brief.rs`, `bridge.rs` and the core LSP/trace code; HEAD `7b1dc280` is dated
2026-11-26 08:40. The stale build was detected only from file mtimes, and the
first run had to be discarded and rebuilt. The export manifest carries
`producer_commit`/`producer_dirty`, but `status` and `brief` do not surface
them. Recommendation: embed `GIT_HASH_SHORT` in `--version` and MCP
`serverInfo`, and show the producer commit/dirty in the snapshot summary so a
stale tool is visible in the evidence itself.

## 6. What is solid

- The snapshot/ledger split is real: capture hashes bytes before parsing; the
  graph is bound to `snapshot_id`, `config_digest` and per-file `blob_digest`.
- Coverage and resolution are explicit; nothing in this run was silently
  dropped. Binary/too-large/gitignore omissions are enumerated.
- Candidate vs declared is modeled and displayed (`brief` confirmed/candidate).
- The one-way dependency boundary holds at source, Cargo and import level.
- Anchors resolve to exact source ranges (`Store` 55-59; literal search line
  96) and carry evidence IDs.
- The read-only MCP surface is consistent: 8 tools, no writes, no arbitrary
  paths, untrusted content policy on every payload.
- Core structure is credible: errors, hashing, validation, snapshot and store
  are the most referenced modules, which matches the documented architecture.

## 7. Limitations of this analysis

- Static only: no build, test, runtime trace, SCIP, LSP or scenario was run
  against this snapshot.
- 1,828 of 8,458 walked files were captured; 742 files were offered to parsers.
  `examples/` and `fixtures/` are excluded by default, including the bridge
  example `archmeld_export.rs` and the two `archmeld-core/examples` programs.
- The graph has no history: one snapshot, no comparison with earlier commits.
- Candidate edges are unverified; the curated core API list in §3.3 is a
  filtered view, not a resolved call graph.
- The Windows GNU host and the debug build may differ from the release package
  in performance.
- No `archmeld.map.txt`, no imported contracts/claims/architectures, so the
  non-graph evidence surfaces are only exercised in their empty state.
- Numbers are for the indexed bytes at `7b1dc280`; rerunning after any commit
  changes them.

## 8. Recommended order of work

1. **F1** context pack must keep the explicit seed and budget its omissions.
   This is the difference between an empty and a useful pack for a central
   entity.
2. **F2** one workspace policy for capture and drift; a clean tree must be
   clean. Fix the `brief` wording in the same change.
3. **F4** language provenance from the export + complete extension map; this
   is a small, testable data fix that also removes 605 false `unknown` entities.
4. **F3/F5** candidate ranking and vendor/config filtering; make the default
   views honest without weakening the raw graph.
5. **F6/F7/F9** per-file parse diagnostics, a cheap freshness mode, and a
   three-number coverage display.
6. **F8/F10** decide whether the map is dogfooded or explicitly opt-in, and surface the producer build in `--version`, MCP `serverInfo` and `status`.

Evidence: `docs/evidence/self-analysis-2026-11-26/` (raw outputs, command
manifest, `SHA256SUMS`). The problem deep-dive, including P01–P25 and the
shared root causes, is in
[`ARCHMELD-SELF-ANALYSIS-PROBLEMS-2026-11-26.md`](ARCHMELD-SELF-ANALYSIS-PROBLEMS-2026-11-26.md).
