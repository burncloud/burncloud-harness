use std::{
    fs,
    path::{Path, PathBuf},
};

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
    pub context_files: Vec<String>,
    #[serde(skip)]
    pub resolved_context_files: Vec<ResolvedContextFile>,
    #[serde(default)]
    pub extra_checks: Vec<CheckSpec>,
}

#[derive(Debug, Clone)]
pub struct ResolvedContextFile {
    pub declared_path: String,
    pub absolute_path: PathBuf,
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
    #[serde(default)]
    pub max_changed_files: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentSpec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_true")]
    pub append_prompt: bool,
    #[serde(default)]
    pub soft_timeout_minutes: Option<u64>,
    #[serde(default)]
    pub hard_timeout_minutes: Option<u64>,
    #[serde(default)]
    pub idle_timeout_minutes: Option<u64>,
}

impl AgentSpec {
    pub fn soft_timeout_secs(&self) -> Option<u64> {
        self.soft_timeout_minutes.map(|minutes| minutes * 60)
    }

    pub fn hard_timeout_secs(&self) -> Option<u64> {
        self.hard_timeout_minutes.map(|minutes| minutes * 60)
    }

    pub fn idle_timeout_secs(&self) -> Option<u64> {
        self.idle_timeout_minutes.map(|minutes| minutes * 60)
    }

    pub fn time_budget_prompt_text(&self) -> String {
        let soft = self
            .soft_timeout_minutes
            .map(|minutes| format!("- Soft convergence target: {minutes} minutes"))
            .unwrap_or_else(|| "- Soft convergence target: not configured".to_owned());
        let hard = self
            .hard_timeout_minutes
            .map(|minutes| format!("- Hard per-attempt limit: {minutes} minutes"))
            .unwrap_or_else(|| "- Hard per-attempt limit: not configured".to_owned());
        let idle = self
            .idle_timeout_minutes
            .map(|minutes| format!("- Idle warning threshold: {minutes} minutes without output"))
            .unwrap_or_else(|| "- Idle warning threshold: not configured".to_owned());
        format!("{soft}\n{hard}\n{idle}")
    }
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
        task.resolve_context_files(path)?;
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
        if self.scope.max_changed_files == Some(0) {
            bail!("scope.max_changed_files must be at least 1 when declared");
        }
        if self.agent.program.trim().is_empty() {
            bail!("agent.program must not be empty");
        }
        for (name, value) in [
            ("soft_timeout_minutes", self.agent.soft_timeout_minutes),
            ("hard_timeout_minutes", self.agent.hard_timeout_minutes),
            ("idle_timeout_minutes", self.agent.idle_timeout_minutes),
        ] {
            if value == Some(0) {
                bail!("agent.{name} must be at least 1 when declared");
            }
        }
        if let (Some(soft), Some(hard)) = (
            self.agent.soft_timeout_minutes,
            self.agent.hard_timeout_minutes,
        ) {
            if soft >= hard {
                bail!("agent.soft_timeout_minutes must be less than agent.hard_timeout_minutes");
            }
        }
        if let (Some(idle), Some(hard)) = (
            self.agent.idle_timeout_minutes,
            self.agent.hard_timeout_minutes,
        ) {
            if idle >= hard {
                bail!("agent.idle_timeout_minutes must be less than agent.hard_timeout_minutes");
            }
        }
        for context_file in &self.context_files {
            if context_file.trim().is_empty() {
                bail!("context_files entries must not be empty");
            }
        }
        for check in &self.extra_checks {
            if check.name.trim().is_empty() || check.command.trim().is_empty() {
                bail!("every extra check requires a non-empty name and command");
            }
        }
        Ok(())
    }

    pub fn context_prompt_text(&self) -> String {
        if self.resolved_context_files.is_empty() {
            return "- None declared".to_owned();
        }

        self.resolved_context_files
            .iter()
            .map(|context_file| {
                format!(
                    "- {} -> {}",
                    context_file.declared_path,
                    context_file.absolute_path.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn resolve_context_files(&mut self, task_path: &Path) -> Result<()> {
        let task_path = task_path
            .canonicalize()
            .with_context(|| format!("failed to resolve task file {}", task_path.display()))?;
        let task_dir = task_path
            .parent()
            .context("task file has no parent directory")?;

        self.resolved_context_files = self
            .context_files
            .iter()
            .map(|declared_path| {
                let absolute_path =
                    task_dir
                        .join(declared_path)
                        .canonicalize()
                        .with_context(|| {
                            format!(
                                "failed to resolve context file '{}' relative to {}",
                                declared_path,
                                task_dir.display()
                            )
                        })?;
                if !absolute_path.is_file() {
                    bail!(
                        "task context path is not a file: {}",
                        absolute_path.display()
                    );
                }
                Ok(ResolvedContextFile {
                    declared_path: declared_path.clone(),
                    absolute_path,
                })
            })
            .collect::<Result<Vec<_>>>()?;
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

    fn agent() -> AgentSpec {
        AgentSpec {
            program: "agent".into(),
            args: vec![],
            append_prompt: true,
            soft_timeout_minutes: None,
            hard_timeout_minutes: None,
            idle_timeout_minutes: None,
        }
    }

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
                max_changed_files: None,
            },
            agent: agent(),
            context_files: vec![],
            resolved_context_files: vec![],
            extra_checks: vec![],
        };

        assert!(task.validate().is_err());
    }

    #[test]
    fn rejects_zero_change_budget() {
        let task = TaskSpec {
            name: "test".into(),
            goal: "do something".into(),
            workspace: ".".into(),
            max_loops: 1,
            area: BurncloudArea::Ui,
            scope: ScopeSpec {
                allowed: vec!["crates/client/**".into()],
                avoid: vec![],
                max_changed_files: Some(0),
            },
            agent: agent(),
            context_files: vec![],
            resolved_context_files: vec![],
            extra_checks: vec![],
        };

        assert!(task.validate().is_err());
    }

    #[test]
    fn rejects_invalid_agent_time_budgets() {
        let mut task = TaskSpec {
            name: "test".into(),
            goal: "do something".into(),
            workspace: ".".into(),
            max_loops: 1,
            area: BurncloudArea::Ui,
            scope: ScopeSpec {
                allowed: vec!["crates/client/**".into()],
                avoid: vec![],
                max_changed_files: None,
            },
            agent: agent(),
            context_files: vec![],
            resolved_context_files: vec![],
            extra_checks: vec![],
        };

        task.agent.soft_timeout_minutes = Some(25);
        task.agent.hard_timeout_minutes = Some(25);
        assert!(task.validate().is_err());

        task.agent.soft_timeout_minutes = Some(15);
        task.agent.hard_timeout_minutes = Some(25);
        task.agent.idle_timeout_minutes = Some(25);
        assert!(task.validate().is_err());

        task.agent.idle_timeout_minutes = Some(5);
        assert!(task.validate().is_ok());
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
                max_changed_files: None,
            },
            agent: AgentSpec {
                program: "codex".into(),
                args: vec!["exec".into(), "--full-auto".into()],
                append_prompt: true,
                soft_timeout_minutes: None,
                hard_timeout_minutes: None,
                idle_timeout_minutes: None,
            },
            context_files: vec![],
            resolved_context_files: vec![],
            extra_checks: vec![],
        };

        task.normalize_agent_compatibility();

        assert_eq!(
            task.agent.args,
            vec!["exec", "--sandbox", "workspace-write"]
        );
    }

    #[test]
    fn resolves_context_files_relative_to_the_task_yaml() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "burncloud-harness-context-{}-{unique}",
            std::process::id()
        ));
        let task_dir = root.join("tasks/ui");
        let docs_dir = root.join("docs/ui");
        fs::create_dir_all(&task_dir).unwrap();
        fs::create_dir_all(&docs_dir).unwrap();
        let context_path = docs_dir.join("contract.md");
        fs::write(&context_path, "approved target contract").unwrap();
        let task_path = task_dir.join("task.yaml");
        fs::write(
            &task_path,
            r#"name: context-test
goal: use the approved target contract
workspace: .
area: ui
scope:
  allowed:
    - crates/client/**
  max_changed_files: 8
agent:
  program: codex
  args:
    - exec
  soft_timeout_minutes: 15
  hard_timeout_minutes: 25
  idle_timeout_minutes: 5
context_files:
  - ../../docs/ui/contract.md
"#,
        )
        .unwrap();

        let task = TaskSpec::load(&task_path).unwrap();
        let expected = context_path.canonicalize().unwrap();

        assert_eq!(task.scope.max_changed_files, Some(8));
        assert_eq!(task.agent.soft_timeout_minutes, Some(15));
        assert_eq!(task.agent.hard_timeout_minutes, Some(25));
        assert_eq!(task.agent.idle_timeout_minutes, Some(5));
        assert_eq!(task.resolved_context_files.len(), 1);
        assert_eq!(task.resolved_context_files[0].absolute_path, expected);
        assert!(task
            .context_prompt_text()
            .contains("../../docs/ui/contract.md"));
        assert!(task
            .context_prompt_text()
            .contains(&expected.display().to_string()));
        fs::remove_dir_all(root).unwrap();
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
                max_changed_files: None,
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
                soft_timeout_minutes: None,
                hard_timeout_minutes: None,
                idle_timeout_minutes: None,
            },
            context_files: vec![],
            resolved_context_files: vec![],
            extra_checks: vec![],
        };

        task.normalize_agent_compatibility();

        assert_eq!(
            task.agent.args,
            vec!["exec", "--sandbox", "danger-full-access"]
        );
    }
}
