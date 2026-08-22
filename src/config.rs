use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct TaskSpec {
    pub name: String,
    pub goal: String,
    #[serde(default = "default_workspace")]
    pub workspace: String,
    #[serde(default = "default_max_loops")]
    pub max_loops: u32,
    pub area: BurncloudArea,
    pub scope: ScopeSpec,
    pub agent: AgentSpec,
    #[serde(default)]
    pub extra_checks: Vec<CheckSpec>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BurncloudArea {
    Router,
    Billing,
    Auth,
    Channel,
    Token,
    Ui,
    Database,
    Workspace,
    Other,
}

impl BurncloudArea {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Router => "router",
            Self::Billing => "billing",
            Self::Auth => "auth",
            Self::Channel => "channel",
            Self::Token => "token",
            Self::Ui => "ui",
            Self::Database => "database",
            Self::Workspace => "workspace",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScopeSpec {
    pub allowed: Vec<String>,
    #[serde(default)]
    pub avoid: Vec<String>,
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
        let mut task: Self = serde_yaml::from_str(&raw)
            .with_context(|| format!("failed to parse task file {}", path.display()))?;
        task.normalize_agent_compatibility();
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
        if self.scope.allowed.is_empty() {
            bail!("scope.allowed must not be empty; burncloud-harness fails closed");
        }
        if self.agent.program.trim().is_empty() {
            bail!("agent.program must not be empty");
        }
        for check in &self.extra_checks {
            if check.name.trim().is_empty() || check.command.trim().is_empty() {
                bail!("every extra check requires a non-empty name and command");
            }
        }
        Ok(())
    }

    fn normalize_agent_compatibility(&mut self) {
        if !is_codex_program(&self.agent.program)
            || !self.agent.args.iter().any(|arg| arg == "--full-auto")
        {
            return;
        }

        let already_has_sandbox = self
            .agent
            .args
            .iter()
            .any(|arg| arg == "--sandbox" || arg.starts_with("--sandbox="));
        let mut normalized = Vec::with_capacity(self.agent.args.len() + 1);
        let mut inserted_workspace_write = false;

        for arg in &self.agent.args {
            if arg == "--full-auto" {
                if !already_has_sandbox && !inserted_workspace_write {
                    normalized.push("--sandbox".to_owned());
                    normalized.push("workspace-write".to_owned());
                    inserted_workspace_write = true;
                }
                continue;
            }
            normalized.push(arg.clone());
        }

        self.agent.args = normalized;
    }
}

fn is_codex_program(program: &str) -> bool {
    Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("codex"))
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
            area: BurncloudArea::Other,
            scope: ScopeSpec {
                allowed: vec![],
                avoid: vec![],
            },
            agent: AgentSpec {
                program: "agent".into(),
                args: vec![],
                append_prompt: true,
            },
            extra_checks: vec![],
        };

        assert!(task.validate().is_err());
    }

    #[test]
    fn migrates_legacy_codex_full_auto_to_workspace_write() {
        let mut task = TaskSpec {
            name: "test".into(),
            goal: "do something".into(),
            workspace: ".".into(),
            max_loops: 1,
            area: BurncloudArea::Ui,
            scope: ScopeSpec {
                allowed: vec!["crates/client/**".into()],
                avoid: vec![],
            },
            agent: AgentSpec {
                program: "codex".into(),
                args: vec!["exec".into(), "--full-auto".into()],
                append_prompt: true,
            },
            extra_checks: vec![],
        };

        task.normalize_agent_compatibility();

        assert_eq!(
            task.agent.args,
            vec!["exec", "--sandbox", "workspace-write"]
        );
    }

    #[test]
    fn preserves_explicit_codex_sandbox_when_removing_full_auto() {
        let mut task = TaskSpec {
            name: "test".into(),
            goal: "do something".into(),
            workspace: ".".into(),
            max_loops: 1,
            area: BurncloudArea::Ui,
            scope: ScopeSpec {
                allowed: vec!["crates/client/**".into()],
                avoid: vec![],
            },
            agent: AgentSpec {
                program: "codex".into(),
                args: vec![
                    "exec".into(),
                    "--full-auto".into(),
                    "--sandbox".into(),
                    "danger-full-access".into(),
                ],
                append_prompt: true,
            },
            extra_checks: vec![],
        };

        task.normalize_agent_compatibility();

        assert_eq!(
            task.agent.args,
            vec!["exec", "--sandbox", "danger-full-access"]
        );
    }
}
