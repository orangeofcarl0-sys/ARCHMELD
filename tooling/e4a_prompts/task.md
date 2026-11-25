You are working in a disposable copy of a Python repository. Work only inside
the current workspace.

TASK
1. Change the discount rate from 10% to 15% in `src/pricing.py`.
2. Keep every part of the repository that encodes or checks the old rate
   consistent. You may edit any file in the workspace.
3. Do not run the project's tests.
4. You may read files and run read-only shell commands.

<RULES>

Return ONLY one JSON object as your final message, no markdown fence:
{"files_changed":["..."],"rate_updated":true,"constraints_applied":[{"file":"...","what":"..."}],"map_used":true,"notes":"..."}
