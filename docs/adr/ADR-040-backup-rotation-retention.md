# ADR-040: encrypted backup key rotation and path retention

Status: accepted for the project testing phase.

## Context

Local encrypted backups were verified but had no key provenance, no way to
change the passphrase without re-creating a backup from the ledger, and no
off-host retention operation. B025 tracked "remote retention/key replication and
key rotation". S14 requires consistent backup snapshots and a recovery path; it
does not mandate a particular cloud API.

## Decision

1. Every exported manifest carries `key_provenance`:
   `{key_id, generation, rotated_from?, rotated_at?}`.
   - `key_id` is an HMAC-SHA256 fingerprint of the MAC key, truncated to 32 hex
     characters. It identifies a key generation without revealing the key or the
     passphrase.
   - Backups created before this field existed are treated as generation 1 with
     a key id derived at verification time.
2. `rotate_encrypted_backup(source, old_pass, new_pass, destination)`:
   - fully verifies and decrypts the source under the old passphrase;
   - refuses a non-empty destination or a destination inside the source;
   - re-encrypts every file with a fresh salt and per-file nonce under the new
     passphrase;
   - increments `generation` and sets `rotated_from` to the previous key id;
   - verifies the rotated bundle with the new passphrase before returning.
   The source bundle is left in place; deleting it is an explicit operator act.
3. `retain_encrypted_backup(source, target_root, retention_id, passphrase)`:
   - verifies the source before copying;
   - copies `manifest.json` and exactly the manifest-listed encrypted files to
     `<target_root>/<retention_id>/` (a path on any mounted filesystem,
     including a network mount);
   - verifies the copy independently;
   - writes a plain `retention.json` sidecar with the manifest digest, key
     provenance and verification result.
   No cloud API, credential or network transport is implemented.
4. CLI: `backup-rotate` and `retain-backup`. Passphrases are read only from
   environment variables (`ARCHMELD_BACKUP_PASSPHRASE`,
   `ARCHMELD_BACKUP_NEW_PASSPHRASE`), never argv.

## Consequences

- A passphrase can be rotated without rebuilding the ledger, and the new
  manifest records which key generation it belongs to.
- A retained copy is verified on the destination before the record is written,
  so a successful retention result is not just a copy claim.
- The retention sidecar is not authenticated and the old key is not escrowed;
  the bundle itself must always be verified before restore.
- Credentialed remote transport and automatic propagation of key rotations to
  already-retained copies remain outside this decision.
