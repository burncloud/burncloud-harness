use std::{collections::BTreeSet, path::PathBuf, process::Command};

use anyhow::{bail, Context, Result};

use crate::{
    burncloud::BurncloudRepo,
    checks::{plan_checks, run_check},
    config::TaskSpec,
    git::GitRepo,
    invariants,
    observer::{NoopObserver, RunEvent, RunObserver, RunPhase},
    policy::ScopePolicy,
    risk, route,
    trajectory::{
        load_resume_provenances, new_run_id, Event, FailureClass, ResumeProvenance,
        TrajectoryWriter,
    },
};

pub struct RunSummary {
    pub run_id: String,
    pub attempts: u32,
    pub changed_paths: Vec<String>,
    pub trajectory_path: PathBuf,
}

pub fn run(task: TaskSpec) -> Result<RunSummary> {
    let mut observer = NoopObserver;
    run_with_observer_mode(task, &mut observer, false, true, false)
}

pub fn run_ui_migration(task: TaskSpec) -> Result<RunSummary> {
    let mut observer = NoopObserver;
    run_with_observer_mode(task, &mut observer, false, true, true)
}

pub fn resume(task: TaskSpec) -> Result<RunSummary> {
    let mut observer = NoopObserver;
    run_with_observer_mode(task, &mut observer, true, true, false)
}

pub fn resume_ui_migration(task: TaskSpec) -> Result<RunSummary> {
    let mut observer = NoopObserver;
    run_with_observer_mode(task, &mut observer, true, true, true)
}

pub fn verify_existing(task: TaskSpec) -> Result<RunSummary> {
    let mut observer = NoopObserver;
    run_with_observer_mode(task, &mut observer, true, false, false)
}

pub fn run_with_observer<O: RunObserver + ?Sized>(
    task: TaskSpec,
    observer: &mut O,
) -> Result<RunSummary> {
    run_with_observer_mode(task, observer, false, true, false)
}

fn run_with_observer_mode<O: RunObserver + ?Sized>(
    task: TaskSpec,
    observer: &mut O,
    resume_existing_changes: bool,
    execute_agent: bool,
    strict_visual: bool,
) -> Result<RunSummary> {
    let workspace = PathBuf::from(task.workspace.as_str())
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace {}", task.workspace))?;
    let burncloud = BurncloudRepo::open(workspace.as_path())?;
    let git = GitRepo::new(burncloud.root());
    git.ensure_repository()?;
    let scope = ScopePolicy::compile(&task.scope)?;
    let state_dir = git.harness_state_dir()?;
    let current_head = git.head_sha()?;

    let resume_provenance = if resume_existing_changes {
        Some(select_resume_provenance(
            &task,
            &git,
            &scope,
            &state_dir,
            &current_head,
        )?)
    } else {
        git.ensure_clean()?;
        None
    };
    let resumed_paths = resume_provenance
        .as_ref()
        .map(|provenance| provenance.changed_paths.clone())
        .unwrap_or_default();

    let routes = route::resolve(burncloud.root(), &task.goal, task.area)?;
    let mut active_invariants =
        invariants::resolve(burncloud.root(), task.area, &task.goal, &routes)?;
    let route_labels = routes.labels();
    let initial_invariant_ids = active_invariants.ids();

    let baseline_head = current_head;
    let run_id = new_run_id();
    let resumed_from = resume_provenance
        .as_ref()
        .map(|provenance| provenance.run_id.as_str());
    let mut trajectory = TrajectoryWriter::create(&state_dir, &run_id)?;
    trajectory.record(Event::RunStarted {
        run_id: &run_id,
        task: &task.name,
        goal: &task.goal,
        area: task.area.as_str(),
        max_loops: task.max_loops,
        baseline_head: &baseline_head,
        allowed: &task.scope.allowed,
        avoid: &task.scope.avoid,
        context_files: &task.context_files,
        agent_program: &task.agent.program,
        agent_args: &task.agent.args,
        agent_append_prompt: task.agent.append_prompt,
        resumed_from,
    })?;
    trajectory.record(Event::TaskRouted {
        routes: &route_labels,
    })?;
    trajectory.record(Event::InvariantsSelected {
        invariants: &initial_invariant_ids,
    })?;
    observer.on_event(RunEvent::Prepared {
        task: task.name.clone(),
        goal: task.goal.clone(),
        area: task.area.as_str().to_owned(),
        max_loops: task.max_loops,
        allowed: task.scope.allowed.clone(),
        avoid: task.scope.avoid.clone(),
        routes: route_labels.clone(),
        invariants: initial_invariant_ids.clone(),
    })?;

    let mut previous_feedback = resume_provenance.as_ref().map(|provenance| {
        format!(
            "This run is resuming interrupted Harness run {}. The current Git HEAD, task contract, scope, agent configuration, changed paths, and exact diff fingerprint match the last recorded checkpoint. Inspect and continue or correct these paths; do not assume they are complete:\n- {}",
            provenance.run_id,
            resumed_paths.join("\n- ")
        )
    });
    let mut reviewed_risks = BTreeSet::new();
    let attempt_limit = if execute_agent { task.max_loops } else { 1 };

    for attempt in 1..=attempt_limit {
        trajectory.record(Event::AttemptStarted { attempt })?;
        record_phase_start(&mut trajectory, attempt, RunPhase::Agent)?;
        observer.on_event(RunEvent::Phase {
            attempt,
            phase: RunPhase::Agent,
            detail: if execute_agent {
                "Coding agent is executing inside the declared BurnCloud scope".into()
            } else {
                "Operator requested deterministic verification of provenance-matched resumed changes"
                    .into()
            },
        })?;

        let agent_result = if execute_agent {
            let prompt = burncloud.control_prompt(
                &task,
                &routes,
                &active_invariants,
                attempt,
                previous_feedback.as_deref(),
            );
            run_agent(
                &workspace,
                &task,
                &prompt,
                &state_dir,
                &run_id,
                attempt,
                strict_visual,
            )?
        } else {
            AgentResult {
                success: true,
                timed_out: false,
                exit_code: Some(0),
                stdout: "No agent command executed; verifying provenance-matched existing changes."
                    .to_owned(),
                stderr: String::new(),
            }
        };
        trajectory.record(Event::AgentFinished {
            attempt,
            success: agent_result.success,
            exit_code: agent_result.exit_code,
            stdout: &agent_result.stdout,
            stderr: &agent_result.stderr,
        })?;

        record_phase_start(&mut trajectory, attempt, RunPhase::Scope)?;
        observer.on_event(RunEvent::Phase {
            attempt,
            phase: RunPhase::Scope,
            detail: "Reading the real Git diff; agent claims do not define the boundary".into(),
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
                observer,
                attempt,
                FailureClass::GitHistory,
                &detail,
            )?;
            trajectory.record(Event::RunFinished {
                success: false,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            observer.on_event(RunEvent::Finished {
                success: false,
                attempts: attempt,
                changed_paths,
                trajectory_path: trajectory.path().to_path_buf(),
            })?;
            bail!("{detail}");
        }

        let changed_paths = git.changed_paths()?;
        let report = scope.evaluate(&changed_paths);
        let diff_fingerprint = git.diff_fingerprint()?;
        trajectory.record(Event::ScopeEvaluated {
            attempt,
            changed_paths: &changed_paths,
            violations: &report.violations,
            diff_fingerprint: &diff_fingerprint,
        })?;
        observer.on_event(RunEvent::Paths {
            attempt,
            changed: changed_paths.clone(),
            violations: report.violations.clone(),
        })?;

        if !report.is_ok() {
            let detail = format!(
                "scope violation; unauthorized changes: {}",
                report.violations.join(", ")
            );
            record_failure(
                &mut trajectory,
                observer,
                attempt,
                FailureClass::ScopeViolation,
                &detail,
            )?;
            trajectory.record(Event::RunFinished {
                success: false,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            observer.on_event(RunEvent::Finished {
                success: false,
                attempts: attempt,
                changed_paths,
                trajectory_path: trajectory.path().to_path_buf(),
            })?;
            bail!("{detail}");
        }

        if !changed_paths.is_empty() {
            record_phase_start(&mut trajectory, attempt, RunPhase::Invariants)?;
            observer.on_event(RunEvent::Phase {
                attempt,
                phase: RunPhase::Invariants,
                detail: "Recomputing invariant impact from the actual changed paths".into(),
            })?;
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
            }
            observer.on_event(RunEvent::Invariants {
                attempt,
                active: active_invariants.ids(),
                newly_required: newly_required_ids.clone(),
            })?;

            if !newly_required_ids.is_empty() {
                let mut feedback = format!(
                    "The actual BurnCloud diff expanded invariant impact beyond the pre-change contract. Before completion, re-read and verify these newly required invariants against current source: {}.\nImpact evidence:\n{}\nDo not widen scope unless source evidence requires NEED_SCOPE_EXPANSION.",
                    newly_required_ids.join(", "),
                    impact.reasons.join("\n")
                );
                if !agent_result.success {
                    feedback.push_str("\n");
                    feedback.push_str(&agent_failure_feedback(&task, &agent_result));
                }
                record_retry(
                    &mut trajectory,
                    observer,
                    attempt,
                    FailureClass::InvariantExpansion,
                    &feedback,
                )?;
                previous_feedback = Some(feedback);
                continue;
            }
        }

        if !agent_result.success {
            let feedback = agent_failure_feedback(&task, &agent_result);
            record_retry(
                &mut trajectory,
                observer,
                attempt,
                FailureClass::AgentCommand,
                &feedback,
            )?;
            previous_feedback = Some(feedback);
            continue;
        }

        if changed_paths.is_empty() {
            let agent_report = compact_failure(&agent_result.stdout, &agent_result.stderr);
            let feedback = format!(
                "No repository changes were produced. Re-check the goal and either make the smallest in-scope change or clearly report why no change is required.\nPrevious agent report:\n{agent_report}"
            );
            record_retry(
                &mut trajectory,
                observer,
                attempt,
                FailureClass::NoChange,
                &feedback,
            )?;
            previous_feedback = Some(feedback);
            continue;
        }

        record_phase_start(&mut trajectory, attempt, RunPhase::Risk)?;
        observer.on_event(RunEvent::Phase {
            attempt,
            phase: RunPhase::Risk,
            detail: "Scanning the final diff for deterministic BurnCloud risk signals".into(),
        })?;
        let diff = git.diff()?;
        let risk_report = risk::inspect(&diff);
        let risk_summaries = risk_report.summaries();
        trajectory.record(Event::RiskAssessed {
            attempt,
            findings: &risk_summaries,
        })?;
        observer.on_event(RunEvent::Risks {
            attempt,
            findings: risk_summaries,
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
                observer,
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
                observer,
                attempt,
                FailureClass::RiskReview,
                &feedback,
            )?;
            previous_feedback = Some(feedback);
            continue;
        }

        record_phase_start(&mut trajectory, attempt, RunPhase::Verify)?;
        observer.on_event(RunEvent::Phase {
            attempt,
            phase: RunPhase::Verify,
            detail: "Running mandatory checks selected from changed paths and active invariants"
                .into(),
        })?;
        let active_invariant_ids = active_invariants.ids();
        let checks = plan_checks(&changed_paths, &active_invariant_ids, &task.extra_checks);
        let mut failed = Vec::new();

        for check in checks {
            trajectory.record(Event::CheckStarted {
                attempt,
                name: &check.name,
            })?;
            observer.on_event(RunEvent::Check {
                attempt,
                name: check.name.clone(),
                reason: check.reason.clone(),
                success: None,
            })?;
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
            observer.on_event(RunEvent::Check {
                attempt,
                name: result.name.clone(),
                reason: result.reason.clone(),
                success: Some(result.success),
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

        if strict_visual {
            const CHECK_NAME: &str = "ui-pixel-parity";
            const CHECK_COMMAND: &str = "builtin:strict-ui-pixel-parity";
            const CHECK_REASON: &str =
                "UI migration daemon requires deterministic reference pixel parity";
            trajectory.record(Event::CheckStarted {
                attempt,
                name: CHECK_NAME,
            })?;
            observer.on_event(RunEvent::Check {
                attempt,
                name: CHECK_NAME.to_owned(),
                reason: CHECK_REASON.to_owned(),
                success: None,
            })?;

            let visual_target = workspace
                .join("target")
                .join("burncloud-harness-visual-build");
            let visual_result = task
                .visual
                .as_ref()
                .context("UI migration tasks require a visual contract")
                .and_then(|visual| {
                    crate::ui_visual::run_migration(&workspace, visual, Some(&visual_target))
                });
            let (success, stdout, stderr) = match visual_result {
                Ok(output) => (true, output, String::new()),
                Err(error) => (false, String::new(), format!("{error:#}")),
            };
            trajectory.record(Event::CheckFinished {
                attempt,
                name: CHECK_NAME,
                command: CHECK_COMMAND,
                reason: CHECK_REASON,
                success,
                exit_code: Some(if success { 0 } else { 1 }),
                stdout: &stdout,
                stderr: &stderr,
            })?;
            observer.on_event(RunEvent::Check {
                attempt,
                name: CHECK_NAME.to_owned(),
                reason: CHECK_REASON.to_owned(),
                success: Some(success),
            })?;
            if !success {
                failed.push(format!(
                    "{CHECK_NAME} (`{CHECK_COMMAND}`): {}",
                    compact_failure(&stderr, &stdout)
                ));
            }
        }

        if failed.is_empty() {
            trajectory.record(Event::RunFinished {
                success: true,
                attempts: attempt,
                changed_paths: &changed_paths,
            })?;
            let trajectory_path = trajectory.path().to_path_buf();
            observer.on_event(RunEvent::Finished {
                success: true,
                attempts: attempt,
                changed_paths: changed_paths.clone(),
                trajectory_path: trajectory_path.clone(),
            })?;
            return Ok(RunSummary {
                run_id,
                attempts: attempt,
                changed_paths,
                trajectory_path,
            });
        }

        let verification_guidance = if strict_visual {
            "BurnCloud strict source-parity verification failed. Inspect reference/local/diff PNGs and report.json before editing. Compare the pinned source TSX and Dioxus RSX node-for-node, including wrapper hierarchy, exact text, icons, and responsive overflow. Prefer the source Tailwind class lists backed by crates/client/src/aether_source.css over approximating dimensions through buyer-* semantic CSS. Do not weaken pixel thresholds, mask regions, alter source behavior, widen scope, or change production data outside the capture fixture."
        } else {
            "BurnCloud mandatory verification failed. Fix the existing in-scope change; do not widen scope or weaken tests."
        };
        let feedback = format!("{verification_guidance}\n{}", failed.join("\n"));
        record_retry(
            &mut trajectory,
            observer,
            attempt,
            FailureClass::Verification,
            &feedback,
        )?;
        previous_feedback = Some(feedback);
    }

    let changed_paths = git.changed_paths()?;
    let detail = format!(
        "task did not pass burncloud-harness after {} attempts",
        attempt_limit
    );
    record_failure(
        &mut trajectory,
        observer,
        attempt_limit,
        FailureClass::MaxLoops,
        &detail,
    )?;
    trajectory.record(Event::RunFinished {
        success: false,
        attempts: attempt_limit,
        changed_paths: &changed_paths,
    })?;
    observer.on_event(RunEvent::Finished {
        success: false,
        attempts: attempt_limit,
        changed_paths,
        trajectory_path: trajectory.path().to_path_buf(),
    })?;
    bail!("{detail}; trajectory: {}", trajectory.path().display())
}

fn record_phase_start(
    trajectory: &mut TrajectoryWriter,
    attempt: u32,
    phase: RunPhase,
) -> Result<()> {
    trajectory.record(Event::PhaseStarted {
        attempt,
        phase: phase.as_str(),
    })
}

fn select_resume_provenance(
    task: &TaskSpec,
    git: &GitRepo,
    scope: &ScopePolicy,
    state_dir: &std::path::Path,
    current_head: &str,
) -> Result<ResumeProvenance> {
    let changed_paths = git.changed_paths()?;
    if changed_paths.is_empty() {
        bail!("--resume requires existing BurnCloud worktree changes");
    }

    let report = scope.evaluate(&changed_paths);
    if !report.is_ok() {
        bail!(
            "cannot resume changes outside the task scope: {}",
            report.violations.join(", ")
        );
    }

    let diff_fingerprint = git.diff_fingerprint()?;
    let candidates = load_resume_provenances(state_dir)?;
    let matching = candidates.into_iter().rev().find(|candidate| {
        candidate.task == task.name
            && candidate.goal == task.goal
            && candidate.area == task.area.as_str()
            && candidate.max_loops == task.max_loops
            && candidate.baseline_head == current_head
            && candidate.allowed == task.scope.allowed
            && candidate.avoid == task.scope.avoid
            && candidate.context_files == task.context_files
            && candidate.agent_program == task.agent.program
            && candidate.agent_args == task.agent.args
            && candidate.agent_append_prompt == task.agent.append_prompt
            && candidate.changed_paths == changed_paths
            && candidate.diff_fingerprint == diff_fingerprint
    });

    matching.with_context(|| {
        format!(
            "cannot prove that the current dirty worktree belongs to an interrupted Harness run. --resume now fails closed unless a recorded checkpoint matches HEAD {}, the full task contract, changed paths, and diff fingerprint {}",
            current_head, diff_fingerprint
        )
    })
}

fn record_retry<O: RunObserver + ?Sized>(
    trajectory: &mut TrajectoryWriter,
    observer: &mut O,
    attempt: u32,
    class: FailureClass,
    feedback: &str,
) -> Result<()> {
    record_failure(trajectory, observer, attempt, class, feedback)?;
    trajectory.record(Event::AttemptFailed { attempt, feedback })?;
    Ok(())
}

fn record_failure<O: RunObserver + ?Sized>(
    trajectory: &mut TrajectoryWriter,
    observer: &mut O,
    attempt: u32,
    class: FailureClass,
    detail: &str,
) -> Result<()> {
    trajectory.record(Event::FailureRecorded {
        attempt,
        class,
        detail,
    })?;
    observer.on_event(RunEvent::Failure {
        attempt,
        class: class.as_str().to_owned(),
        detail: detail.to_owned(),
    })?;
    Ok(())
}

struct AgentResult {
    success: bool,
    timed_out: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

enum AgentLine {
    Stdout(String),
    Stderr(String),
}

fn dsh_headless_command() -> Result<(Command, String)> {
    #[cfg(windows)]
    {
        let app_data = std::env::var_os("APPDATA").context("APPDATA is required to locate DSH")?;
        let entry = PathBuf::from(app_data)
            .join("npm")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib")
            .join("bin.js");
        if !entry.is_file() {
            bail!("DSH Node entry was not found at {}", entry.display());
        }
        let mut command = Command::new("node");
        command.arg(&entry);
        return Ok((command, format!("node {}", entry.display())));
    }
    #[cfg(not(windows))]
    {
        Ok((Command::new("dsh"), "dsh".to_owned()))
    }
}

fn run_agent(
    workspace: &std::path::Path,
    task: &TaskSpec,
    prompt: &str,
    state_dir: &std::path::Path,
    run_id: &str,
    attempt: u32,
    strict_visual: bool,
) -> Result<AgentResult> {
    let (mut command, program_label) = if strict_visual {
        let (mut command, label) = dsh_headless_command()?;
        command.args(["--profile", "headless"]);
        (command, label)
    } else {
        let mut command = Command::new(&task.agent.program);
        command.args(&task.agent.args);
        (command, task.agent.program.clone())
    };
    command
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if strict_visual {
        let prompt_path = state_dir
            .join("runs")
            .join(format!("{run_id}-attempt-{attempt}-agent-prompt.md"));
        std::fs::write(&prompt_path, prompt).with_context(|| {
            format!(
                "failed to persist DSH agent prompt at {}",
                prompt_path.display()
            )
        })?;
        command.arg(format!(
            "Read the complete task prompt from this exact UTF-8 file and execute it: {}",
            prompt_path.display()
        ));
    } else if task.agent.append_prompt {
        command.arg(prompt);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start agent program '{program_label}'"))?;
    let stdout = child
        .stdout
        .take()
        .context("agent stdout pipe was not available")?;
    let stderr = child
        .stderr
        .take()
        .context("agent stderr pipe was not available")?;

    let events = crate::event_writer::RunEventWriter::open(state_dir, run_id)?;
    let soft_limit_secs = task.agent.soft_timeout_secs();
    let hard_limit_secs = task.agent.hard_timeout_secs();
    let idle_warning_secs = task.agent.idle_timeout_secs();
    events.append(&crate::events::HarnessEvent::AgentTimeBudgetConfigured {
        attempt,
        soft_limit_secs,
        hard_limit_secs,
        idle_warning_secs,
    })?;

    let (sender, receiver) = std::sync::mpsc::channel();
    let stdout_sender = sender.clone();
    let stderr_sender = sender.clone();
    drop(sender);

    let stdout_thread = std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if stdout_sender.send(AgentLine::Stdout(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = stdout_sender.send(AgentLine::Stderr(format!(
                        "failed reading agent stdout: {error}"
                    )));
                    break;
                }
            }
        }
    });
    let stderr_thread = std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if stderr_sender.send(AgentLine::Stderr(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = stderr_sender.send(AgentLine::Stderr(format!(
                        "failed reading agent stderr: {error}"
                    )));
                    break;
                }
            }
        }
    });

    let started = std::time::Instant::now();
    let mut last_output = std::time::Instant::now();
    let mut next_heartbeat_secs = 5_u64;
    let mut soft_limit_emitted = false;
    let mut idle_warning_emitted = false;
    let mut hard_timed_out = false;
    let mut stdout_buffer = String::new();
    let mut stderr_buffer = String::new();

    let status = loop {
        match receiver.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(line) => {
                last_output = std::time::Instant::now();
                idle_warning_emitted = false;
                record_agent_line(
                    &events,
                    attempt,
                    line,
                    &mut stdout_buffer,
                    &mut stderr_buffer,
                )?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }

        let elapsed_secs = started.elapsed().as_secs();
        while elapsed_secs >= next_heartbeat_secs {
            events.append(&crate::events::HarnessEvent::AgentHeartbeat {
                attempt,
                elapsed_secs: next_heartbeat_secs,
            })?;
            next_heartbeat_secs += 5;
        }

        if let Some(soft_limit_secs) = soft_limit_secs {
            if !soft_limit_emitted && elapsed_secs >= soft_limit_secs {
                events.append(&crate::events::HarnessEvent::AgentSoftLimitReached {
                    attempt,
                    elapsed_secs,
                    soft_limit_secs,
                })?;
                tracing::warn!(
                    attempt,
                    phase = "AGENT",
                    elapsed_secs,
                    soft_limit_secs,
                    "agent reached soft convergence target; finish the smallest coherent checkpoint"
                );
                soft_limit_emitted = true;
            }
        }

        if let Some(idle_warning_secs) = idle_warning_secs {
            let idle_secs = last_output.elapsed().as_secs();
            if !idle_warning_emitted && idle_secs >= idle_warning_secs {
                events.append(&crate::events::HarnessEvent::AgentIdleWarning {
                    attempt,
                    elapsed_secs,
                    idle_secs,
                    idle_warning_secs,
                })?;
                tracing::warn!(
                    attempt,
                    phase = "AGENT",
                    elapsed_secs,
                    idle_secs,
                    idle_warning_secs,
                    "agent has produced no output beyond the idle warning threshold"
                );
                idle_warning_emitted = true;
            }
        }

        if let Some(status) = child.try_wait()? {
            break status;
        }

        if let Some(hard_limit_secs) = hard_limit_secs {
            if elapsed_secs >= hard_limit_secs {
                events.append(&crate::events::HarnessEvent::AgentHardTimeout {
                    attempt,
                    elapsed_secs,
                    hard_limit_secs,
                })?;
                tracing::warn!(
                    attempt,
                    phase = "AGENT",
                    elapsed_secs,
                    hard_limit_secs,
                    "agent reached hard time budget; preserving worktree and ending this attempt"
                );
                terminate_agent_process_tree(&mut child)?;
                hard_timed_out = true;
                break child.wait()?;
            }
        }
    };

    if hard_timed_out {
        // Descendants can inherit these pipes. Do not let an already-enforced hard timeout
        // hang forever while waiting for an unrelated inherited handle to close.
        drop(stdout_thread);
        drop(stderr_thread);
    } else {
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
    }
    while let Ok(line) = receiver.try_recv() {
        record_agent_line(
            &events,
            attempt,
            line,
            &mut stdout_buffer,
            &mut stderr_buffer,
        )?;
    }

    Ok(AgentResult {
        success: status.success() && !hard_timed_out,
        timed_out: hard_timed_out,
        exit_code: status.code(),
        stdout: stdout_buffer,
        stderr: stderr_buffer,
    })
}

fn terminate_agent_process_tree(child: &mut std::process::Child) -> Result<()> {
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .status()
            .context("failed to invoke taskkill for timed-out agent")?;
        if status.success() {
            return Ok(());
        }
    }

    child
        .kill()
        .context("failed to terminate agent after hard time budget")
}

fn agent_failure_feedback(task: &TaskSpec, result: &AgentResult) -> String {
    let evidence = compact_failure(&result.stderr, &result.stdout);
    if result.timed_out {
        let hard = task
            .agent
            .hard_timeout_minutes
            .map(|minutes| format!("{minutes} minutes"))
            .unwrap_or_else(|| "the configured hard limit".to_owned());
        format!(
            "Agent reached the hard time budget ({hard}). The current worktree is preserved. Continue from the existing in-scope diff on the next attempt; do not restart discovery or discard correct work. Prioritize closing the smallest remaining gap and verification.\nLast agent evidence:\n{evidence}"
        )
    } else {
        format!(
            "Agent command failed with exit code {:?}.\n{}",
            result.exit_code, evidence
        )
    }
}

fn record_agent_line(
    events: &crate::event_writer::RunEventWriter,
    attempt: u32,
    line: AgentLine,
    stdout: &mut String,
    stderr: &mut String,
) -> Result<()> {
    let (stream, value) = match line {
        AgentLine::Stdout(value) => {
            stdout.push_str(&value);
            stdout.push('\n');
            ("stdout", value)
        }
        AgentLine::Stderr(value) => {
            stderr.push_str(&value);
            stderr.push('\n');
            ("stderr", value)
        }
    };

    if value.trim().is_empty() {
        return Ok(());
    }

    let live_line = compact_agent_line(&value, 1_000);
    events.append(&crate::events::HarnessEvent::AgentOutput {
        attempt,
        stream: stream.to_owned(),
        line: live_line.clone(),
    })?;

    if stream == "stderr" {
        tracing::warn!(attempt, phase = "AGENT", stream, line = %live_line, "agent output");
    } else {
        tracing::info!(attempt, phase = "AGENT", stream, line = %live_line, "agent output");
    }
    Ok(())
}

fn compact_agent_line(value: &str, limit: usize) -> String {
    let normalized = value.trim().replace('\t', " ");
    if normalized.chars().count() <= limit {
        normalized
    } else {
        format!("{}…", normalized.chars().take(limit).collect::<String>())
    }
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
