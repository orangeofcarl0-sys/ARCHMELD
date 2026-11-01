# ADR-041: OS-enforced memory ceiling for trace imports

Status: accepted for the project testing phase.

## Context

B026 requires an enforced memory ceiling for the million-event stress path.
Measuring peak RSS after the fact only reports a budget; it does not stop an
import from exceeding it. The trace importer is a single process, so the ceiling
must be applied in-process before the trace is read.

## Decision

1. `archmeld trace-import` gains `--memory-limit-mb <MB>` (1..=1,048,576).
   The limit is applied before the store or trace is opened, and the value is
   recorded as `memory_limit_mb` in the result.
2. Platform enforcement:
   - Windows: the current process is assigned to a job object with
     `JOB_OBJECT_LIMIT_PROCESS_MEMORY` and `JOB_OBJECT_LIMIT_JOB_MEMORY`.
   - Linux/macOS: `RLIMIT_AS` is lowered to the requested size.
   - Other platforms return an explicit unsupported error.
3. A ceiling breach fails the process; there is no in-process recovery after an
   allocation failure. The import transaction is atomic, so a failed run leaves
   no `runtime_runs` row.
4. Remote validation uses a cgroup v2 scope with `MemoryMax=4G` and
   `MemorySwapMax=0` in addition to the in-process limit, so both the hard
   address-space ceiling and the RSS-like cgroup peak are observable without
   swap masking.
5. `--format normalized` imports an explicit `archmeld-trace-v1` envelope
   directly. This is the format used by the stress runs and by offline trace
   producers; it still goes through the same validation and binding rules.

## Evidence

- Remote WSL2, 24 cores / 24 GB, kernel 6.6.87.2, systemd 255, cgroup v2.
- One-million-event normalized trace (81,728,764 bytes): import under
  `RLIMIT_AS=4096 MB` and cgroup `MemoryMax=4G` exited 0 in 5.66 s with
  `memory.peak=1,869,725,696` bytes and zero OOM events.
- The same trace with `RLIMIT_AS=1024 MB` exited 134 with
  `memory allocation ... failed`, cgroup peak 1,054,494,720 bytes, no
  `runtime_runs` row, and SQLite integrity `ok`.
- Local Windows CLI tests cover flag recording and a 1 MB ceiling failure.

## Consequences

- The stress path has a real ceiling instead of a post-hoc measurement.
- `RLIMIT_AS` is stricter than RSS (it includes virtual mappings), so a limit
  that passes on Linux is not directly comparable to a Windows working-set
  number.
- A breach is a process failure, not a structured partial result; callers must
  treat a non-zero exit as "ceiling exceeded or import failed" and re-run under
  a larger declared budget.
- A dedicated webview paint/GPU frame metric remains outside this decision.
