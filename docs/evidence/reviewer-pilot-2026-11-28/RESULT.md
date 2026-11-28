# Reviewer pilot (agent reviewers)

Status: run with two durable `pi` sessions backed by different model
families. This is **not** a human reviewer study and does not claim one.

| reviewer | model | fixture | arm | matched | owner | paths | labels | citations | unsupported | duration_ms | calls |
|---|---|---|---|---|---|---|---|---|---|---|---|
| reviewer-1 | `deepseek/deepseek-v4-pro` | context | a | False | True | True | False | True | True | 68622 | 0 |
| reviewer-1 | `deepseek/deepseek-v4-pro` | context | b | True | True | True | True | True | False | 70220 | 5 |
| reviewer-1 | `deepseek/deepseek-v4-pro` | gap | a | True | True | True | True | True | False | 88620 | 0 |
| reviewer-1 | `deepseek/deepseek-v4-pro` | gap | b | True | True | True | True | True | False | 46105 | 7 |
| reviewer-1 | `deepseek/deepseek-v4-pro` | move | a | True | True | True | True | True | False | 10877 | 0 |
| reviewer-1 | `deepseek/deepseek-v4-pro` | move | b | True | True | True | True | True | False | 21711 | 3 |
| reviewer-2 | `omni-responses/deepseek-v4.1-flash` | context | a | False | True | True | False | True | True | 25794 | 0 |
| reviewer-2 | `omni-responses/deepseek-v4.1-flash` | context | b | True | True | True | True | True | False | 31827 | 5 |
| reviewer-2 | `omni-responses/deepseek-v4.1-flash` | gap | a | False | True | True | True | False | False | 263740 | 0 |
| reviewer-2 | `omni-responses/deepseek-v4.1-flash` | gap | b | True | True | True | True | True | False | 38128 | 8 |
| reviewer-2 | `omni-responses/deepseek-v4.1-flash` | move | a | True | True | True | True | True | False | 11858 | 0 |
| reviewer-2 | `omni-responses/deepseek-v4.1-flash` | move | b | True | True | True | True | True | False | 25608 | 5 |

## Limitations

- Agent reviewers, not humans: no human time-to-owner or human judgment.
- Small sample (3 fixtures x 2 arms x 2 reviewers); descriptive only, no
  statistical significance.
- Scoring is deterministic against the pre-registered key and is blind to
  the arm; there is no second human scorer.
- A negative or mixed result is reported as-is.
