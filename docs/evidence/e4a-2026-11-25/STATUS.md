# E4a status (2026-11-25): no-go

The pi-backed E4a experiment ran on a cleaned non-lexical fixture and failed
the pre-registered gate. E stays at the read-only E2 surface; `map apply`
(E3), E4b and rule v3 are not promoted.

- Backend: `pi` 1.0.4, `omni-anthropic/deepseek-v4.1-flash`, thinking max,
  durable sessions; `.pi/mcp.json` direct MCP exposure.
- Clean fixture: 26 files (7 core + 18 service decoys + README), 7 map
  entries, no coupling comments.
- A (no map): 2/3 correct; mean 152,524 tokens.
- B (map): 1/1 correct; 74,792 tokens; 3 Entry reads.
- Gate required A <=1/3; A=2/3 failed it, so B2/B3 were not run.

See `RESULT.md` for the table and design iterations. The first two drafts
leaked the coupling in README/comments and are invalid, not no-go evidence.
Evidence: `docs/release-evidence/E4A-NOGO-2026-11-25.json`.
