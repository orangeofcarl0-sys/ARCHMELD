# ADR-038 · Passphrase-encrypted local backups

Status: accepted (2026-10-31)

## Context

`create_backup` writes a consistent SQLite copy and every CAS object as plaintext
files. S14 requires backup encryption, and the passphrase must not leak into
logs, argv or the backup manifest.

## Decision

Add `encrypted_backup` with three operations:

- `create_encrypted_backup(store, project, destination, passphrase)`: create the
  normal plaintext backup in a temporary directory, encrypt every file with
  AES-256-GCM, then remove the plaintext directory.
- `verify_encrypted_backup(destination, passphrase)`: authenticate the manifest
  with HMAC-SHA256, decrypt every file, and check both ciphertext and plaintext
  digests and sizes.
- `restore_encrypted_backup(destination, passphrase, target)`: verify first,
  then decrypt into a new empty directory that can be opened as a Store.

Cryptography:

- KDF: PBKDF2-HMAC-SHA256, 120,000 iterations, 16-byte random salt.
- 64 derived bytes: first 32 for AES-256-GCM, last 32 for manifest HMAC.
- Every file has a random 12-byte nonce and uses its bundle-relative path as
  AAD.
- The manifest stores only ciphertext/plaintext digests, sizes, nonces and KDF
  parameters; it is authenticated with HMAC-SHA256 over its AM-C14N-1 canonical
  form without the `manifest_hmac` field.
- The passphrase is supplied through an environment variable by the CLI
  (`ARCHMELD_BACKUP_PASSPHRASE` by default), never argv.

The backup is recorded in `backup_runs` with the plaintext database digest and
the encrypted manifest digest. `object_count` is the number of encrypted files.

## Consequences

- A wrong passphrase, a modified ciphertext, a modified file set or a modified
  manifest fails verification before any restore write.
- Restore refuses a non-empty target and never overwrites existing state.
- The encrypted bundle still contains source objects (readable only with the
  passphrase); it is not a redacted export.
- Key rotation and remote retention are future work; this ADR covers local
  encrypted backups only.
