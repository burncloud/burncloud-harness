mod analysis;
mod burncloud;
mod checks;
mod config;
mod console;
mod event_writer;
mod events;
mod git;
mod invariants;
mod observer;
mod policy;
mod proposal;
mod risk;
mod route;
mod runner;
mod trajectory;
mod tui;

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
    Tui,
    Doctor {
        #[arg(default_value = ".")]
        workspace: PathBuf,
    },
    Explain {
        #[arg(short, long)]
        task: PathBuf,
    },
    Analyze {
        #[arg(default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    Recommend {
        #[arg(default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value_t = 3)]
        min_count: usize,
    },
    Run {
        #[arg(short, long)]
        task: PathBuf,
        #[arg(long)]
        tui: bool,
        #[arg(long, conflicts_with = "tui")]
        resume: bool,
        #[arg(long, requires = "resume", conflicts_with = "tui")]
        verify_existing: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Tui => tui::run()?,
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
        Commands::Analyze { workspace, limit } => analyze_workspace(workspace, limit)?,
        Commands::Recommend {
            workspace,
            limit,
            min_count,
        } => recommend_workspace(workspace, limit, min_count)?,
        Commands::Run {
            task,
            tui,
            resume,
            verify_existing,
        } => {
            let task = TaskSpec::load(task)?;
            let summary = if tui {
                console::run(task)?
            } else if verify_existing {
                runner::verify_existing(task)?
            } else if resume {
                runner::resume(task)?
            } else {
                runner::run(task)?
            };
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

fn analyze_workspace(workspace: PathBuf, limit: usize) -> Result<()> {
    let report = load_analysis(workspace, limit)?;
    print!("{}", report.render());
    Ok(())
}

fn recommend_workspace(workspace: PathBuf, limit: usize, min_count: usize) -> Result<()> {
    let report = load_analysis(workspace, limit)?;
    let proposals = proposal::build(&report, min_count);
    print!("{}", proposal::render(&proposals, min_count));
    Ok(())
}

fn load_analysis(workspace: PathBuf, limit: usize) -> Result<analysis::AnalysisReport> {
    let workspace = workspace.canonicalize()?;
    let burncloud = burncloud::BurncloudRepo::open(workspace.as_path())?;
    let git = git::GitRepo::new(burncloud.root());
    git.ensure_repository()?;
    let state_dir = git.harness_state_dir()?;
    analysis::analyze(&state_dir, limit)
}
