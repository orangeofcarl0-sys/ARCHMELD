# Baseline evidence

`commands.ndjson` is append-only. Every line records one executed command:

- stable `id` and note;
- absolute `cwd`, exact `argv`, exit code, start/finish and duration;
- platform and tool first lines (`rustc -vV`, `cargo`, `git`, `node`, `npm`);
- git HEAD/branch/status before and after;
- stdout/stderr log paths and their SHA-256 hashes.

Run a command through the recorder:

```sh
python baseline/run_cmd.py --id <stable-id> --note "<why>" \
  --env "KEY=VALUE" -- <argv...>
```

`environment.json` captures the machine and toolchain inventory. Failed attempts
are kept on purpose: the Windows RocksDB crash, missing MSVC linker, missing C
headers for bindgen, the empty-disk build failure and the DLL-order failure are
part of the reproducible baseline, not noise to be deleted.

`upstream-probes/` contains probe outputs and real exports; `real-export/`
artifacts are generated on Linux and imported on Windows through the bridge.
