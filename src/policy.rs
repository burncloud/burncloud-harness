use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::config::ScopeSpec;

#[derive(Debug)]
pub struct ScopePolicy {
    allowed: GlobSet,
    avoid: GlobSet,
}

#[derive(Debug, Clone)]
pub struct ScopeReport {
    pub allowed: Vec<String>,
    pub violations: Vec<String>,
}

impl ScopePolicy {
    pub fn compile(spec: &ScopeSpec) -> Result<Self> {
        Ok(Self {
            allowed: compile_globs(&spec.allowed).context("invalid scope.allowed glob")?,
            avoid: compile_globs(&spec.avoid).context("invalid scope.avoid glob")?,
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
            let permitted = self.allowed.is_match(&normalized) && !self.avoid.is_match(&normalized);
            if permitted {
                allowed.push(normalized);
            } else {
                violations.push(normalized);
            }
        }

        ScopeReport {
            allowed,
            violations,
        }
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
        ScopePolicy::compile(&ScopeSpec {
            allowed: vec![
                "crates/router/**".into(),
                "crates/tests/tests/api/**".into(),
            ],
            avoid: vec!["crates/router/secrets/**".into()],
        })
        .unwrap()
    }

    #[test]
    fn avoid_paths_override_allowed_paths() {
        let report = policy().evaluate([
            "crates/router/src/lib.rs",
            "crates/router/secrets/key.rs",
            "crates/service/crates/billing/src/lib.rs",
        ]);

        assert_eq!(report.allowed, vec!["crates/router/src/lib.rs"]);
        assert_eq!(
            report.violations,
            vec![
                "crates/router/secrets/key.rs",
                "crates/service/crates/billing/src/lib.rs"
            ]
        );
    }

    #[test]
    fn normalizes_windows_paths() {
        let report = policy().evaluate([r"crates\router\src\lib.rs"]);
        assert!(report.is_ok());
    }
}
