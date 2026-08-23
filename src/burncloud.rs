use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use crate::{config::TaskSpec, invariants::InvariantSelection, route::RouteSelection};

pub const REQUIRED_BOOTSTRAP_DOCS: &[&str] = &[
    "AGENTS.md",
    "docs/CLAUDE.md",
    "docs/agent/START_HERE.md",
    "docs/agent/TASK_ROUTER.md",
    "docs/agent/TASK_CONTRACT.md",
    "docs/agent/INVARIANTS.md",
    "docs/agent/INVARIANT_STANDARD.md",
    "docs/agent/TEST_MATRIX.md",
    "docs/agent/verification/VERIFICATION_STANDARD.md",
    "docs/agent/CHANGE_PROTOCOL.md",
];

pub struct BurncloudRepo {
    root: PathBuf,
}

impl BurncloudRepo {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let repo = Self { root: root.into() };
        repo.validate()?;
        Ok(repo)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn validate(&self) -> Result<()> {
        let agents = self.read("AGENTS.md")?;
        if !agents.contains("# BurnCloud Agent Instructions") {
            bail!("workspace AGENTS.md is not the BurnCloud repository constitution");
        }

        let cargo = self.read("Cargo.toml")?;
        if !cargo.contains("name = \"burncloud\"")
            || !cargo.contains("https://github.com/burncloud/burncloud")
        {
            bail!("workspace Cargo.toml does not identify burncloud/burncloud");
        }

        for path in REQUIRED_BOOTSTRAP_DOCS {
            let full = self.root.join(path);
            if !full.is_file() {
                bail!("required BurnCloud agent document is missing: {path}");
            }
        }

        Ok(())
    }

    pub fn control_prompt(
        &self,
        task: &TaskSpec,
        routes: &RouteSelection,
        invariants: &InvariantSelection,
        attempt: u32,
        previous_feedback: Option<&str>,
    ) -> String {
        let allowed = task
            .scope
            .allowed
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n");
        let avoid = if task.scope.avoid.is_empty() {
            "- None declared beyond the allowlist".to_owned()
        } else {
            task.scope
                .avoid
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let change_budget = task
            .scope
            .max_changed_files
            .map(|limit| format!("- Maximum changed files: {limit}"))
            .unwrap_or_else(|| "- Maximum changed files: not separately capped".to_owned());
        let context_files = task.context_prompt_text();
        let feedback = previous_feedback
            .map(|value| format!("\nPrevious harness feedback:\n{value}\n"))
            .unwrap_or_default();

        format!(
            r#"You are working only on the burncloud/burncloud repository.

This run is controlled by burncloud-harness. The repository constitution is authoritative. Before editing, read the required bootstrap documents in AGENTS.md, including docs/CLAUDE.md, docs/agent/START_HERE.md, TASK_ROUTER.md, TASK_CONTRACT.md, INVARIANTS.md, TEST_MATRIX.md, VERIFICATION_STANDARD.md, and CHANGE_PROTOCOL.md.

Follow BurnCloud's execution loop exactly:
DISCOVER -> UNDERSTAND -> TRACE -> CONTRACT -> PLAN -> CHANGE -> VERIFY -> INSPECT -> REPORT

Task name: {name}
Task area: {area}
Attempt: {attempt}/{max_loops}
Goal: {goal}

Task-provided read-only reference documents:
{context_files}

These reference documents may live outside the BurnCloud checkout. Read them as task context, but do not modify, copy, or promote them into the BurnCloud repository unless the allowlist and task goal explicitly require that change.

Harness-selected TASK_ROUTER starting points:
{routes}

Harness-selected candidate invariants:
{invariants}

These selections are navigation hints from BurnCloud's current repository docs, not proof. Confirm the real execution path and actual invariant relevance from current source before editing.

Allowed change scope:
{allowed}

Explicit avoid scope:
{avoid}

Change budget:
{change_budget}

Hard rules for this run:
- Understand current behavior from real BurnCloud source before changing it.
- Start discovery from the selected TASK_ROUTER rows when they are relevant, then confirm ownership from current source.
- Trace the smallest relevant execution path before editing runtime behavior.
- Establish the task contract required by docs/agent/TASK_CONTRACT.md.
- Explicitly verify the selected candidate invariants and add any additional relevant invariants discovered from the real execution path.
- Make the smallest coherent change that satisfies the goal.
- Do not modify anything outside the allowlist.
- Treat the declared changed-file budget as a hard boundary. Plan within it, count the real changed files before finishing, and if correct implementation requires exceeding it, STOP and report NEED_SCOPE_EXPANSION instead of broadening the diff.
- If evidence shows the root cause requires a file outside the allowlist, STOP and report NEED_SCOPE_EXPANSION with the exact file/domain and evidence. Do not edit that file.
- Do not weaken, delete, skip, or ignore tests to make the task green.
- Do not commit, push, merge, reset, clean, or rewrite git history.
- Do not create root-level scratch files or task-contract artifacts. Keep reasoning/reporting in your response.
- Run the closest useful verification you can, but burncloud-harness will independently run mandatory checks after you finish.
- End with a concise REPORT that lists: verified execution path, files changed, verification actually run, relevant invariants, remaining risk, and unrelated changes.
{feedback}"#,
            name = task.name,
            area = task.area.as_str(),
            attempt = attempt,
            max_loops = task.max_loops,
            goal = task.goal,
            context_files = context_files,
            routes = routes.prompt_text(),
            invariants = invariants.prompt_text(),
            allowed = allowed,
            avoid = avoid,
            change_budget = change_budget,
            feedback = feedback,
        )
    }

    fn read(&self, path: &str) -> Result<String> {
        let full = self.root.join(path);
        fs::read_to_string(&full)
            .with_context(|| format!("failed to read required BurnCloud file {}", full.display()))
    }
}
