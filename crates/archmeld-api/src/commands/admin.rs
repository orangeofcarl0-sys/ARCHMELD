// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `admin` CLI commands.

use super::*;

#[derive(clap::Args)]
pub(crate) struct McpArgs {
    #[command(subcommand)]
    pub(crate) action: McpAction,
}

#[derive(Subcommand)]
pub(crate) enum McpAction {
    /// Render or install one client's MCP server entry.
    Install(McpInstallArgs),
}

#[derive(clap::Args)]
pub(crate) struct McpInstallArgs {
    /// claude, codex or generic.
    #[arg(long, default_value = "claude")]
    pub(crate) client: String,
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    /// Execute the printed client command instead of only printing it.
    #[arg(long)]
    pub(crate) write: bool,
    /// Run a real initialize/tools/list/status handshake.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(clap::Args)]
pub(crate) struct MigrateArgs {
    /// Existing complete ledger to copy. It is opened read-only.
    #[arg(long)]
    pub(crate) from: PathBuf,
    /// Destination state directory. It must not exist.
    #[arg(long)]
    pub(crate) to: PathBuf,
    /// Operator justification recorded in migration.json.
    #[arg(long)]
    pub(crate) reason: String,
}

pub(crate) fn run_mcp_command(args: McpArgs) -> Result<Value, String> {
    match args.action {
        McpAction::Install(install) => {
            let plan =
                onboard::plan_mcp_install(&install.client, &install.state, &install.project)?;
            let mut result = json!({"command": "mcp.install", "plan": plan});
            if install.check {
                result["health"] = onboard::health_check(&install.state, &install.project)?;
            }
            if install.write {
                result["install"] = onboard::execute_mcp_install(&plan)?;
            }
            Ok(result)
        }
    }
}

pub(crate) fn run_migrate(args: MigrateArgs) -> Result<Value, String> {
    let receipt =
        migrate_ledger(&args.from, &args.to, &args.reason).map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "ledger-migrate",
        "from": args.from,
        "to": args.to,
        "receipt": receipt,
    }))
}
