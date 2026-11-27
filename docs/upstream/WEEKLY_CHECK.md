# Weekly upstream check and evidence template

Status: U4b governance, 2026-11-26.

The weekly check is read-only. It never fetches into the working tree, merges,
pushes, opens a pull request or changes a pin. A newer upstream commit is a
finding, not a repository defect.

## Commands

```text
python tooling/upstream.py verify --skip-cargo --json --out upstream-verify.json
python tooling/upstream.py status --remote --json --out upstream-status.json --fail-on-update
python tooling/upstream.py check codegraph --json --out upstream-check.json --fail-on-update
python tooling/upstream.py probes --json --out upstream-probes.json
```

`status --remote` tries the configured remote name first and falls back to the
registry URL with `HEAD`. This makes the check work in a fresh CI checkout where
only `origin` exists. `check <id>` uses the same fallback and only applies to
commit pins.

Exit codes:

| Code | Meaning | Weekly handling |
|---|---|---|
| 0 | no newer commit detected | pass |
| 3 | `--fail-on-update` and a newer commit exists | record in the job summary; not a job failure |
| other | registry, network or tooling failure | fail the job |

CodeBoarding may report `update_available=true` for a long time because its pin
is a specific commit, not the default branch. That is expected; it does not
schedule a merge by itself.

## Evidence template

Copy the fields below into `docs/release-evidence/UPSTREAM-WEEKLY-YYYY-MM-DD.json`
after a real run. Do not add an `AT077` block: `handoff_check.py` and
`test_release.py` select the newest `UPSTREAM-*.json` containing `AT077`, and a
weekly file must not override real merge evidence.

```json
{
  "schema_version": "1.0",
  "recorded_at": "YYYY-MM-DD",
  "generated_by": "tooling/upstream.py status + verify + check + probes",
  "head": "<40-char commit>",
  "registry_sha256": "<sha256 of upstreams.lock.json>",
  "tools": {
    "git": "<first line of git --version>",
    "python": "<version>"
  },
  "verify": {
    "status": "passed | passed_with_warnings | failed",
    "checks_failed": [],
    "checks_warned": []
  },
  "status": {
    "artifact": "upstream-status.json",
    "sha256": "<sha256>",
    "updates": [
      {"id": "codegraph", "pin": "<commit>", "latest": "<commit>", "update_available": false}
    ]
  },
  "check": {
    "artifact": "upstream-check.json",
    "sha256": "<sha256>",
    "exit_code": 0
  },
  "probes": {
    "artifact": "upstream-probes.json",
    "sha256": "<sha256>",
    "updates": [
      {"id": "tracy", "pin": "0.13.1", "latest": "0.15.0", "update_available": true},
      {"id": "clangd", "status": "skipped", "detail": "local lock is authoritative"}
    ]
  },
  "decision": "no action | schedule drill | investigate registry failure",
  "limitations": [
    "version probes report latest/update_available but not a commit behind count; behind is null",
    "version updates are informational and do not fail the weekly job",
    "git ls-remote reads tags or the registry ref; it does not fetch objects"
  ]
}
```

Also record the commands through `python baseline/run_cmd.py` and keep the raw
JSON outputs under `docs/evidence/upstream-weekly-<date>/` with a `SHA256SUMS`
file.

## CI

`.github/workflows/archmeld-upstream.yml` runs the same commands weekly and on
`workflow_dispatch`, with `contents: read`, and uploads the three JSON files as
an artifact. It runs on the public snapshot's default branch; the private
development tree uses the same commands locally.
