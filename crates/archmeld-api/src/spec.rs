// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Clap surface: the public command names and their argument structs.

use super::commands::*;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "archmeld",
    version = env!("CARGO_PKG_VERSION"),
    long_version = concat!(
        env!("CARGO_PKG_VERSION"),
        "+",
        env!("ARCHMELD_GIT_SHORT")
    ),
    about = "Offline code evidence CLI"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Index a workspace into a local evidence ledger.
    Index(IndexArgs),
    /// Report projects, latest snapshots, counts, languages and optional
    /// workspace freshness without mutating source or history.
    Status(StatusArgs),
    /// Review the current workspace against a frozen snapshot. Static only:
    /// no build, test or runtime command is executed.
    ReviewDiff(ReviewDiffArgs),
    /// List or delete persisted review reports and their retention pins.
    ReviewReport(ReviewReportArgs),
    /// Import human-attested structural relations (reads/writes/owns).
    Evidence(EvidenceArgs),
    /// Index the current workspace as a new snapshot and report the exact diff
    /// from the previous snapshot. The old snapshot and evidence are retained.
    Refresh(RefreshArgs),
    /// Export a checksummed offline report bundle for one snapshot.
    Report(ReportArgs),
    /// Verify every file in an existing report bundle.
    VerifyReport(VerifyArgs),
    /// Create a passphrase-encrypted backup of the ledger and CAS objects.
    Backup(BackupArgs),
    /// Verify an encrypted backup bundle.
    VerifyBackup(VerifyBackupArgs),
    /// Restore an encrypted backup into a new state directory.
    RestoreBackup(RestoreBackupArgs),
    /// Re-encrypt an encrypted backup under a new passphrase.
    RotateBackup(RotateBackupArgs),
    /// Copy a verified encrypted backup to a path-based retention target.
    RetainBackup(RetainBackupArgs),
    /// Run one scenario command against an existing snapshot.
    Scenario(ScenarioArgs),
    /// Poll a workspace for drift against a base snapshot.
    Watch(WatchArgs),
    /// Import a native runtime trace into an existing snapshot.
    TraceImport(TraceImportArgs),
    /// Copy a complete ledger into a new, empty state directory with integrity
    /// checks. Merging into an existing ledger is refused.
    LedgerMigrate(MigrateArgs),
    /// Evaluate a numeric_tolerance predicate against artifacts without
    /// creating a verification or accepting a ChangeSet.
    NumericCheck(NumericCheckArgs),
    /// One-command onboarding: index the workspace, register its root, render
    /// MCP configuration and install the agent rule block.
    Init(InitArgs),
    /// Print or install the MCP server entry for a supported client.
    Mcp(McpArgs),
    /// Build a one-screen project or change brief from the current snapshot.
    Brief(BriefArgs),
    /// Validate or report a committed evidence-bound cognition map (read-only).
    Map(MapArgs),
}

#[cfg(test)]
mod command_surface {
    use super::*;
    use clap::CommandFactory;

    fn names(command: &clap::Command) -> Vec<String> {
        let mut names: Vec<String> = command
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_string())
            .filter(|name| name != "help")
            .collect();
        names.sort();
        names
    }

    fn child<'a>(command: &'a clap::Command, name: &str) -> &'a clap::Command {
        command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == name)
            .unwrap_or_else(|| panic!("missing subcommand {name}"))
    }

    /// A module split must not add, rename or remove a command. The nested
    /// action enums are part of the same public surface, so they are pinned
    /// here as well.
    #[test]
    fn command_tree_is_stable_across_module_splits() {
        let command = Cli::command();
        assert_eq!(
            names(&command),
            vec![
                "backup",
                "brief",
                "evidence",
                "index",
                "init",
                "ledger-migrate",
                "map",
                "mcp",
                "numeric-check",
                "refresh",
                "report",
                "restore-backup",
                "retain-backup",
                "review-diff",
                "review-report",
                "rotate-backup",
                "scenario",
                "status",
                "trace-import",
                "verify-backup",
                "verify-report",
                "watch",
            ]
        );
        assert_eq!(names(child(&command, "evidence")), vec!["import-human"]);
        assert_eq!(
            names(child(&command, "map")),
            vec!["check", "maintain", "status"]
        );
        assert_eq!(names(child(&command, "mcp")), vec!["install"]);
        assert_eq!(
            names(child(&command, "review-report")),
            vec!["delete", "list"]
        );
    }
}
