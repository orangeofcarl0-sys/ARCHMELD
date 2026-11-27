# Git hooks

Enable the tracked hooks once per clone:

```sh
py -3.11 tooling/publication_guard.py --install
```

`pre-push` calls `tooling/publication_guard.py --hook`. For a public remote it
allows only:

- `refs/heads/main`
- `refs/heads/public-snapshot-*`

Deleting a non-`main` branch is allowed so a mistaken push can be cleaned up.
A private remote is not restricted.

Override only for an explicit cleanup of a mistakenly pushed branch:

```sh
ARCHMELD_ALLOW_PUBLIC_BRANCH_PUSH=1 git push <remote> --delete <branch>
```

The same guard runs on pushes in the public repository through
`.github/workflows/archmeld-public-branch-guard.yml`. A sanitized snapshot is
still the only supported way to publish private work:

```sh
py -3.11 tooling/public_snapshot.py export --out <dir> --ref HEAD
py -3.11 tooling/public_snapshot.py verify-export <dir>
```
