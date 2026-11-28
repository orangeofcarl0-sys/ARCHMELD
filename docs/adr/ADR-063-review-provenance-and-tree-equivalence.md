# ADR-063: Review provenance and tree equivalence

Status: accepted for implementation (2026-11-28).

## Context

Palimpsest recorded the candidate Git SHA outside the review result because
`review-diff` returned only a base snapshot id. The snapshot already records
`git_commit`, `git_tree_digest`, `config_digest`, `environment_digest`,
`exclusions_digest` and `workspace_policy`; `index_runs` records the analyser
build and index scope. The missing piece was a stable output contract.

`git_tree_digest` is not a Git tree OID: it mixes Git blob ids for clean tracked
files with `working:<blob_digest>` for dirty or untracked files. It cannot be
used alone to prove byte equivalence.

## Decision

1. `review-diff` gains a top-level `provenance` block:
   - `base`: snapshot id, Git commit, head tree oid (when resolvable), stored
     Git tree digest, a pure `content_tree_digest` over `(path, blob_digest)`,
     config/environment/exclusion digests, workspace policy and the latest
     index-run analyser identity;
   - `candidate`: Git commit, HEAD tree oid, dirty flag/count, pure content tree
     digest and `durable=false`.
2. `content_tree_digest` is `sha256` over canonical `(path, blob_digest)`
   entries. It is the primary byte-equivalence signal. `head_tree_oid` is
   labelled as the commit's tracked tree, never as the snapshot's bytes.
3. `diff_snapshots` gains the same provenance block on both the full and
   `state=unchanged` path, plus `identity_changes` for config, environment,
   exclusions, workspace policy, Git tree digest, content tree digest and
   analyser. `tree_equivalent` is true when the content tree digests match.
4. A relation-only change now makes `diff_snapshots.state=changed`; previously
   the state decision ignored the relation lists.
5. File entries carry `base_side=base` and `current_side=candidate`; entities
   cite `side=base`; line changes cite `side=both`.
6. Candidate bytes are not frozen by default. `review-diff --freeze` is an
   explicit CLI opt-in that writes changed/added bytes to the CAS and pins the
   objects; it does not create a snapshot or change `latest_snapshot_id`.
   See ADR-067.

## Consequences

- A review result is traceable to the exact base snapshot and the observed
  candidate Git state without a host-side side channel.
- A dirty candidate is explicitly non-durable; base citations remain
  replayable from the ledger. This limitation is visible rather than implied.
- `git_tree_digest` remains recorded for compatibility but is not used as the
  sole equivalence proof.
- `head_tree_oid` is null for non-Git workspaces, missing commits, or a Git
  command failure; that is reported as unknown, not clean.

## Evidence

- `crates/archmeld-core/tests/diff_review.rs`:
  `review_and_diff_expose_git_provenance_and_tree_equivalence`.
- Existing drift and capture tests remain unchanged.
