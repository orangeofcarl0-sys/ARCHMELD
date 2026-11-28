// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `backup` CLI commands.

use super::*;

#[derive(clap::Args)]
pub(crate) struct BackupArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long, default_value = "project.local")]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) out: PathBuf,
    /// Environment variable holding the passphrase. The passphrase is never
    /// read from argv because process lists are observable.
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    pub(crate) passphrase_env: String,
}

#[derive(clap::Args)]
pub(crate) struct VerifyBackupArgs {
    #[arg(long)]
    pub(crate) dir: PathBuf,
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    pub(crate) passphrase_env: String,
}

#[derive(clap::Args)]
pub(crate) struct RestoreBackupArgs {
    #[arg(long)]
    pub(crate) dir: PathBuf,
    #[arg(long)]
    pub(crate) target: PathBuf,
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    pub(crate) passphrase_env: String,
}

#[derive(clap::Args)]
pub(crate) struct RotateBackupArgs {
    #[arg(long)]
    pub(crate) dir: PathBuf,
    #[arg(long)]
    pub(crate) out: PathBuf,
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    pub(crate) old_passphrase_env: String,
    #[arg(long, default_value = "ARCHMELD_BACKUP_NEW_PASSPHRASE")]
    pub(crate) new_passphrase_env: String,
}

#[derive(clap::Args)]
pub(crate) struct RetainBackupArgs {
    #[arg(long)]
    pub(crate) dir: PathBuf,
    /// Directory on any mounted filesystem (including a network mount).
    #[arg(long)]
    pub(crate) target: PathBuf,
    #[arg(long)]
    pub(crate) retention_id: String,
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    pub(crate) passphrase_env: String,
}

pub(crate) fn run_backup(args: BackupArgs) -> Result<Value, String> {
    let secret = passphrase(&args.passphrase_env)?;
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let result = create_encrypted_backup(&mut store, &args.project, &args.out, &secret)
        .map_err(|error| error.to_string())?;
    Ok(json!({"command": "backup", "project_id": args.project, "result": result}))
}

pub(crate) fn run_verify_backup(args: VerifyBackupArgs) -> Result<Value, String> {
    let secret = passphrase(&args.passphrase_env)?;
    let result = verify_encrypted_backup(&args.dir, &secret).map_err(|error| error.to_string())?;
    Ok(json!({"command": "verify-backup", "dir": args.dir, "verified": result}))
}

pub(crate) fn run_restore_backup(args: RestoreBackupArgs) -> Result<Value, String> {
    let secret = passphrase(&args.passphrase_env)?;
    let result = restore_encrypted_backup(&args.dir, &secret, &args.target)
        .map_err(|error| error.to_string())?;
    Ok(json!({"command": "restore-backup", "target": args.target, "restored": result}))
}

pub(crate) fn run_rotate_backup(args: RotateBackupArgs) -> Result<Value, String> {
    let old_secret = passphrase(&args.old_passphrase_env)?;
    let new_secret = passphrase(&args.new_passphrase_env)?;
    let result = rotate_encrypted_backup(&args.dir, &old_secret, &new_secret, &args.out)
        .map_err(|error| error.to_string())?;
    Ok(json!({"command": "rotate-backup", "dir": args.dir, "out": args.out, "rotated": result}))
}

pub(crate) fn run_retain_backup(args: RetainBackupArgs) -> Result<Value, String> {
    let secret = passphrase(&args.passphrase_env)?;
    let result = retain_encrypted_backup(&args.dir, &args.target, &args.retention_id, &secret)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "retain-backup",
        "dir": args.dir,
        "target": args.target,
        "retention_id": args.retention_id,
        "retention": result,
    }))
}
