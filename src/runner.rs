use std::{collections::BTreeSet, path::PathBuf, process::Command};

use anyhow::{bail, Context, Result};

use crate::{
    burncloud::BurncloudRepo,
    checks::{plan_checks, run_check},
    config::TaskSpec,
    git::GitRepo,
    invariants,
    policy::ScopePolicy,
    risk, route,
    trajectory::{new_run_id, Event, FailureClass, TrajectoryWriter},
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
    let mut reviewed_risks = BTreeSet::new();

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
            let detail = format!(
                "agent changed git HEAD from {} to {}; burncloud-harness forbids commits/history changes",
                baseline_head, current_head
            );
            record_failure(
                &mut trajectory,
                attempt,
                FailureClass::GitHistory,
                &detail,
            )?;
            trajectory.record(Event::RunFinished {
                success: false,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            bail!("{detail}");
        }

        let changed_paths = git.changed_paths()?;
        let report = scope.evaluate(&changed_paths);
        trajectory.record(Event::ScopeEvaluated {
            attempt,
            changed_paths: &changed_paths,
            violations: &report.violations,
        })?;

        if !report.is_ok() {
            let detail = format!(
                "scope violation; unauthorized changes: {}",
                report.violations.join(", ")
            );
            record_failure(
                &mut trajectory,
                attempt,
                FailureClass::ScopeViolation,
                &detail,
            )?;
            trajectory.record(Event::RunFinished {
                success: false,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            bail!("{detail}");
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
                record_retry(
                    &mut trajectory,
                    attempt,
                    FailureClass::InvariantExpansion,
                    &feedback,
                )?;
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
            record_retry(
                &mut trajectory,
                attempt,
                FailureClass::AgentCommand,
                &feedback,
            )?;
            previous_feedback = Some(feedback);
            continue;
        }

        if changed_paths.is_empty() {
            let feedback = "No repository changes were produced. Re-check the goal and either make the smallest in-scope change or clearly report why no change is required.".to_owned();
            record_retry(
                &mut trajectory,
                attempt,
                FailureClass::NoChange,
                &feedback,
            )?;
            previous_feedback = Some(feedback);
            continue;
        }

        let diff = git.diff()?;
        let risk_report = risk::inspect(&diff);
        let risk_summaries = risk_report.summaries();
        trajectory.record(Event::RiskAssessed {
            attempt,
            findings: &risk_summaries,
        })?;

        let blockers = risk_report.blockers();
        if !blockers.is_empty() {
            let feedback = format!(
                "BurnCloud final-diff risk gate found blocking changes. Remove or restore these before completion:\n{}",
                blockers
                    .iter()
                    .map(|finding| finding.summary())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            record_retry(
                &mut trajectory,
                attempt,
                FailureClass::RiskBlock,
                &feedback,
            )?;
            previous_feedback = Some(feedback);
            continue;
        }

        let unreviewed = risk_report.unreviewed(&reviewed_risks);
        if !unreviewed.is_empty() {
            for finding in &unreviewed {
                reviewed_risks.insert(finding.fingerprint());
            }
            let feedback = format!(
                "BurnCloud final-diff risk gate found changes that require one explicit review pass. Verify each item against the task contract and relevant invariants; fix it if accidental, or keep it only if intentional and explain why in REPORT:\n{}",
                unreviewed
                    .iter()
                    .map(|finding| finding.summary())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            record_retry(
                &mut trajectory,
                attempt,
                FailureClass::RiskReview,
                &feedback,
            )?;
            previous_feedback = Some(feedback);
            continue;
        }

        let active_invariant_ids = active_invariants.ids();
        let checks = plan_checks(&changed_paths, &active_invariant_ids, &task.extra_checks);
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
        record_retry(
            &mut trajectory,
            attempt,
            FailureClass::Verification,
            &feedback,
        )?;
        previous_feedback = Some(feedback);
    }

    let changed_paths = git.changed_paths()?;
    let detail = format!(
        "task did not pass burncloud-harness after {} attempts",
        task.max_loops
    );
    record_failure(
        &mut trajectory,
        task.max_loops,
        FailureClass::MaxLoops,
        &detail,
    )?;
    trajectory.record(Event::RunFinished {
        success: false,
        attempts: task.max_loops,
        changed_paths: &changed_paths,
    })?;
    bail!("{detail}; trajectory: {}", trajectory.path().display())
}

fn record_retry(
    trajectory: &mut TrajectoryWriter,
    attempt: u32,
    class: FailureClass,
    feedback: &str,
) -> Result<()> {
    record_failure(trajectory, attempt, class, feedback)?;
    trajectory.record(Event::AttemptFailed { attempt, feedback })?;
    Ok(())
}

fn record_failure(
    trajectory: &mut TrajectoryWriter,
    attempt: u32,
    class: FailureClass,
    detail: &str,
) -> Result<()> {
    trajectory.record(Event::FailureRecorded {
        attempt,
        class,
        detail,
    })?;
    Ok(())
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
