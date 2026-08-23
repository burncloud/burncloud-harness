mod agent_activity;
mod analysis;
mod burncloud;
mod checks;
mod config;
mod event_writer;
mod events;
mod evidence;
mod git;
mod invariants;
mod logging;
mod observer;
mod policy;
mod proposal;
mod risk;
mod route;
mod run_explain;
mod run_history;
mod run_state;
mod runner;
mod trajectory;
mod tui;
mod ui_daemon;
mod ui_visual;

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
    Tui {
        #[arg(long, default_value = "../burncloud")]
        workspace: PathBuf,
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        list: bool,
    },
    Doctor {
        #[arg(default_value = ".")]
        workspace: PathBuf,
    },
    Explain {
        #[arg(short, long)]
        task: PathBuf,
    },
    VerifyUi {
        #[arg(short, long)]
        task: PathBuf,
    },
    UiDaemon {
        #[arg(long, default_value = "tasks/ui")]
        tasks_dir: PathBuf,
        #[arg(long, default_value = "../burncloud")]
        workspace: PathBuf,
        #[arg(long, default_value = "../burncloud-ui")]
        source_workspace: PathBuf,
        #[arg(long, default_value = "ce4fa9d2e79928a388bffa363a1eec77f6998900")]
        source_revision: String,
        #[arg(long)]
        plan: bool,
        #[arg(long)]
        status: bool,
        #[arg(long)]
        once: bool,
        #[arg(long, default_value_t = 15)]
        retry_delay_seconds: u64,
    },
    ExplainRun {
        #[arg(long)]
        run: String,
        #[arg(long, default_value = "../burncloud")]
        workspace: PathBuf,
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
        resume: bool,
        #[arg(long, requires = "resume")]
        verify_existing: bool,
    },
}

fn main() -> Result<()> {
    logging::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Tui {
            workspace,
            run,
            list,
        } => {
            if list {
                tui::list_runs(&workspace)?;
            } else {
                tui::run(&workspace, run.as_deref())?;
            }
        }
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
        Commands::VerifyUi { task } => verify_ui(TaskSpec::load(task)?)?,
        Commands::UiDaemon {
            tasks_dir,
            workspace,
            source_workspace,
            source_revision,
            plan,
            status,
            once,
            retry_delay_seconds,
        } => ui_daemon::run(ui_daemon::UiDaemonOptions {
            tasks_dir,
            workspace,
            source_workspace,
            source_revision,
            plan_only: plan,
            status_only: status,
            once,
            retry_delay: std::time::Duration::from_secs(retry_delay_seconds),
        })?,
        Commands::ExplainRun { run, workspace } => explain_run(&workspace, &run)?,
        Commands::Analyze { workspace, limit } => analyze_workspace(workspace, limit)?,
        Commands::Recommend {
            workspace,
            limit,
            min_count,
        } => recommend_workspace(workspace, limit, min_count)?,
        Commands::Run {
            task,
            resume,
            verify_existing,
        } => {
            let task = TaskSpec::load(task)?;
            let summary = if verify_existing {
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

fn verify_ui(task: TaskSpec) -> Result<()> {
    let workspace = PathBuf::from(task.workspace.as_str()).canonicalize()?;
    let visual = task
        .visual
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("task '{}' has no visual contract", task.name))?;
    let visual_target = workspace
        .join("target")
        .join("burncloud-harness-visual-build");
    println!(
        "{}",
        ui_visual::run(&workspace, visual, Some(&visual_target))?
    );
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

fn explain_run(workspace: &std::path::Path, run_id: &str) -> Result<()> {
    let workspace = workspace.canonicalize()?;
    let git = git::GitRepo::new(workspace);
    git.ensure_repository()?;
    let state_dir = git.harness_state_dir()?;
    let artifact = run_history::resolve(&state_dir, Some(run_id))?;
    let replay = run_history::load(&artifact)?;
    print!("{}", run_explain::render(&replay.state));
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
