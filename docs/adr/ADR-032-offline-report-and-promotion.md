# ADR-032 · Offline review bundles and validated mode promotion

Status: accepted for the R1 report/promotion slice (B027 / AT059–AT060).

## Decision

- A report names exactly one snapshot. The exporter records the project id, mode,
  snapshot/config/environment/exclusions digests, snapshot manifest artifact,
  coverage, counts, graph subset, claims, architectures, contracts, plans,
  ChangeSets, verifications/check states, evidence index, explicit unknowns,
  omissions and limitations.
- Source bytes and raw trace payloads are excluded by default. In R1 a request
  with `include_source=true` or `include_raw_traces=true` returns
  `unsupported`; it does not silently include them. Secrets are never emitted.
- The bundle contains four files: `report.html`, `report.json`, `model.json`
  and `evidence/index.json`. `manifest.json` lists each file's SHA-256, size and
  media type, plus `bundle_digest` over the manifest (excluding itself). The
  exporter re-reads every file and verifies the hash before publishing; a
  tampered attachment is detected by `verify_bundle`.
- `report.html` is static: no `<script>`, no external URL, no CDN and no project
  code execution. Tests open it in a headless browser with every hostname
  mapped to `~NOTFOUND`; the DOM must still contain the report title.
- The graph is bounded (`max_entities`, `max_relations`). Omitted entities or
  relations are listed with counts and reasons; a bounded report never claims
  complete coverage. Candidate relations remain labeled as candidates.
- The output directory is created through a sibling temporary directory and
  renamed only after all hashes pass. An existing destination is rejected, not
  overwritten. The HTTP route only accepts a safe `output_name` and always
  places the bundle under the server-owned `state/exports/` directory.
- Expedition → Manager promotion first validates SQLite integrity and every
  project-referenced CAS object. Any missing object, digest mismatch or database
  problem blocks promotion with the affected digests. The transaction then only
  marks the source session `promoted`, changes the project mode and creates the
  Manager session. It does not rewrite entities, relations, claims,
  architectures, contracts, approved plans or ChangeSets, and it makes no model
  calls.
- Object counts are compared before and after inside the transaction. A mismatch
  rolls back. On failure the original session and records remain readable; no
  source session is deleted.
- The promotion result records `object_validation`, `counts_before/after`,
  `records_rewritten: 0`, `model_calls: 0` and the source/manager session ids.

## Residual limits

- Source and raw-trace inclusion is opt-in with explicit risk labels and
  independent byte budgets; the exporter does not redact secrets (ADR-052).
- The offline viewer is an inline-SVG anchor/CSS `:target` view, not a
  JavaScript graph editor (ADR-052).
- Promotion assumes the same local ledger and CAS. Cross-ledger movement is a
  separate whole-ledger copy protocol with integrity checks and a receipt, not
  a merge (ADR-052).
