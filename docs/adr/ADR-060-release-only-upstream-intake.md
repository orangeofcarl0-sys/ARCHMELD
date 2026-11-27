# ADR-060: Release-only CodeGraph intake

Status: accepted (2026-11-27).

## Context

ARCHMELD is a thin fork of CodeGraph. Tracking `upstream/main` directly makes
the fork absorb every intermediate commit, increases the diff surface and turns
routine upstream movement into merge-conflict work. Upstream releases are the
stable boundary: a tag such as `v0.21.0`, or a version-bump commit with release
notes when the project does not tag a release.

The current pin is `ae4b2e54` (0.21.2). Upstream's latest git tag is `v0.21.0`;
0.21.2 is an untagged release commit. The registry previously recorded
`refs/heads/main` as the source ref, so `tooling/upstream.py check` compared the
pin with the moving branch head.

## Decision

1. CodeGraph intake is release-only. Accepted sources are `refs/tags/v*`, or a
   version-bump release commit whose release notes and commit are recorded in
   `upstreams.lock.json`.
2. Branch heads (`refs/heads/main`, `main`, `master`, `upstream/main`) are not
   update sources and must not be merged into `archmeld/initial`.
3. `upstreams.lock.json` records `update_policy.release_only=true`,
   `accepted_refs`, `forbidden_refs`, and a `release` object. An untagged
   release is an explicit exception with `tag: null`, the release commit, the
   release URL and the reason.
4. `tooling/upstream.py check` compares a release tag when one exists; for an
   untagged release it reports the pinned commit and `release_only=true`
   without querying a branch. `diff` and `drill` refuse forbidden branch refs.
5. 0.21.2 is recorded as the current untagged release exception. The next
   upgrade must use a tagged release if one exists, or record a new explicit
   exception with release notes.

## Consequences

- No automatic following of upstream `main`; `auto_follow`, `auto_merge` and
  `auto_pin_update` remain false.
- A release without a tag is still auditable: version, commit, release URL and
  reason are in the registry, and the pin remains a full 40-hex commit.
- `upstream.py check` no longer produces false "update available" results from
  branch movement. Diagnostic diffs against a branch require an explicit
  override outside the release-only path.
- Upgrades remain manual: release notes, patch accounting, drill, evidence and
  acceptance before the pin changes.
