use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::{
        AgentSpec, BurncloudArea, ResolvedContextFile, ScopeSpec, TaskSpec, UiPixelMatchSpec,
        UiVisualSpec,
    },
    git::GitRepo,
    runner,
};

const STATE_SCHEMA: u32 = 1;

#[derive(Debug, Clone)]
pub struct UiDaemonOptions {
    pub tasks_dir: PathBuf,
    pub workspace: PathBuf,
    pub source_workspace: PathBuf,
    pub source_revision: String,
    pub plan_only: bool,
    pub once: bool,
    pub retry_delay: Duration,
}

#[derive(Debug, Deserialize)]
struct CampaignSpec {
    source_revision: String,
    units: Vec<CampaignUnit>,
}

#[derive(Debug, Deserialize)]
struct CampaignUnit {
    order: u32,
    task_file: Option<String>,
    name: Option<String>,
    title: Option<String>,
    route: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    source_page: Option<String>,
    contract: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UiDaemonState {
    schema: u32,
    branch: String,
    source_revision: String,
    completed: BTreeMap<String, CompletedTask>,
    active: Option<ActiveTask>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CompletedTask {
    commit: String,
    run_id: String,
    changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActiveTask {
    name: String,
    task_path: String,
    phase: ActivePhase,
    run_id: Option<String>,
    changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ActivePhase {
    Running,
    PassedAwaitingCommit,
}

pub fn run(options: UiDaemonOptions) -> Result<()> {
    let workspace = options
        .workspace
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", options.workspace.display()))?;
    let source_workspace = options
        .source_workspace
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", options.source_workspace.display()))?;
    verify_source_pin(&source_workspace, &options.source_revision)?;

    let repo = GitRepo::new(&workspace);
    repo.ensure_repository()?;
    let branch = repo.branch_name()?;
    let state_dir = repo.harness_state_dir()?.join("ui-daemon");
    fs::create_dir_all(&state_dir)?;
    let state_path = state_dir.join("state.json");
    let mut state = load_state(&state_path, &branch, &options.source_revision)?;
    let tasks = load_tasks(
        &options.tasks_dir,
        &workspace,
        &source_workspace,
        &options.source_revision,
        &state_dir,
    )?;
    if tasks.is_empty() {
        bail!(
            "no UI migration task YAML files found in {}",
            options.tasks_dir.display()
        );
    }
    if options.plan_only {
        for (index, (_, task)) in tasks.iter().enumerate() {
            let route = task
                .visual
                .as_ref()
                .map(|visual| visual.route.as_str())
                .unwrap_or("<missing>");
            println!("{:02} {} {}", index + 1, task.name, route);
        }
        println!("UI_DAEMON_PLAN tasks={}", tasks.len());
        return Ok(());
    }

    if state.active.is_none() {
        repo.ensure_clean()?;
    }

    loop {
        if let Some(active) = state.active.clone() {
            if active.phase == ActivePhase::PassedAwaitingCommit {
                commit_active(&repo, &state_path, &mut state, active)?;
                if options.once {
                    return Ok(());
                }
                continue;
            }
        }

        let Some((task_path, task)) = tasks
            .iter()
            .find(|(_, task)| !state.completed.contains_key(&task.name))
        else {
            state.active = None;
            state.last_error = None;
            save_state(&state_path, &state)?;
            println!("UI_DAEMON_COMPLETE tasks={}", state.completed.len());
            return Ok(());
        };

        if task
            .visual
            .as_ref()
            .and_then(|visual| visual.pixel_match.as_ref())
            .is_none()
        {
            bail!(
                "daemon task '{}' must declare visual.pixel_match thresholds",
                task.name
            );
        }

        let is_resume = match state.active.as_ref() {
            Some(active) if active.name == task.name => true,
            Some(active) => bail!(
                "daemon state has active task '{}' but next manifest task is '{}'",
                active.name,
                task.name
            ),
            None => {
                repo.ensure_clean()?;
                state.active = Some(ActiveTask {
                    name: task.name.clone(),
                    task_path: task_path.display().to_string(),
                    phase: ActivePhase::Running,
                    run_id: None,
                    changed_paths: Vec::new(),
                });
                state.last_error = None;
                save_state(&state_path, &state)?;
                false
            }
        };

        println!(
            "UI_DAEMON_TASK name={} mode={}",
            task.name,
            if is_resume { "resume" } else { "start" }
        );
        let result = if is_resume && !repo.changed_paths()?.is_empty() {
            runner::resume_ui_migration(task.clone())
        } else {
            runner::run_ui_migration(task.clone())
        };

        match result {
            Ok(summary) => {
                let actual_paths = repo.changed_paths()?;
                if actual_paths != summary.changed_paths {
                    bail!(
                        "task '{}' passed but worktree paths changed after verification: expected {:?}, got {:?}",
                        task.name,
                        summary.changed_paths,
                        actual_paths
                    );
                }
                state.active = Some(ActiveTask {
                    name: task.name.clone(),
                    task_path: task_path.display().to_string(),
                    phase: ActivePhase::PassedAwaitingCommit,
                    run_id: Some(summary.run_id),
                    changed_paths: summary.changed_paths,
                });
                state.last_error = None;
                save_state(&state_path, &state)?;
            }
            Err(error) => {
                state.last_error = Some(format!("{error:#}"));
                save_state(&state_path, &state)?;
                eprintln!("UI_DAEMON_RETRY task={} error={error:#}", task.name);
                if options.once {
                    return Err(error);
                }
                thread::sleep(options.retry_delay);
            }
        }
    }
}

fn commit_active(
    repo: &GitRepo,
    state_path: &Path,
    state: &mut UiDaemonState,
    active: ActiveTask,
) -> Result<()> {
    let actual_paths = repo.changed_paths()?;
    if actual_paths != active.changed_paths {
        bail!(
            "refusing to commit task '{}': verified paths {:?}, current paths {:?}",
            active.name,
            active.changed_paths,
            actual_paths
        );
    }
    let message = format!("feat(ui): migrate {} from aether source", active.name);
    let commit = repo.commit_paths(&active.changed_paths, &message)?;
    let run_id = active
        .run_id
        .clone()
        .context("passed daemon task is missing its Harness run ID")?;
    state.completed.insert(
        active.name.clone(),
        CompletedTask {
            commit: commit.clone(),
            run_id,
            changed_paths: active.changed_paths,
        },
    );
    state.active = None;
    state.last_error = None;
    save_state(state_path, state)?;
    println!("UI_DAEMON_COMMIT task={} commit={commit}", active.name);
    Ok(())
}

fn load_tasks(
    tasks_dir: &Path,
    workspace: &Path,
    source_workspace: &Path,
    source_revision: &str,
    state_dir: &Path,
) -> Result<Vec<(PathBuf, TaskSpec)>> {
    let tasks_dir = tasks_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve task directory {}", tasks_dir.display()))?;
    let campaign_path = tasks_dir.join("campaign.yaml");
    let campaign: CampaignSpec = serde_yaml::from_slice(
        &fs::read(&campaign_path)
            .with_context(|| format!("failed to read {}", campaign_path.display()))?,
    )?;
    if campaign.source_revision != source_revision {
        bail!(
            "campaign source pin is {}, daemon source pin is {}",
            campaign.source_revision,
            source_revision
        );
    }

    let harness_root = tasks_dir
        .parent()
        .and_then(Path::parent)
        .context("tasks directory must be nested under the Harness repository")?
        .canonicalize()?;
    let generated_dir = state_dir.join("generated-tasks");
    fs::create_dir_all(&generated_dir)?;
    let mut tasks = Vec::new();
    let mut units = campaign.units;
    units.sort_by_key(|unit| unit.order);

    for unit in units {
        let (path, mut task) = if let Some(task_file) = unit.task_file {
            let path = tasks_dir.join(task_file);
            let task = TaskSpec::load(&path)?;
            (path, task)
        } else {
            generated_task(
                unit,
                workspace,
                source_workspace,
                source_revision,
                &harness_root,
                &generated_dir,
            )?
        };
        let declared_workspace = PathBuf::from(&task.workspace);
        let task_workspace = if declared_workspace.is_absolute() {
            declared_workspace.canonicalize()?
        } else {
            harness_root.join(declared_workspace).canonicalize()?
        };
        if task_workspace != workspace {
            bail!(
                "task '{}' targets {}, daemon targets {}",
                task.name,
                task_workspace.display(),
                workspace.display()
            );
        }
        task.workspace = task_workspace.display().to_string();
        tasks.push((path, task));
    }
    Ok(tasks)
}

fn generated_task(
    unit: CampaignUnit,
    workspace: &Path,
    source_workspace: &Path,
    source_revision: &str,
    harness_root: &Path,
    generated_dir: &Path,
) -> Result<(PathBuf, TaskSpec)> {
    let name = unit.name.context("generated campaign unit requires name")?;
    let title = unit
        .title
        .context("generated campaign unit requires title")?;
    let route = unit
        .route
        .context("generated campaign unit requires route")?;
    let source_page = unit
        .source_page
        .context("generated campaign unit requires source_page")?;
    let aliases = if unit.aliases.is_empty() {
        route.clone()
    } else {
        unit.aliases.join(", ")
    };
    let source_page_path = source_workspace.join(&source_page);
    if !source_page_path.is_file() {
        bail!("source page does not exist: {}", source_page_path.display());
    }

    let mut context_paths = vec![
        harness_root.join("docs/ui/source-migration-fidelity.md"),
        harness_root.join("docs/ui/product-standard.md"),
        harness_root.join("docs/ui/information-architecture.md"),
        harness_root.join("docs/ui/design-system.md"),
        harness_root.join("docs/ui/interaction-rules.md"),
        harness_root.join("docs/ui/content-standard.md"),
        source_workspace.join("src/App.tsx"),
        source_workspace.join("src/components/Layout.tsx"),
        source_workspace.join("src/components/ui.tsx"),
        source_workspace.join("src/context/RoleContext.tsx"),
        source_workspace.join("src/data/workbenchData.ts"),
        source_workspace.join("src/index.css"),
        source_page_path,
    ];
    if let Some(contract) = unit.contract {
        context_paths.push(harness_root.join(contract));
    }
    let resolved_context_files = context_paths
        .iter()
        .map(|path| {
            let absolute_path = path
                .canonicalize()
                .with_context(|| format!("missing generated task context {}", path.display()))?;
            Ok(ResolvedContextFile {
                declared_path: absolute_path.display().to_string(),
                absolute_path,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let context_files = resolved_context_files
        .iter()
        .map(|context| context.declared_path.clone())
        .collect();

    let goal = format!(
        "Port the active {title} page from burncloud-ui into BurnCloud's Rust/Dioxus client with strict source fidelity.\n\nSOURCE: pinned commit {source_revision}, page {source_page}, canonical route {route}, aliases {aliases}. Reproduce the source DOM topology, Tailwind classes, generated CSS behavior, shell, icons, typography, spacing, responsive behavior, controls, initial states and interactions. Use the source class names and shared source stylesheet directly where practical; do not reinterpret the design through legacy BurnCloud components. Keep production data truthful, but expose the deterministic aether-ce4fa9 visual fixture during Harness capture so pixel comparison uses the same values. Add the canonical route and aliases to Dioxus routing. Preserve en, zh, zh-TW and ja localization. Work only on this page and reusable shared primitives required by it. Do not stop at compile success: inspect Harness reference/local/diff PNGs and iterate until desktop and mobile strict pixel comparison pass."
    );
    let task = TaskSpec {
        name: name.clone(),
        goal,
        workspace: workspace.display().to_string(),
        max_loops: 12,
        area: BurncloudArea::Ui,
        scope: ScopeSpec {
            allowed: vec![
                "crates/client/**".to_owned(),
                "crates/tests/tests/e2e/**".to_owned(),
            ],
            avoid: vec![
                "crates/router/**".to_owned(),
                "crates/server/**".to_owned(),
                "crates/service/**".to_owned(),
                "crates/database/**".to_owned(),
            ],
            max_changed_files: Some(12),
        },
        agent: AgentSpec {
            program: "codex".to_owned(),
            args: vec!["exec".to_owned(), "--full-auto".to_owned()],
            append_prompt: true,
            soft_timeout_minutes: Some(40),
            hard_timeout_minutes: Some(60),
            idle_timeout_minutes: Some(10),
        },
        context_files,
        resolved_context_files,
        extra_checks: Vec::new(),
        visual: Some(UiVisualSpec {
            route: route.clone(),
            reference_url: Some(format!("https://aether-router.ai.studio{}", route)),
            required_selectors: vec!["[data-visual-fixture=\"aether-ce4fa9\"]".to_owned()],
            metric_labels: Vec::new(),
            section_titles: Vec::new(),
            clipping_selectors: Vec::new(),
            locale_selector: None,
            locale_title_selector: None,
            locale_titles: Vec::new(),
            mobile_menu: None,
            pixel_match: Some(UiPixelMatchSpec {
                channel_tolerance: 0,
                max_changed_pixel_ratio: 0.0,
                max_mean_channel_delta: 0.0,
            }),
        }),
    };
    task.validate()?;
    let path = generated_dir.join(format!("{:02}-{}.yaml", unit.order, name));
    fs::write(
        &path,
        format!("# Generated from tasks/ui/campaign.yaml\n# {title}\n"),
    )?;
    Ok((path, task))
}

fn verify_source_pin(source: &Path, expected: &str) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(source)
        .output()?;
    if !output.status.success() {
        bail!("failed to resolve source UI revision");
    }
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if actual != expected {
        bail!("source UI revision drift: expected {expected}, got {actual}");
    }
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(source)
        .output()?;
    if !output.status.success() || !output.stdout.is_empty() {
        bail!("source UI workspace must be clean at pinned revision {expected}");
    }
    Ok(())
}

fn load_state(path: &Path, branch: &str, source_revision: &str) -> Result<UiDaemonState> {
    if !path.is_file() {
        return Ok(UiDaemonState {
            schema: STATE_SCHEMA,
            branch: branch.to_owned(),
            source_revision: source_revision.to_owned(),
            completed: BTreeMap::new(),
            active: None,
            last_error: None,
        });
    }
    let state: UiDaemonState = serde_json::from_slice(&fs::read(path)?)?;
    if state.schema != STATE_SCHEMA {
        bail!("unsupported UI daemon state schema {}", state.schema);
    }
    if state.branch != branch {
        bail!(
            "UI daemon state belongs to branch '{}', current branch is '{branch}'",
            state.branch
        );
    }
    if state.source_revision != source_revision {
        bail!(
            "UI daemon source pin is {}, requested {}",
            state.source_revision,
            source_revision
        );
    }
    Ok(state)
}

fn save_state(path: &Path, state: &UiDaemonState) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}
