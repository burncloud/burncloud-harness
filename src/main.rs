mod burncloud;
mod checks;
mod config;
mod git;
mod invariants;
mod policy;
mod risk;
mod route;
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
    /// Show how the harness routes a task before allowing an agent to edit code.
    Explain {
        #[arg(short, long)]
        task: PathBuf,
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
            println!(
                "BurnCloud harness preflight passed: {}",
                workspace.display()
            );
        }
        Commands::Explain { task } => explain_task(TaskSpec::load(task)?)?,
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

fn explain_task(task: TaskSpec) -> Result<()> {
    let workspace = PathBuf::from(task.workspace.as_str()).canonicalize()?;
    let burncloud = burncloud::BurncloudRepo::open(workspace.as_path())?;
    git::GitRepo::new(burncloud.root()).ensure_repository()?;

    let routes = route::resolve(burncloud.root(), &task.goal, task.area)?;
    let selected_invariants =
        invariants::resolve(burncloud.root(), task.area, &task.goal, &routes)?;

    println!("task={}", task.name);
    println!("area={}", task.area.as_str());
    println!("goal={}", task.goal);
    println!("\nTASK_ROUTER starting points:\n{}", routes.prompt_text());
    println!(
        "\nCandidate invariants:\n{}",
        selected_invariants.prompt_text()
    );
    println!("\nAllowed scope:\n- {}", task.scope.allowed.join("\n- "));
    if !task.scope.avoid.is_empty() {
        println!("\nAvoid scope:\n- {}", task.scope.avoid.join("\n- "));
    }

    Ok(())
}
