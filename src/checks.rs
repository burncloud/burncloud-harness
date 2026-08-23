use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};

use crate::config::CheckSpec;

const REPEATED_FAILURE_LIMIT: u32 = 2;

#[derive(Debug, Clone)]
pub struct PlannedCheck {
    pub name: String,
    pub command: String,
    pub reason: String,
    argv: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct CheckResult {
    pub name: String,
    pub command: String,
    pub reason: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
struct FailureObservation {
    signature: String,
    consecutive: u32,
}

#[derive(Debug, Clone)]
struct BaselineObservation {
    head: String,
    failure_signature: Option<String>,
}

static FAILURE_OBSERVATIONS: OnceLock<Mutex<HashMap<String, FailureObservation>>> = OnceLock::new();
static BASELINE_OBSERVATIONS: OnceLock<Mutex<HashMap<String, BaselineObservation>>> =
    OnceLock::new();

pub fn plan_checks(
    changed_paths: &[String],
    invariant_ids: &[String],
    extra_checks: &[CheckSpec],
) -> Vec<PlannedCheck> {
    let mut checks = Vec::new();
    let mut seen = BTreeSet::new();

    let changed_rust_paths = changed_paths
        .iter()
        .filter(|path| path.ends_with(".rs"))
        .cloned()
        .collect::<Vec<_>>();
    if !changed_rust_paths.is_empty() {
        let mut argv = vec![
            "git".to_owned(),
            "diff".to_owned(),
            "--check".to_owned(),
            "HEAD".to_owned(),
            "--".to_owned(),
        ];
        argv.extend(changed_rust_paths);
        push_unique_argv(
            &mut checks,
            &mut seen,
            "changed-rust-lines",
            argv,
            "changed Rust lines must be free of whitespace errors without inheriting baseline formatting noise",
        );
    }

    let mappings = [
        (
            "crates/router/",
            "router-check",
            "cargo check -p burncloud-router",
        ),
        (
            "crates/server/",
            "server-check",
            "cargo check -p burncloud-server",
        ),
        (
            "crates/service/crates/billing/",
            "billing-service-check",
            "cargo check -p burncloud-service-billing",
        ),
        (
            "crates/database/crates/billing/",
            "billing-database-check",
            "cargo check -p burncloud-database-billing",
        ),
        (
            "crates/database/crates/router/",
            "router-database-check",
            "cargo check -p burncloud-database-router",
        ),
        (
            "crates/service/crates/router-log/",
            "router-log-service-check",
            "cargo check -p burncloud-service-router-log",
        ),
        (
            "crates/service/crates/channel/",
            "channel-service-check",
            "cargo check -p burncloud-service-channel",
        ),
        (
            "crates/database/crates/channel/",
            "channel-database-check",
            "cargo check -p burncloud-database-channel",
        ),
        (
            "crates/service/crates/user/",
            "user-service-check",
            "cargo check -p burncloud-service-user",
        ),
        (
            "crates/database/crates/user/",
            "user-database-check",
            "cargo check -p burncloud-database-user",
        ),
        (
            "crates/client/",
            "client-liveview-check",
            "cargo check -p burncloud-client --no-default-features --features liveview",
        ),
    ];

    for (prefix, name, command) in mappings {
        if changed_paths.iter().any(|path| path.starts_with(prefix)) {
            push_unique(
                &mut checks,
                &mut seen,
                name,
                command,
                "affected BurnCloud package must compile in the task-relevant feature mode",
            );
        }
    }

    if changed_paths
        .iter()
        .any(|path| path == "Cargo.toml" || path == "Cargo.lock")
        || has_invariant_family(invariant_ids, "INV-WORKSPACE-")
    {
        push_unique(
            &mut checks,
            &mut seen,
            "workspace-check",
            "cargo check --workspace",
            "BurnCloud workspace dependency invariant is in the actual impact set",
        );
    }

    if has_invariant_family(invariant_ids, "INV-RUNTIME-") {
        push_unique(
            &mut checks,
            &mut seen,
            "server-check",
            "cargo check -p burncloud-server",
            "BurnCloud runtime composition invariant is in the actual impact set",
        );
    }

    if has_invariant_family(invariant_ids, "INV-ROUTER-") {
        push_unique(
            &mut checks,
            &mut seen,
            "router-check",
            "cargo check -p burncloud-router",
            "BurnCloud router invariant is in the actual impact set",
        );
    }

    if has_invariant_family(invariant_ids, "INV-AUTH-")
        || has_invariant_family(invariant_ids, "INV-INTERNAL-")
    {
        push_unique(
            &mut checks,
            &mut seen,
            "security-invariants",
            "cargo test -p burncloud-server --test security_invariants",
            "BurnCloud auth/internal security invariant is in the actual impact set",
        );
    }

    if has_invariant_family(invariant_ids, "INV-BILLING-") {
        push_unique(
            &mut checks,
            &mut seen,
            "billing-invariants",
            "cargo test -p burncloud-router --test billing_invariants --test quota_tests",
            "BurnCloud billing/quota invariant is in the actual impact set",
        );
    }

    for extra in extra_checks {
        push_unique(
            &mut checks,
            &mut seen,
            &format!("extra:{}", extra.name),
            &extra.command,
            "task-defined extra verification",
        );
    }

    checks
}

pub fn run_check(workspace: &Path, check: &PlannedCheck) -> Result<CheckResult> {
    let result = execute_check(workspace, check, None)?;

    if result.success {
        clear_failure_observation(workspace, &check.name);
        return Ok(result);
    }

    let signature = failure_signature(
        &check.name,
        result.exit_code,
        &result.stderr,
        &result.stdout,
    );

    match baseline_observation(workspace, check) {
        Ok(baseline) if baseline.failure_signature.as_deref() == Some(signature.as_str()) => {
            clear_failure_observation(workspace, &check.name);
            let evidence = compact_diagnostic(&result.stderr, &result.stdout);
            bail!(
                "BASELINE_BLOCKER: mandatory check '{}' failed with signature {} in the current worktree and failed with the identical signature on clean HEAD {}. BurnCloud Harness attributes this failure to the repository baseline/dependency/environment rather than the current diff. The current worktree is preserved and no additional Agent loop will be spent trying to fix this unchanged baseline failure. Diagnose the blocker separately, then continue with --resume.\n{}",
                check.name,
                signature,
                baseline.head,
                evidence
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(
                check = %check.name,
                error = %error,
                "baseline attribution probe failed; falling back to repeated-failure protection"
            );
        }
    }

    let consecutive = observe_failure(workspace, &check.name, &signature);
    if consecutive >= REPEATED_FAILURE_LIMIT {
        let evidence = compact_diagnostic(&result.stderr, &result.stdout);
        bail!(
            "REPEATED_UNCHANGED_FAILURE: mandatory check '{}' produced the same failure signature {} for {} consecutive verification passes, and clean-HEAD attribution did not prove it is the same baseline failure. BurnCloud Harness stopped before spending another Agent loop. The current worktree is preserved. Diagnose the unchanged root cause, then continue with --resume.\n{}",
            check.name,
            signature,
            consecutive,
            evidence
        );
    }

    Ok(result)
}

fn execute_check(
    workspace: &Path,
    check: &PlannedCheck,
    shared_target_dir: Option<&Path>,
) -> Result<CheckResult> {
    let mut command = if let Some(argv) = &check.argv {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command
    } else {
        shell_command(&check.command)
    };
    command.current_dir(workspace);
    if let Some(target_dir) = shared_target_dir {
        command.env("CARGO_TARGET_DIR", target_dir);
    }

    let output = command
        .output()
        .with_context(|| format!("failed to execute check '{}'", check.name))?;

    Ok(CheckResult {
        name: check.name.clone(),
        command: check.command.clone(),
        reason: check.reason.clone(),
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn baseline_observation(workspace: &Path, check: &PlannedCheck) -> Result<BaselineObservation> {
    let head = git_stdout(workspace, &["rev-parse", "HEAD"])?;
    let cache_key = format!("{}::{head}::{}", workspace.display(), check.name);
    if let Some(observation) = baseline_cache_get(&cache_key) {
        return Ok(observation);
    }

    let baseline_dir = baseline_worktree_path();
    add_detached_worktree(workspace, &baseline_dir, &head)?;

    let shared_target_dir = workspace.join("target");
    let probe = execute_check(&baseline_dir, check, Some(&shared_target_dir));
    let cleanup = remove_detached_worktree(workspace, &baseline_dir);
    if let Err(error) = cleanup {
        tracing::warn!(
            path = %baseline_dir.display(),
            error = %error,
            "failed to remove temporary baseline worktree"
        );
    }

    let result = probe?;
    let failure_signature = if result.success {
        None
    } else {
        Some(failure_signature(
            &check.name,
            result.exit_code,
            &result.stderr,
            &result.stdout,
        ))
    };
    let observation = BaselineObservation {
        head,
        failure_signature,
    };
    baseline_cache_insert(cache_key, observation.clone());
    Ok(observation)
}

fn baseline_cache_get(key: &str) -> Option<BaselineObservation> {
    let observations = BASELINE_OBSERVATIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let observations = observations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    observations.get(key).cloned()
}

fn baseline_cache_insert(key: String, observation: BaselineObservation) {
    let observations = BASELINE_OBSERVATIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut observations = observations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    observations.insert(key, observation);
}

fn baseline_worktree_path() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "burncloud-harness-baseline-{}-{unique}",
        std::process::id()
    ))
}

fn add_detached_worktree(workspace: &Path, destination: &Path, head: &str) -> Result<()> {
    if destination.exists() {
        fs::remove_dir_all(destination).with_context(|| {
            format!(
                "failed to clear stale baseline worktree {}",
                destination.display()
            )
        })?;
    }
    let output = Command::new("git")
        .args(["worktree", "add", "--detach", "--force"])
        .arg(destination)
        .arg(head)
        .current_dir(workspace)
        .output()
        .context("failed to create temporary clean-HEAD worktree for baseline attribution")?;
    if !output.status.success() {
        bail!(
            "git worktree add failed for baseline attribution: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn remove_detached_worktree(workspace: &Path, destination: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(destination)
        .current_dir(workspace)
        .output()
        .context("failed to remove temporary baseline worktree")?;
    if !output.status.success() {
        bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if destination.exists() {
        fs::remove_dir_all(destination).with_context(|| {
            format!(
                "failed to remove baseline worktree directory {}",
                destination.display()
            )
        })?;
    }
    Ok(())
}

fn git_stdout(workspace: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn observe_failure(workspace: &Path, check_name: &str, signature: &str) -> u32 {
    let key = observation_key(workspace, check_name);
    let observations = FAILURE_OBSERVATIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut observations = observations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = observations.entry(key).or_insert(FailureObservation {
        signature: signature.to_owned(),
        consecutive: 0,
    });

    if entry.signature == signature {
        entry.consecutive += 1;
    } else {
        entry.signature = signature.to_owned();
        entry.consecutive = 1;
    }
    entry.consecutive
}

fn clear_failure_observation(workspace: &Path, check_name: &str) {
    let Some(observations) = FAILURE_OBSERVATIONS.get() else {
        return;
    };
    let mut observations = observations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    observations.remove(&observation_key(workspace, check_name));
}

fn observation_key(workspace: &Path, check_name: &str) -> String {
    format!("{}::{check_name}", workspace.display())
}

fn failure_signature(
    check_name: &str,
    exit_code: Option<i32>,
    stderr: &str,
    stdout: &str,
) -> String {
    let diagnostic = normalized_diagnostic(stderr, stdout);
    let value = format!("{check_name}|{exit_code:?}|{diagnostic}");
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn normalized_diagnostic(stderr: &str, stdout: &str) -> String {
    let value = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !line.starts_with("Checking ")
                && !line.starts_with("Compiling ")
                && !line.starts_with("Downloaded ")
                && !line.starts_with("Downloading ")
                && !line.starts_with("Finished ")
        })
        .take(80)
        .collect::<Vec<_>>()
        .join("\n")
        .replace('\\', "/")
}

fn compact_diagnostic(stderr: &str, stdout: &str) -> String {
    let value = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    const LIMIT: usize = 2_000;
    let trimmed = value.trim();
    if trimmed.chars().count() <= LIMIT {
        trimmed.to_owned()
    } else {
        format!("{}…", trimmed.chars().take(LIMIT).collect::<String>())
    }
}

fn has_invariant_family(invariant_ids: &[String], prefix: &str) -> bool {
    invariant_ids.iter().any(|id| id.starts_with(prefix))
}

fn push_unique(
    checks: &mut Vec<PlannedCheck>,
    seen: &mut BTreeSet<String>,
    name: &str,
    command: &str,
    reason: &str,
) {
    if seen.insert(name.to_owned()) {
        checks.push(PlannedCheck {
            name: name.to_owned(),
            command: command.to_owned(),
            reason: reason.to_owned(),
            argv: None,
        });
    }
}

fn push_unique_argv(
    checks: &mut Vec<PlannedCheck>,
    seen: &mut BTreeSet<String>,
    name: &str,
    argv: Vec<String>,
    reason: &str,
) {
    if seen.insert(name.to_owned()) {
        checks.push(PlannedCheck {
            name: name.to_owned(),
            command: argv.join(" "),
            reason: reason.to_owned(),
            argv: Some(argv),
        });
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.args(["-lc", command]);
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_change_gets_router_and_billing_gates_from_impact() {
        let checks = plan_checks(
            &["crates/router/src/lib.rs".into()],
            &["INV-ROUTER-001".into(), "INV-BILLING-001".into()],
            &[],
        );
        let names = checks
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["changed-rust-lines", "router-check", "billing-invariants"]
        );
    }

    #[test]
    fn ui_change_uses_changed_lines_and_liveview_compile_checks() {
        let checks = plan_checks(
            &[
                "crates/client/src/critical_pages/dashboard.rs".into(),
                "crates/client/src/product_ui.css".into(),
            ],
            &[],
            &[],
        );

        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "changed-rust-lines");
        assert_eq!(
            checks[0].argv.as_ref().unwrap(),
            &vec![
                "git".to_owned(),
                "diff".to_owned(),
                "--check".to_owned(),
                "HEAD".to_owned(),
                "--".to_owned(),
                "crates/client/src/critical_pages/dashboard.rs".to_owned(),
            ]
        );
        assert_eq!(checks[1].name, "client-liveview-check");
        assert_eq!(
            checks[1].command,
            "cargo check -p burncloud-client --no-default-features --features liveview"
        );
    }

    #[test]
    fn ui_change_without_visual_contract_remains_compile_checked() {
        let checks = plan_checks(&["crates/client/src/components.rs".into()], &[], &[]);
        let names = checks
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["changed-rust-lines", "client-liveview-check"]);
    }

    #[test]
    fn client_data_change_gets_only_liveview_compile_check() {
        let checks = plan_checks(&["crates/client/src/backend.rs".into()], &[], &[]);
        let names = checks
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["changed-rust-lines", "client-liveview-check"]);
    }

    #[test]
    fn auth_invariant_adds_security_suite_even_for_non_api_path() {
        let checks = plan_checks(
            &["docs/agent/INVARIANTS.md".into()],
            &["INV-AUTH-002".into()],
            &[],
        );
        assert_eq!(checks[0].name, "security-invariants");
    }

    #[test]
    fn docs_only_change_without_invariant_impact_does_not_invent_build_checks() {
        let checks = plan_checks(&["docs/agent/README.md".into()], &[], &[]);
        assert!(checks.is_empty());
    }

    #[test]
    fn failure_signature_ignores_cargo_progress_noise() {
        let first = failure_signature(
            "client-web-check",
            Some(101),
            "Checking dioxus-web v0.7.5\nerror[E0599]: no method named `location`\n  --> C:\\src\\history.rs:96:36",
            "",
        );
        let second = failure_signature(
            "client-web-check",
            Some(101),
            "Checking burncloud-client v0.1.0\nerror[E0599]: no method named `location`\n  --> C:\\src\\history.rs:96:36",
            "",
        );
        assert_eq!(first, second);
    }

    #[test]
    fn baseline_and_current_identical_diagnostics_have_identical_signature() {
        let current = failure_signature(
            "client-web-check",
            Some(101),
            "Checking burncloud-client v0.2.0\nerror[E0599]: no method named `location` found for struct `web_sys::Window`\n  --> C:\\Users\\huang\\.cargo\\registry\\dioxus-web-0.7.5\\src\\history.rs:96:36",
            "",
        );
        let baseline = failure_signature(
            "client-web-check",
            Some(101),
            "Checking dioxus-web v0.7.5\nerror[E0599]: no method named `location` found for struct `web_sys::Window`\n  --> C:\\Users\\huang\\.cargo\\registry\\dioxus-web-0.7.5\\src\\history.rs:96:36",
            "",
        );
        assert_eq!(current, baseline);
    }
}
