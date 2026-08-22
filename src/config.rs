use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct TaskSpec {
    pub name: String,
    pub goal: String,
    #[serde(default = "default_workspace")]
    pub workspace: String,
    #[serde(default = "default_max_loops")]
    pub max_loops: u32,
    #[serde(default = "default_true")]
    pub require_clean_worktree: bool,
    pub policy: PolicySpec,
    pub agent: AgentSpec,
    #[serde(default)]
    pub checks: Vec<CheckSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolicySpec {
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub denied_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentSpec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_true")]
    pub append_prompt: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckSpec {
    pub name: String,
    pub command: String,
}

impl TaskSpec {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read task file {}", path.display()))?;
        let task: Self = serde_yaml::from_str(&raw)
            .with_context(|| format!("failed to parse task file {}", path.display()))?;
        task.validate()?;
        Ok(task)
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("task.name must not be empty");
        }
        if self.goal.trim().is_empty() {
            bail!("task.goal must not be empty");
        }
        if self.max_loops == 0 {
            bail!("task.max_loops must be at least 1");
        }
        if self.policy.allowed_paths.is_empty() {
            bail!("policy.allowed_paths must not be empty; burncloud-harness fails closed");
        }
        if self.agent.program.trim().is_empty() {
            bail!("agent.program must not be empty");
        }
        for check in &self.checks {
            if check.name.trim().is_empty() || check.command.trim().is_empty() {
                bail!("every check requires a non-empty name and command");
            }
        }
        Ok(())
    }
}

fn default_workspace() -> String {
    ".".to_owned()
}

fn default_max_loops() -> u32 {
    3
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_allowlist() {
        let task = TaskSpec {
            name: "test".into(),
            goal: "do something".into(),
            workspace: ".".into(),
            max_loops: 1,
            require_clean_worktree: true,
            policy: PolicySpec {
                allowed_paths: vec![],
                denied_paths: vec![],
            },
            agent: AgentSpec {
                program: "agent".into(),
                args: vec![],
                append_prompt: true,
            },
            checks: vec![],
        };

        assert!(task.validate().is_err());
    }
}
