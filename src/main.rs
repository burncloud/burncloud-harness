mod burncloud;
mod checks;
mod config;
mod git;
mod policy;
mod runner;
mod trajectory;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::TaskSpec;

#[derive(Parser)]
#[command(name = "burncloud-harness")]
#[command(about = "A repository-specific coding harness for burncloud/burncloud")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate a BurnCloud checkout and its required agent-control documents.
    Doctor {
        #[arg(default_value = ".")]
        workspace: PathBuf,
    },
    /// Run one bounded coding task against a clean BurnCloud worktree.
    Run {
        #[arg(short, long)]
        task: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Doctor { workspace } => {
            let workspace = workspace.canonicalize()?;
            burncloud::BurncloudRepo::open(workspace.as_path())?;
            git::GitRepo::new(workspace.as_path()).ensure_repository()?;
            println!("BurnCloud harness preflight passed: {}", workspace.display());
        }
        Commands::Run { task } => {
            let task = TaskSpec::load(task)?;
            let summary = runner::run(task)?;
            println!("PASS run={} attempts={}", summary.run_id, summary.attempts);
            println!("changed={}", summary.changed_paths.join(", "));
            println!("trajectory={}", summary.trajectory_path.display());
        }
    }

    Ok(())
}
