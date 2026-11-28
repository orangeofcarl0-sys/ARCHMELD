# ADR-064: Architecture rule exceptions, rule source and revision diff

Status: accepted for implementation (2026-11-28).

## Context

Palimpsest asked projects to supply versioned layer rules and accepted
exceptions, with the introduction version, rationale and scope visible. The
implementation already had project-scoped, revisioned architecture records and
six executable rule kinds. It had no exception object, no rule-level source,
and no way to compare rule exceptions across architecture revisions.

## Decision

1. `ArchitectureRule` gains two optional fields:
   - `source: {path, line, digest?}`: workspace-relative rule provenance;
   - `exceptions: [{id, violation_key, reason, introduced_revision,
     expires_revision?, scope_ids?, source?}]`.
   Omitted fields preserve the previous canonical bytes.
2. Exceptions match a violation by the violation's `key`, not by a broad
   wildcard. For `forbid_dependency` the key is the sequence of qualified
   entity names (so a content-only re-index keeps it); for other kinds it is
   the cycle, component pair, owned item or contract endpoint. For
   `custom_contract` the key also names the assertion and `parameters.limit`,
   so an exception written for one contract cannot excuse a different limit.
   `scope_ids` are recorded for audit and do not widen or narrow matching.
3. A check result reports:
   - `violations` (active),
   - `excepted` (violation + matching exception),
   - `unused_exceptions`,
   - `inactive_exceptions` (not yet in force or expired),
   - `exception_changes` against the previous architecture revision,
   - `rule_source`.
   `exception_changes.changed` compares `id`, `introduced_revision`, `reason`,
   `scope_ids`, `expires_revision` and `source`.
4. A rule with only excepted violations has `verdict="excepted"`,
   `passed=false`, `blocks_acceptance=false`. It is never reported as
   `passed`, and required rules do not silently disappear.
5. `check_architecture_rules` aggregates `state="excepted"` when no active
   violation or blocking rule remains. Exception expiry is evaluated against
   the current architecture revision.
6. Prior revisions are loaded by revision number for the diff; the public
   latest-only `get_architecture` behaviour is unchanged.
7. Validation rejects duplicate `rule.id`, duplicate exception `id` and
   duplicate `violation_key` within one rule, so `exception_changes` cannot be
   overwritten by a later entry.

## Consequences

- Exception matching is exact and auditable. A new path is not covered by an
  old exception; it produces a new active violation. `scope_ids` are audit
  metadata, not a wildcard.
- Adding, changing or removing an exception is visible in
  `exception_changes` without reading the raw architecture history.
- `expires_revision` is the last revision on which an exception is active;
  inactive exceptions are reported in `inactive_exceptions`.
- The sealed `ArchitectureRule` schema still has `additionalProperties:false`;
  the optional fields are recorded in
  `contracts/archmeld/architecture-rule-exceptions.patch.json` until the next
  pack revision.

## Evidence

- `crates/archmeld-core/tests/blueprint_rules.rs`:
  `architecture_rule_exceptions_are_visible_and_revision_scoped`,
  `rule_exception_future_scope_changes_and_unused_entries_are_visible`,
  `duplicate_rule_ids_and_exception_keys_are_rejected`,
  `custom_contract_key_names_the_assertion_and_limit`.
