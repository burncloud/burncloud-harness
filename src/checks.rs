use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    process::Command,
    sync::{Mutex, OnceLock},
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

static FAILURE_OBSERVATIONS: OnceLock<Mutex<HashMap<String, FailureObservation>>> = OnceLock::new();

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
            "rustfmt".to_owned(),
            "--edition".to_owned(),
            "2021".to_owned(),
            "--check".to_owned(),
        ];
        argv.extend(changed_rust_paths);
        push_unique_argv(
            &mut checks,
            &mut seen,
            "rustfmt",
            argv,
            "changed Rust files must satisfy rustfmt without inheriting unrelated baseline formatting failures",
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
            "client-web-check",
            "cargo check -p burncloud-client --no-default-features --features web",
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
    let mut command = if let Some(argv) = &check.argv {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command
    } else {
        shell_command(&check.command)
    };
    let output = command
        .current_dir(workspace)
        .output()
        .with_context(|| format!("failed to execute check '{}'", check.name))?;

    let success = output.status.success();
    let exit_code = output.status.code();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if success {
        clear_failure_observation(workspace, &check.name);
    } else {
        let signature = failure_signature(&check.name, exit_code, &stderr, &stdout);
        let consecutive = observe_failure(workspace, &check.name, &signature);
        if consecutive >= REPEATED_FAILURE_LIMIT {
            let evidence = compact_diagnostic(&stderr, &stdout);
            bail!(
                "REPEATED_UNCHANGED_FAILURE: mandatory check '{}' produced the same failure signature {} for {} consecutive verification passes. BurnCloud Harness stopped before spending another Agent loop. The current worktree is preserved. Treat this as a likely baseline/dependency/environment blocker or an unchanged root cause; diagnose the blocker, then continue with --resume.\n{}",
                check.name,
                signature,
                consecutive,
                evidence
            );
        }
    }

    Ok(CheckResult {
        name: check.name.clone(),
        command: check.command.clone(),
        reason: check.reason.clone(),
        success,
        exit_code,
        stdout,
        stderr,
    })
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
        assert_eq!(names, vec!["rustfmt", "router-check", "billing-invariants"]);
    }

    #[test]
    fn rustfmt_check_is_scoped_to_changed_rust_files_without_a_shell() {
        let checks = plan_checks(
            &[
                "crates/client/src/critical_pages/buyer_overview.rs".into(),
                "crates/client/src/product_ui.css".into(),
            ],
            &[],
            &[],
        );

        assert_eq!(checks[0].name, "rustfmt");
        assert_eq!(
            checks[0].argv.as_ref().unwrap(),
            &vec![
                "rustfmt".to_owned(),
                "--edition".to_owned(),
                "2021".to_owned(),
                "--check".to_owned(),
                "crates/client/src/critical_pages/buyer_overview.rs".to_owned(),
            ]
        );
        assert_eq!(checks[1].name, "client-web-check");
        assert_eq!(
            checks[1].command,
            "cargo check -p burncloud-client --no-default-features --features web"
        );
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
}
