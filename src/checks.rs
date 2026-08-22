use std::{collections::BTreeSet, path::Path, process::Command};

use anyhow::{Context, Result};

use crate::config::CheckSpec;

#[derive(Debug, Clone)]
pub struct PlannedCheck {
    pub name: String,
    pub command: String,
    pub reason: String,
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

pub fn plan_checks(
    changed_paths: &[String],
    invariant_ids: &[String],
    extra_checks: &[CheckSpec],
) -> Vec<PlannedCheck> {
    let mut checks = Vec::new();
    let mut seen = BTreeSet::new();

    if changed_paths.iter().any(|path| path.ends_with(".rs")) {
        push_unique(
            &mut checks,
            &mut seen,
            "format",
            "cargo fmt --check",
            "BurnCloud TEST_MATRIX default verification ladder",
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
                "affected BurnCloud package must compile",
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
    let output = shell_command(&check.command)
        .current_dir(workspace)
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
        assert_eq!(names, vec!["format", "router-check", "billing-invariants"]);
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
}
