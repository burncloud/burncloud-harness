use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::git::GitRepo;

pub struct EvidenceBundle {
    run_id: String,
    run_dir: PathBuf,
    workspace: PathBuf,
    trajectory: BufWriter<File>,
    task: Option<String>,
}

impl EvidenceBundle {
    pub fn create(state_dir: &Path, run_id: &str) -> Result<Self> {
        let run_dir = state_dir.join("runs").join(run_id);
        fs::create_dir_all(&run_dir)
            .with_context(|| format!("failed to create evidence directory {}", run_dir.display()))?;
        let trajectory_path = run_dir.join("trajectory.jsonl");
        let trajectory = BufWriter::new(
            File::create(&trajectory_path).with_context(|| {
                format!(
                    "failed to create evidence trajectory {}",
                    trajectory_path.display()
                )
            })?,
        );
        let diff_path = run_dir.join("diff.patch");
        fs::write(&diff_path, b"").with_context(|| {
            format!("failed to initialize evidence diff {}", diff_path.display())
        })?;

        Ok(Self {
            run_id: run_id.to_owned(),
            run_dir,
            workspace: workspace_from_state_dir(state_dir),
            trajectory,
            task: None,
        })
    }

    pub fn start(&mut self, contract: RunContract<'_>) -> Result<()> {
        self.task = Some(contract.task.to_owned());
        let snapshot = TaskSnapshot {
            name: contract.task,
            goal: contract.goal,
            workspace: self.workspace.display().to_string(),
            area: contract.area,
            max_loops: contract.max_loops,
            scope: ScopeSnapshot {
                allowed: contract.allowed,
                avoid: contract.avoid,
            },
            agent: AgentSnapshot {
                program: contract.agent_program,
                args: contract.agent_args,
                append_prompt: contract.agent_append_prompt,
            },
            context_files: contract.context_files,
            resumed_from: contract.resumed_from,
        };
        let task_yaml = serde_yaml::to_string(&snapshot)?;
        fs::write(self.run_dir.join("task.yaml"), task_yaml)?;
        self.write_summary("RUNNING", false, 0, &[])
    }

    pub fn write_trajectory_line(&mut self, line: &[u8]) -> Result<()> {
        self.trajectory.write_all(line)?;
        self.trajectory.write_all(b"\n")?;
        self.trajectory.flush()?;
        Ok(())
    }

    pub fn snapshot_diff(&self) -> Result<()> {
        let path = self.run_dir.join("diff.patch");
        let diff = GitRepo::new(&self.workspace)
            .diff()
            .unwrap_or_else(|error| format!("# diff snapshot unavailable: {error}\n"));
        fs::write(&path, diff)
            .with_context(|| format!("failed to write evidence diff {}", path.display()))?;
        Ok(())
    }

    pub fn finish(&self, success: bool, attempts: u32, changed_paths: &[String]) -> Result<()> {
        self.write_summary(
            if success { "PASSED" } else { "FAILED" },
            success,
            attempts,
            changed_paths,
        )
    }

    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    fn write_summary(
        &self,
        status: &str,
        success: bool,
        attempts: u32,
        changed_paths: &[String],
    ) -> Result<()> {
        let summary = EvidenceSummary {
            run_id: &self.run_id,
            task: self.task.as_deref().unwrap_or("unknown"),
            status,
            success,
            attempts,
            changed_paths,
            files: EvidenceFiles {
                task: "task.yaml",
                events: "events.jsonl",
                trajectory: "trajectory.jsonl",
                diff: "diff.patch",
                summary: "summary.json",
            },
        };
        let bytes = serde_json::to_vec_pretty(&summary)?;
        fs::write(self.run_dir.join("summary.json"), bytes)?;
        Ok(())
    }
}

pub struct RunContract<'a> {
    pub task: &'a str,
    pub goal: &'a str,
    pub area: &'a str,
    pub max_loops: u32,
    pub allowed: &'a [String],
    pub avoid: &'a [String],
    pub context_files: &'a [String],
    pub agent_program: &'a str,
    pub agent_args: &'a [String],
    pub agent_append_prompt: bool,
    pub resumed_from: Option<&'a str>,
}

#[derive(Serialize)]
struct TaskSnapshot<'a> {
    name: &'a str,
    goal: &'a str,
    workspace: String,
    area: &'a str,
    max_loops: u32,
    scope: ScopeSnapshot<'a>,
    agent: AgentSnapshot<'a>,
    context_files: &'a [String],
    resumed_from: Option<&'a str>,
}

#[derive(Serialize)]
struct ScopeSnapshot<'a> {
    allowed: &'a [String],
    avoid: &'a [String],
}

#[derive(Serialize)]
struct AgentSnapshot<'a> {
    program: &'a str,
    args: &'a [String],
    append_prompt: bool,
}

#[derive(Serialize)]
struct EvidenceSummary<'a> {
    run_id: &'a str,
    task: &'a str,
    status: &'a str,
    success: bool,
    attempts: u32,
    changed_paths: &'a [String],
    files: EvidenceFiles<'a>,
}

#[derive(Serialize)]
struct EvidenceFiles<'a> {
    task: &'a str,
    events: &'a str,
    trajectory: &'a str,
    diff: &'a str,
    summary: &'a str,
}

fn workspace_from_state_dir(state_dir: &Path) -> PathBuf {
    let Some(git_dir) = state_dir.parent() else {
        return PathBuf::from(".");
    };

    let worktree_git_file = git_dir.join("gitdir");
    if let Ok(path) = fs::read_to_string(worktree_git_file) {
        let path = PathBuf::from(path.trim());
        if let Some(worktree) = path.parent() {
            return worktree.to_path_buf();
        }
    }

    git_dir.parent().unwrap_or(git_dir).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_state() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("burncloud-evidence-{}-{unique}", std::process::id()))
            .join(".git/burncloud-harness")
    }

    #[test]
    fn creates_complete_evidence_layout() {
        let state = temp_state();
        let mut bundle = EvidenceBundle::create(&state, "run-1").unwrap();
        let allowed = vec!["src/**".to_owned()];
        let avoid = vec!["Cargo.lock".to_owned()];
        let args = vec!["exec".to_owned()];
        bundle
            .start(RunContract {
                task: "test-task",
                goal: "test goal",
                area: "ui",
                max_loops: 3,
                allowed: &allowed,
                avoid: &avoid,
                context_files: &[],
                agent_program: "codex",
                agent_args: &args,
                agent_append_prompt: true,
                resumed_from: None,
            })
            .unwrap();
        bundle
            .write_trajectory_line(b"{\"type\":\"run_started\"}")
            .unwrap();
        bundle
            .finish(true, 1, &["src/main.rs".to_owned()])
            .unwrap();

        for file in ["task.yaml", "trajectory.jsonl", "diff.patch", "summary.json"] {
            assert!(bundle.run_dir().join(file).is_file(), "missing {file}");
        }
        let summary = fs::read_to_string(bundle.run_dir().join("summary.json")).unwrap();
        assert!(summary.contains("\"status\": \"PASSED\""));
        assert!(summary.contains("src/main.rs"));
        fs::remove_dir_all(state.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn resolves_linked_worktree_from_gitdir_pointer() {
        let state = temp_state();
        let git_dir = state.parent().unwrap();
        fs::create_dir_all(git_dir).unwrap();
        let worktree = state
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("linked-worktree");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(git_dir.join("gitdir"), worktree.join(".git").display().to_string()).unwrap();

        assert_eq!(workspace_from_state_dir(&state), worktree);
        fs::remove_dir_all(state.parent().unwrap().parent().unwrap()).unwrap();
    }
}
