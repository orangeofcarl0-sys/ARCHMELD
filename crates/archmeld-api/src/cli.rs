// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Offline operator CLI: index a workspace, export a review bundle, or verify
//! an existing bundle. No model provider, network or server is required.

mod commands;
mod memory_limit;
mod spec;

use clap::Parser;
use commands::*;
use serde_json::json;
use spec::{Cli, Commands};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Index(args) => run_index(args),
        Commands::Status(args) => run_status(args),
        Commands::ReviewDiff(args) => run_review_diff(args),
        Commands::ReviewReport(args) => run_review_report(args),
        Commands::Evidence(args) => run_evidence(args),
        Commands::Refresh(args) => run_refresh(args),
        Commands::Report(args) => run_report(args),
        Commands::VerifyReport(args) => run_verify(args),
        Commands::Backup(args) => run_backup(args),
        Commands::VerifyBackup(args) => run_verify_backup(args),
        Commands::RestoreBackup(args) => run_restore_backup(args),
        Commands::RotateBackup(args) => run_rotate_backup(args),
        Commands::RetainBackup(args) => run_retain_backup(args),
        Commands::Scenario(args) => run_scenario_command(args),
        Commands::Watch(args) => run_watch(args),
        Commands::TraceImport(args) => run_trace_import(args),
        Commands::LedgerMigrate(args) => run_migrate(args),
        Commands::NumericCheck(args) => run_numeric_check(args),
        Commands::Init(args) => run_init(args),
        Commands::Mcp(args) => run_mcp_command(args),
        Commands::Brief(args) => run_brief(args),
        Commands::Map(args) => run_map_command(args),
    };
    let invalid = matches!(&result, Ok(value) if value["state"] == "invalid");
    match result {
        Ok(value) => {
            if let Some(text) = value.as_str() {
                println!("{text}");
            } else if let Some(text) = value["markdown"].as_str() {
                println!("{text}");
            } else {
                println!("{value}");
            }
            if invalid {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("{}", json!({"error": error}));
            ExitCode::FAILURE
        }
    }
}
