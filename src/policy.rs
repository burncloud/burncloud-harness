use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::config::PolicySpec;

#[derive(Debug)]
pub struct ScopePolicy {
    allowed: GlobSet,
    denied: GlobSet,
}

#[derive(Debug, Clone)]
pub struct ScopeReport {
    pub allowed: Vec<String>,
    pub violations: Vec<String>,
}

impl ScopePolicy {
    pub fn compile(spec: &PolicySpec) -> Result<Self> {
        Ok(Self {
            allowed: compile_globs(&spec.allowed_paths).context("invalid allowed_paths glob")?,
            denied: compile_globs(&spec.denied_paths).context("invalid denied_paths glob")?,
        })
    }

    pub fn evaluate<I, S>(&self, paths: I) -> ScopeReport
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut allowed = Vec::new();
        let mut violations = Vec::new();

        for path in paths {
            let normalized = normalize_path(path.as_ref());
            let permitted = self.allowed.is_match(&normalized) && !self.denied.is_match(&normalized);
            if permitted {
                allowed.push(normalized);
            } else {
                violations.push(normalized);
            }
        }

        ScopeReport { allowed, violations }
    }
}

impl ScopeReport {
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }
}

fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    Ok(builder.build()?)
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ScopePolicy {
        ScopePolicy::compile(&PolicySpec {
            allowed_paths: vec!["src/router/**".into(), "tests/router/**".into()],
            denied_paths: vec!["src/router/secrets/**".into()],
        })
        .unwrap()
    }

    #[test]
    fn denied_paths_override_allowed_paths() {
        let report = policy().evaluate([
            "src/router/mod.rs",
            "src/router/secrets/key.rs",
            "src/billing.rs",
        ]);

        assert_eq!(report.allowed, vec!["src/router/mod.rs"]);
        assert_eq!(
            report.violations,
            vec!["src/router/secrets/key.rs", "src/billing.rs"]
        );
    }

    #[test]
    fn normalizes_windows_paths() {
        let report = policy().evaluate([r"src\router\mod.rs"]);
        assert!(report.is_ok());
    }
}
