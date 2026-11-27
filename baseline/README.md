# Baseline evidence

`commands.ndjson` is append-only. Every line records one executed command:

- stable `id` and note;
- absolute `cwd`, exact `argv`, exit code, start/finish and duration;
- platform and tool first lines (`rustc -vV`, `cargo`, `git`, `node`, `npm`);
- git HEAD/branch/status before and after;
- stdout/stderr log paths and their SHA-256 hashes;
- an allowlisted `environment` block plus `environment_resolution` (toolchain,
  temp/tools roots, libclang, bindgen includes, CXXFLAGS) and an
  `environment_replayable` flag; secrets and the full environment are never
  dumped.

Run a command through the recorder:

```sh
python baseline/run_cmd.py --id <stable-id> --note "<why>" \
  --env "KEY=VALUE" -- <argv...>
```

`environment.json` captures the machine and toolchain inventory. Failed attempts
are kept on purpose: the historical Windows RocksDB crash (Strawberry/winlibs
GCC 13.2.0; UCRT64 16.1.0 passes 59/59), missing MSVC linker, missing C headers
for bindgen, the empty-disk build failure and the DLL-order failure are part of
the reproducible baseline, not noise to be deleted.

`annotations.ndjson` is an append-only sidecar for records whose recorded
environment is not replayable (for example the 2026-11-25 bindgen prose value).
It never rewrites `commands.ndjson`; read it before replaying an old id.

`upstream-probes/` contains probe outputs and real exports; `real-export/`
artifacts are generated on Linux and imported on Windows through the bridge.
