use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskSeverity {
    Block,
    Review,
}

impl RiskSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Review => "review",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskFinding {
    pub code: &'static str,
    pub severity: RiskSeverity,
    pub path: String,
    pub detail: String,
}

impl RiskFinding {
    pub fn fingerprint(&self) -> String {
        format!("{}:{}:{}", self.code, self.path, self.detail)
    }

    pub fn summary(&self) -> String {
        format!(
            "{} [{}] {}: {}",
            self.code,
            self.severity.as_str(),
            self.path,
            self.detail
        )
    }
}

#[derive(Debug, Clone)]
pub struct RiskReport {
    pub findings: Vec<RiskFinding>,
}

impl RiskReport {
    pub fn blockers(&self) -> Vec<&RiskFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == RiskSeverity::Block)
            .collect()
    }

    pub fn unreviewed<'a>(&'a self, reviewed: &BTreeSet<String>) -> Vec<&'a RiskFinding> {
        self.findings
            .iter()
            .filter(|finding| {
                finding.severity == RiskSeverity::Review
                    && !reviewed.contains(&finding.fingerprint())
            })
            .collect()
    }

    pub fn summaries(&self) -> Vec<String> {
        self.findings.iter().map(RiskFinding::summary).collect()
    }
}

pub fn inspect(diff: &str) -> RiskReport {
    let files = parse_diff(diff);
    let mut findings = Vec::new();

    for (path, file) in files {
        if file.added.iter().any(|line| line.contains("#[ignore]")) {
            findings.push(RiskFinding {
                code: "TEST_IGNORE_ADDED",
                severity: RiskSeverity::Block,
                path: path.clone(),
                detail: "new #[ignore] weakens executable verification".into(),
            });
        }

        if file
            .added
            .iter()
            .any(|line| line.contains("allow(clippy::unwrap_used)"))
        {
            findings.push(RiskFinding {
                code: "LINT_ESCAPE_ADDED",
                severity: RiskSeverity::Block,
                path: path.clone(),
                detail:
                    "new allow(clippy::unwrap_used) bypasses a BurnCloud workspace lint boundary"
                        .into(),
            });
        }

        if file.deleted_file && is_invariant_test(&path) {
            findings.push(RiskFinding {
                code: "INVARIANT_TEST_DELETED",
                severity: RiskSeverity::Block,
                path: path.clone(),
                detail: "a dedicated BurnCloud invariant test file was deleted".into(),
            });
        }

        for symbol in protected_symbols(&path) {
            if removed_symbol(&file, symbol) && !added_symbol(&file, symbol) {
                findings.push(RiskFinding {
                    code: "SECURITY_GUARD_REMOVED",
                    severity: RiskSeverity::Block,
                    path: path.clone(),
                    detail: format!(
                        "protected boundary symbol `{symbol}` was removed without replacement"
                    ),
                });
            }
        }

        if is_test_path(&path) {
            let removed_assertions = count_assertions(&file.removed);
            let added_assertions = count_assertions(&file.added);
            if removed_assertions > added_assertions {
                findings.push(RiskFinding {
                    code: "ASSERTION_WEAKENING",
                    severity: RiskSeverity::Review,
                    path: path.clone(),
                    detail: format!(
                        "removed {removed_assertions} assertion(s) but added only {added_assertions}; review whether the regression contract became weaker"
                    ),
                });
            }
        }

        if is_runtime_source(&path) && file.added.iter().any(|line| contains_todo_marker(line)) {
            findings.push(RiskFinding {
                code: "RUNTIME_TODO_ADDED",
                severity: RiskSeverity::Review,
                path: path.clone(),
                detail: "new TODO/FIXME marker was added to BurnCloud runtime source".into(),
            });
        }

        if is_sensitive_runtime_path(&path) {
            let removed_fail_closed = count_fail_closed_constructs(&file.removed);
            let added_fail_closed = count_fail_closed_constructs(&file.added);
            if removed_fail_closed > added_fail_closed {
                findings.push(RiskFinding {
                    code: "FAIL_CLOSED_LOGIC_REDUCED",
                    severity: RiskSeverity::Review,
                    path: path.clone(),
                    detail: format!(
                        "removed {removed_fail_closed} fail-closed/error construct(s) but added only {added_fail_closed}; verify the path still rejects unsafe states"
                    ),
                });
            }
        }
    }

    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.code.cmp(right.code))
            .then_with(|| left.detail.cmp(&right.detail))
    });
    findings.dedup();

    RiskReport { findings }
}

#[derive(Default)]
struct FileDiff {
    added: Vec<String>,
    removed: Vec<String>,
    deleted_file: bool,
}

fn parse_diff(diff: &str) -> BTreeMap<String, FileDiff> {
    let mut files = BTreeMap::<String, FileDiff>::new();
    let mut current: Option<String> = None;

    for line in diff.lines() {
        if let Some(path) = parse_diff_header(line) {
            current = Some(path.clone());
            files.entry(path).or_default();
            continue;
        }

        let Some(path) = current.as_ref() else {
            continue;
        };
        let file = files.entry(path.clone()).or_default();

        if line.starts_with("deleted file mode ") {
            file.deleted_file = true;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            file.added.push(line[1..].to_owned());
        } else if line.starts_with('-') && !line.starts_with("---") {
            file.removed.push(line[1..].to_owned());
        }
    }

    files
}

fn parse_diff_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix("diff --git a/")?;
    let (_, right) = rest.split_once(" b/")?;
    Some(right.to_owned())
}

fn protected_symbols(path: &str) -> &'static [&'static str] {
    match path {
        "crates/server/src/lib.rs" => &["security_boundary_middleware"],
        "crates/server/src/api/mod.rs" => &["auth_middleware", "admin_middleware"],
        "crates/server/src/api/auth.rs" => &[
            "security_boundary_middleware",
            "BURNCLOUD_INTERNAL_SECRET",
            "X-Internal-Secret",
        ],
        _ => &[],
    }
}

fn removed_symbol(file: &FileDiff, symbol: &str) -> bool {
    file.removed.iter().any(|line| line.contains(symbol))
}

fn added_symbol(file: &FileDiff, symbol: &str) -> bool {
    file.added.iter().any(|line| line.contains(symbol))
}

fn is_invariant_test(path: &str) -> bool {
    matches!(
        path,
        "crates/server/tests/security_invariants.rs"
            | "crates/router/tests/billing_invariants.rs"
            | "crates/router/tests/quota_tests.rs"
    )
}

fn is_test_path(path: &str) -> bool {
    path.contains("/tests/")
        || path.ends_with("_test.rs")
        || path.ends_with("_tests.rs")
        || path.starts_with("tests/")
}

fn is_runtime_source(path: &str) -> bool {
    path.ends_with(".rs") && !is_test_path(path)
}

fn is_sensitive_runtime_path(path: &str) -> bool {
    matches!(
        path,
        "crates/server/src/lib.rs"
            | "crates/server/src/api/auth.rs"
            | "crates/server/src/api/mod.rs"
            | "crates/database/crates/router/src/token.rs"
            | "crates/router/src/lib.rs"
    )
}

fn count_assertions(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|line| {
            line.contains("assert!(")
                || line.contains("assert_eq!(")
                || line.contains("assert_ne!(")
                || line.contains("debug_assert!(")
        })
        .count()
}

fn count_fail_closed_constructs(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|line| {
            line.contains("bail!(")
                || line.contains("ensure!(")
                || line.contains("return Err(")
                || line.contains("ok_or(")
                || line.contains("ok_or_else(")
        })
        .count()
}

fn contains_todo_marker(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    upper.contains("TODO") || upper.contains("FIXME")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_new_ignore() {
        let report = inspect(
            "diff --git a/crates/router/tests/quota_tests.rs b/crates/router/tests/quota_tests.rs\n+#[ignore]\n",
        );
        assert_eq!(report.blockers()[0].code, "TEST_IGNORE_ADDED");
    }

    #[test]
    fn blocks_removed_auth_middleware_without_replacement() {
        let report = inspect(
            "diff --git a/crates/server/src/api/mod.rs b/crates/server/src/api/mod.rs\n-    auth_middleware(state)\n+    routes\n",
        );
        assert!(report
            .blockers()
            .iter()
            .any(|finding| finding.code == "SECURITY_GUARD_REMOVED"));
    }

    #[test]
    fn moving_protected_symbol_inside_same_file_is_allowed() {
        let report = inspect(
            "diff --git a/crates/server/src/api/mod.rs b/crates/server/src/api/mod.rs\n-    auth_middleware(old_state)\n+    auth_middleware(new_state)\n",
        );
        assert!(report.blockers().is_empty());
    }

    #[test]
    fn assertion_reduction_requires_review() {
        let report = inspect(
            "diff --git a/crates/router/tests/quota_tests.rs b/crates/router/tests/quota_tests.rs\n-assert_eq!(used, expected);\n-assert!(settled);\n+assert!(settled);\n",
        );
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.code == "ASSERTION_WEAKENING"));
    }

    #[test]
    fn deleted_invariant_suite_is_blocked() {
        let report = inspect(
            "diff --git a/crates/server/tests/security_invariants.rs b/crates/server/tests/security_invariants.rs\ndeleted file mode 100644\n",
        );
        assert!(report
            .blockers()
            .iter()
            .any(|finding| finding.code == "INVARIANT_TEST_DELETED"));
    }
}
