# ADR-067: Review candidate byte freeze

Status: accepted for implementation (2026-11-28).

## Context

`review-diff` read the workspace and cited `current_digest` values, but the
candidate bytes were not stored. After a workspace edit, a candidate citation
could not be replayed. A full candidate snapshot would advance
`latest_snapshot_id` and require a new index, which is a much larger change.

## Decision

1. `review-diff --freeze` (workspace mode, requires `--persist`) copies the
   changed/added candidate files into the CAS and pins each object. It does not
   create a snapshot row and does not change `latest_snapshot_id`.
2. The result records `provenance.candidate.durable=true` and a `frozen` block
   with path, blob digest, size and state. Unreadable or oversized files are
   listed as limitations; they are not silently dropped.
3. The freeze is bounded (500 files / 64 MiB); the cap is reported.
4. `review-diff --report <id>` reads the persisted report; the frozen blobs are
   protected by object pins so retention/GC can reclaim other artifacts without
   losing the review evidence.
5. Snapshot-to-snapshot review already has immutable candidates; `--freeze` is
   rejected in that mode.
6. Freeze is CLI-only. MCP remains read-only and exposes no freeze operation.

## Consequences

- A persisted review can replay both base and changed candidate bytes without
  changing the project's current snapshot.
- The freeze covers changed/added files, not every workspace file; a citation
  to an unchanged file remains a base snapshot citation.
- Object pins are explicit and can be audited/removed like other pins.

## Evidence

- `crates/archmeld-core/tests/diff_review.rs`:
  `persisted_review_report_pins_its_base_snapshot` also freezes the candidate
  file and reads it back from the CAS.
