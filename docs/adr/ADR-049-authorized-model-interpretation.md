# ADR-049: Authorized model interpretation for CodeBoarding analysis

Status: accepted for the project testing phase.

## Context

B017 imported a real CodeBoarding `analysis.json` but had never generated one
with a model, because no provider was authorized. The operator has now selected
`opencode/deepseek-v4.1-flash` and authorized reading its key from the ZCode
provider configuration. CodeBoarding supports an OpenAI-compatible base URL but
cannot set the provider's required `x-opencode-session` header.

## Decision

1. The key is read at run time from the ZCode provider config by a loopback
   proxy (`tooling/b017_model_proxy.py`). It is never placed in argv, logs,
   evidence or the CodeBoarding process environment; CodeBoarding receives a
   dummy local key.
2. The proxy forwards `/v1/chat/completions` to the configured provider, adds
   the configured session header and the real bearer key, and logs only method,
   path, status, duration and byte count.
3. The analysis runs CodeBoarding `full --local` on a pinned local copy of the
   CodeBoarding source (commit `a38a8d15…`), depth cap 2, with telemetry
   disabled. The target copy is prepared without `.git`, `.venv` or a previous
   `.codeboarding` output.
4. The import records model provenance in the architecture producer
   (`provider`, `model`, host, calls, input/output tokens). Unknown model fields
   are rejected; an `api_key` field is never accepted. The request digest
   includes the provenance.
5. Imported components, dependencies and references remain candidate-only:
   zero ledger entities or relations are added, and provider commands are
   recorded but never executed.
6. The provider remains `unconfigured` by default in the product; this is an
   explicit, operator-authorized evidence run, not a runtime dependency.

## Evidence

- Real run: 11 model calls, 88,612 input tokens and 22,718 output tokens,
  2,014,697-byte `analysis.json` with 6 root components and 101 relations.
- Import: 29 components, 132 dependencies, 136 source references all resolved,
  `candidate_only=true`, `certain_calls_added=0`, `commands_executed=0`,
  entity/relation additions 0. The stored producer records
  `opencode go orange` / `deepseek-v4.1-flash` / `opencode.ai`.
- `docs/release-evidence/B017-model-interpretation.json`, provenance
  `docs/evidence/B017-model-provenance.json`, analysis
  `docs/evidence/B017-model-analysis.json`, logs
  `baseline/logs/b017-model-run-cb.log` and `b017-model-proxy-cb.log`.
- Tests: `crates/archmeld-core/tests/b017_model_analysis.rs` (gated on the
  evidence paths), `crates/archmeld-api/tests/interpretation_http.rs` (model
  provenance over HTTP and unknown-field rejection).

## Consequences

- B017's model interpretation is now backed by a real, authorized provider run
  and a real `analysis.json`.
- Model output is run-specific; the committed analysis is an observation of
  this run, not a deterministic fixture.
- The proxy is evidence plumbing. It is not part of the product runtime and the
  key remains operator-managed.
