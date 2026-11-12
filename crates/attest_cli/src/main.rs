//! CLI `attest` (PLAN.md 2.6, owner W4). Freeze-1 stub: the command tree only.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "attest", version, about = "beacon-veil Ethereum state attestation CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create an instance from a checkpoint.
    Init,
    /// Submit one light-client update.
    Update,
    /// Show an instance's head.
    Status,
    /// Show or write the CLI config.
    Config,
    /// Attest a claim (`--dry-run` checks on the host with the on-chain error codes).
    Attest {
        #[arg(long)]
        dry_run: bool,
    },
    /// Checkpoint tools.
    Checkpoint {
        #[command(subcommand)]
        cmd: CheckpointCmd,
    },
}

#[derive(Subcommand)]
enum CheckpointCmd {
    /// Compare an instance's checkpoint with N public beacon sources (F9 fallback).
    Verify,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let name = match cli.cmd {
        Cmd::Init => "init",
        Cmd::Update => "update",
        Cmd::Status => "status",
        Cmd::Config => "config",
        Cmd::Attest { .. } => "attest",
        Cmd::Checkpoint {
            cmd: CheckpointCmd::Verify,
        } => "checkpoint verify",
    };
    anyhow::bail!("attest {name}: not implemented (freeze-1 stub)")
}
