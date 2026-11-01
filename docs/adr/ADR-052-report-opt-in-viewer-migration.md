# ADR-052: Opt-in source/trace export, static interactive viewer and ledger migration

Status: accepted for the project testing phase.

## Context

B027's first slice produced a self-contained offline report, but source bytes
and raw traces were refused, the HTML was read-only, and only same-ledger
Expedition → Manager promotion existed. B027 still asked for an explicit
opt-in export with risk labels, an interactive offline viewer without scripts
or CDNs, and a cross-ledger migration protocol with integrity checks.

## Decision

### Opt-in source and trace bytes

1. `include_source` and `include_raw_traces` are accepted instead of returning
   `unsupported`. Source comes only from the snapshot's CAS blobs, in sorted
   snapshot-path order, under `source/<path>`. Raw traces come from
   `artifact_refs(owner_type='runtime_native')` and
   `runtime_runs.artifact_digest`, deduplicated by digest, under
   `traces/sha256/<hex><extension>`.
2. Independent budgets bound the copy: 64 MiB source and 256 MiB traces. When
   a budget is reached the remaining files are counted in an explicit omission
   and the state becomes `partial`; it never silently truncates.
3. The manifest and `report.json` record per-file `kind`, `source_path` or
   `owners`, the source/trace states, file counts and bytes, and
   `risk_level=high` with risk labels. `secrets_included` is
   `unknown: no secret scanner ran`; the exporter does not claim to have
   scanned for secrets.
4. The HTML shows a sensitive-bundle banner and lists source/trace files by
   path and digest. It never embeds the bytes, so the HTML itself stays
   reviewable while the bundle as a whole is sensitive.
5. Every included file is covered by `manifest.json` and re-hashed by
   `verify_bundle`, including nested `source/` and `traces/` paths.

### Static interactive viewer

6. The HTML contains an inline SVG graph (no `xmlns` URL, no scripts, no CDN)
   for up to 240 entities and 480 relations. Nodes are anchor links to entity
   detail cards; CSS `:target` highlights the selected card. `<details>`
   provides the entity list. This is real interaction through native HTML/CSS,
   not a claim of a JavaScript application.
7. The viewer is bounded and says how many entities/relations it shows versus
   the full graph in `model.json`.

### Cross-ledger migration

8. `archmeld ledger-migrate --from <state> --to <state> --reason <text>`
   copies one complete ledger into a destination that must not exist. Merging
   into an existing ledger is refused, not approximated.
9. The source database is opened read-only; `VACUUM INTO` produces the target
   database. Source and target must each pass `PRAGMA integrity_check`, the
   target must have zero `foreign_key_check` rows, and schema versions and all
   table row counts must match.
10. Every CAS object is hashed against its path on read and again after copy;
    every `artifacts.digest` must resolve in the copied store. A tampered or
    missing object fails the migration.
11. A `migration.json` receipt records the source/target roots, database
    hashes, schema version, project ids, CAS counts and every verification
    result. Exports, verification/scenario sandboxes and other transient state
    are not copied. A failed migration removes its partial target.
12. Source and target may not overlap; an empty reason is rejected.

## Evidence

- `crates/archmeld-core/tests/offline_report.rs`
  (`opted_in_source_trace_export_is_labelled_and_verified`): real CAS source
  bytes, a real imported normalized trace plus a native trace blob, risk
  labels, viewer anchors, manifest verification and tamper detection.
- `crates/archmeld-core/tests/ledger_migration.rs`: full copy opens as a normal
  ledger, existing/overlapping targets are refused, a tampered CAS object fails
  and leaves no destination.
- `crates/archmeld-api/tests/report_http.rs`: server-owned output with opt-in
  source, sensitive risk labels and no scripts/remote URLs.
- `crates/archmeld-api/tests/trace_import_cli.rs`: the `ledger-migrate` CLI and
  an existing-target refusal.
- `docs/release-evidence/B027-opt-in-export.json` and
  `docs/release-evidence/B027-ledger-migration.json`.

## Consequences

- A default report still contains no source or trace bytes. Opting in is a
  deliberate, recorded act and the bundle must be treated as sensitive.
- The exporter does not redact secrets or personal data; it labels the risk
  instead.
- The viewer is static. It has no search, pan/zoom library or force layout;
  interaction is limited to anchor navigation, `:target` highlighting and
  native disclosure elements.
- Migration is whole-ledger copy, not merge or partial project transfer.
  Transient exports and sandboxes stay behind by design.
