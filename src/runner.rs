use std::{path::PathBuf, process::Command};

use anyhow::{bail, Context, Result};

use crate::{
    burncloud::BurncloudRepo,
    checks::{plan_checks, run_check},
    config::TaskSpec,
    git::GitRepo,
    invariants,
    policy::ScopePolicy,
    route,
    trajectory::{new_run_id, Event, TrajectoryWriter},
};

pub struct RunSummary {
    pub run_id: String,
    pub attempts: u32,
    pub changed_paths: Vec<String>,
    pub trajectory_path: PathBuf,
}

pub fn run(task: TaskSpec) -> Result<RunSummary> {
    let workspace = PathBuf::from(task.workspace.as_str())
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace {}", task.workspace))?;
    let burncloud = BurncloudRepo::open(workspace.as_path())?;
    let git = GitRepo::new(burncloud.root());
    git.ensure_repository()?;
    git.ensure_clean()?;

    let routes = route::resolve(burncloud.root(), &task.goal, task.area)?;
    let mut active_invariants =
        invariants::resolve(burncloud.root(), task.area, &task.goal, &routes)?;
    let route_labels = routes.labels();
    let initial_invariant_ids = active_invariants.ids();

    let baseline_head = git.head_sha()?;
    let scope = ScopePolicy::compile(&task.scope)?;
    let run_id = new_run_id();
    let state_dir = git.harness_state_dir()?;
    let mut trajectory = TrajectoryWriter::create(&state_dir, &run_id)?;
    trajectory.record(Event::RunStarted {
        run_id: &run_id,
        task: &task.name,
        goal: &task.goal,
        area: task.area.as_str(),
        max_loops: task.max_loops,
    })?;
    trajectory.record(Event::TaskRouted {
        routes: &route_labels,
    })?;
    trajectory.record(Event::InvariantsSelected {
        invariants: &initial_invariant_ids,
    })?;

    let mut previous_feedback: Option<String> = None;

    for attempt in 1..=task.max_loops {
        trajectory.record(Event::AttemptStarted { attempt })?;
        let prompt = burncloud.control_prompt(
            &task,
            &routes,
            &active_invariants,
            attempt,
            previous_feedback.as_deref(),
        );
        let agent_result = run_agent(&workspace, &task, &prompt)?;
        trajectory.record(Event::AgentFinished {
            attempt,
            success: agent_result.success,
            exit_code: agent_result.exit_code,
            stdout: &agent_result.stdout,
            stderr: &agent_result.stderr,
        })?;

        let current_head = git.head_sha()?;
        let head_unchanged = current_head == baseline_head;
        trajectory.record(Event::GitHeadChecked {
            attempt,
            baseline: &baseline_head,
            current: &current_head,
            unchanged: head_unchanged,
        })?;
        if !head_unchanged {
            let changed_paths = git.changed_paths()?;
            trajectory.record(Event::RunFinished {
                success: false,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            bail!(
                "agent changed git HEAD from {} to {}; burncloud-harness forbids commits/history changes",
                baseline_head,
                current_head
            );
        }

        let changed_paths = git.changed_paths()?;
        let report = scope.evaluate(&changed_paths);
        trajectory.record(Event::ScopeEvaluated {
            attempt,
            changed_paths: &changed_paths,
            violations: &report.violations,
        })?;

        if !report.is_ok() {
            trajectory.record(Event::RunFinished {
                success: false,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            bail!(
                "scope violation; refusing to continue because unauthorized changes already exist: {}",
                report.violations.join(", ")
            );
        }

        if !changed_paths.is_empty() {
            let impact = invariants::assess_changed_paths(
                burncloud.root(),
                &changed_paths,
                &active_invariants,
            )?;
            let required_ids = impact.required.ids();
            let newly_required_ids = impact.newly_required.ids();
            trajectory.record(Event::InvariantImpactAssessed {
                attempt,
                required: &required_ids,
                newly_required: &newly_required_ids,
                reasons: &impact.reasons,
            })?;

            if !impact.newly_required.is_empty() {
                active_invariants.merge(&impact.newly_required);
                let mut feedback = format!(
                    "The actual BurnCloud diff expanded invariant impact beyond the pre-change contract. Before completion, re-read and verify these newly required invariants against current source: {}.\nImpact evidence:\n{}\nDo not widen scope unless source evidence requires NEED_SCOPE_EXPANSION.",
                    newly_required_ids.join(", "),
                    impact.reasons.join("\n")
                );
                if !agent_result.success {
                    feedback.push_str(&format!(
                        "\nThe agent command also failed with exit code {:?}:\n{}",
                        agent_result.exit_code,
                        compact_failure(&agent_result.stderr, &agent_result.stdout)
                    ));
                }
                trajectory.record(Event::AttemptFailed {
                    attempt,
                    feedback: &feedback,
                })?;
                previous_feedback = Some(feedback);
                continue;
            }
        }

        if !agent_result.success {
            let feedback = format!(
                "Agent command failed with exit code {:?}.\n{}",
                agent_result.exit_code,
                compact_failure(&agent_result.stderr, &agent_result.stdout)
            );
            trajectory.record(Event::AttemptFailed {
                attempt,
                feedback: &feedback,
            })?;
            previous_feedback = Some(feedback);
            continue;
        }

        if changed_paths.is_empty() {
            let feedback = "No repository changes were produced. Re-check the goal and either make the smallest in-scope change or clearly report why no change is required.".to_owned();
            trajectory.record(Event::AttemptFailed {
                attempt,
                feedback: &feedback,
            })?;
            previous_feedback = Some(feedback);
            continue;
        }

        let diff = git.diff()?;
        if adds_ignored_test(&diff) {
            trajectory.record(Event::RunFinished {
                success: false,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            bail!("diff adds #[ignore] to a test; burncloud-harness refuses test weakening");
        }

        let active_invariant_ids = active_invariants.ids();
        let checks = plan_checks(
            &changed_paths,
            &active_invariant_ids,
            &task.extra_checks,
        );
        let mut failed = Vec::new();

        for check in checks {
            let result = run_check(&workspace, &check)?;
            trajectory.record(Event::CheckFinished {
                attempt,
                name: &result.name,
                command: &result.command,
                reason: &result.reason,
                success: result.success,
                exit_code: result.exit_code,
                stdout: &result.stdout,
                stderr: &result.stderr,
            })?;

            if !result.success {
                failed.push(format!(
                    "{} (`{}`): {}",
                    result.name,
                    result.command,
                    compact_failure(&result.stderr, &result.stdout)
                ));
            }
        }

        if failed.is_empty() {
            trajectory.record(Event::RunFinished {
                success: true,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            return Ok(RunSummary {
                run_id,
                attempts: attempt,
                changed_paths,
                trajectory_path: trajectory.path().to_path_buf(),
            });
        }

        let feedback = format!(
            "BurnCloud mandatory verification failed. Fix the existing in-scope change; do not widen scope or weaken tests.\n{}",
            failed.join("\n")
        );
        trajectory.record(Event::AttemptFailed {
            attempt,
            feedback: &feedback,
        })?;
        previous_feedback = Some(feedback);
    }

    let changed_paths = git.changed_paths()?;
    trajectory.record(Event::RunFinished {
        success: false,
        attempts: task.max_loops,
        changed_paths: &changed_paths,
    })?;
    bail!(
        "task did not pass burncloud-harness after {} attempts; trajectory: {}",
        task.max_loops,
        trajectory.path().display()
    )
}

struct AgentResult {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_agent(workspace: &std::path::Path, task: &TaskSpec, prompt: &str) -> Result<AgentResult> {
    let mut command = Command::new(&task.agent.program);
    command.args(&task.agent.args).current_dir(workspace);
    if task.agent.append_prompt {
        command.arg(prompt);
    }

    let output = command
        .output()
        .with_context(|| format!("failed to start agent program '{}'", task.agent.program))?;

    Ok(AgentResult {
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn adds_ignored_test(diff: &str) -> bool {
    diff.lines()
        .any(|line| line.starts_with('+') && !line.starts_with("+++") && line.contains("#[ignore]"))
}

fn compact_failure(primary: &str, fallback: &str) -> String {
    let value = if primary.trim().is_empty() {
        fallback
    } else {
        primary
    };
    const LIMIT: usize = 4000;
    let trimmed = value.trim();
    if trimmed.chars().count() <= LIMIT {
        trimmed.to_owned()
    } else {
        format!("{}…", trimmed.chars().take(LIMIT).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_new_ignore_attribute() {
        assert!(adds_ignored_test("+    #[ignore]\n+    fn regression() {}"));
        assert!(!adds_ignored_test(
            "-    #[ignore]\n+    fn regression() {}"
        ));
    }
}
