# Reviewer pilot protocol (agent adaptation)

Status: **run as an agent pilot**. This is not a human reviewer study and does
not support a claim that ARCHMELD improves human review outcomes.

## Adaptation

The original protocol asked for two independent human reviewers and a second
human scorer. No humans were provided for this run. To obtain a reproducible
pilot without fabricating human data, the experiment was executed with two
independent durable `pi` sessions, each in its own workspace/state directory
and with no shared conversation:

| Reviewer | Model | Provider route |
|---|---|---|
| `reviewer-1` | `deepseek/deepseek-v4-pro` | `deepseek` |
| `reviewer-2` | `omni-responses/deepseek-v4.1-flash` | `omni-responses` |

Independence is session-level and provider-route-level; both models are
DeepSeek-family variants, so this is not model-family independence. Records are
labelled `agent reviewer` everywhere.

## Design

- 3 fixtures: file move, changed owner with an untouched caller, and
  unmapped-gap with writes unavailable.
- 2 arms: (A) git/file reads only; (B) the same plus read-only ARCHMELD MCP
  tools (`archmeld_brief`, `archmeld_review`, `archmeld_query`,
  `archmeld_source`).
- 3 x 2 x 2 = 12 sessions; arm order randomized per fixture/reviewer with seed
  `20261128`; both reviewers see the same task text and a 300 s limit.
- The key and fixture tasks were written before any session (`key.json`).
- The runner is `tooling/reviewer_pilot.py`; a run records the prompt, answer,
  MCP calls, shell commands, edits (must be empty), durable session id, token
  usage and a SHA-256 of each session JSONL. Session JSONL files stay outside
  the git worktree under `_archmeld_reviewer_pilot_2026-11-28/`; their hashes
  are stored in `runs/*/run.json`.

## Measures and scoring

`matched` requires: the key entity, the key paths, the expected labels
(candidate vs resolved, writes available/unavailable, coverage), at least one
citation, and every cited path existing in the reviewed repo. `unsupported`
is set when an answer claims a resolved relation for a candidate relation or
claims present writes where the key says unavailable. Scoring is deterministic
against the key and does not read the arm.

## Results

See `RESULT.md` and `STATUS.json`. Headline: arm B matched 6/6, arm A 3/6,
with 2 unsupported "resolved" claims in arm A. The sample is small and
descriptive only; no statistical significance is claimed.

## Command

```sh
py -3.11 tooling/reviewer_pilot.py --prepare
py -3.11 tooling/reviewer_pilot.py --run-all
py -3.11 tooling/reviewer_pilot.py --score
```

Recorded commands: `reviewer-pilot-2026-11-28-c` (sessions),
`reviewer-pilot-2026-11-28-score` (aggregate).
