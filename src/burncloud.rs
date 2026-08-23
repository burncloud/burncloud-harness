use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use crate::{
    config::{BurncloudArea, TaskSpec},
    invariants::InvariantSelection,
    route::RouteSelection,
};

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
        let time_budget = task.agent.time_budget_prompt_text();
        let context_files = if matches!(task.area, BurncloudArea::Ui) {
            ui_context_prompt_text(task)
        } else {
            task.context_prompt_text()
        };
        let convergence = if matches!(task.area, BurncloudArea::Ui) {
            ui_convergence_prompt(task, attempt, previous_feedback)
        } else {
            String::new()
        };
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

Time budget for this attempt:
{time_budget}

Time-budget behavior:
- Quality remains the priority. Use the budget to reduce repeated exploration, not to skip required reasoning or verification.
- Before the soft convergence target, finish discovery and move toward the smallest coherent implementation.
- At the soft target, stop broad exploration. Preserve correct work, close the smallest remaining gaps, and prepare a concise checkpoint/report.
- The hard limit is enforced by burncloud-harness. Do not wait until the final minute to save or organize edits.
- An idle warning means the harness has seen no agent output for the configured interval. Treat it as a signal to unblock or simplify the current approach.
- If the hard limit ends the process, the existing worktree is preserved and the next Harness attempt will continue from the real diff. Do not intentionally restart from scratch on later attempts.
{convergence}
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
            time_budget = time_budget,
            convergence = convergence,
            feedback = feedback,
        )
    }

    fn read(&self, path: &str) -> Result<String> {
        let full = self.root.join(path);
        fs::read_to_string(&full)
            .with_context(|| format!("failed to read required BurnCloud file {}", full.display()))
    }
}

fn ui_context_prompt_text(task: &TaskSpec) -> String {
    if task.resolved_context_files.is_empty() {
        return "- None declared".to_owned();
    }

    let mut primary = Vec::new();
    let mut supporting = Vec::new();
    for context in &task.resolved_context_files {
        let line = format!(
            "- {} -> {}",
            context.declared_path,
            context.absolute_path.display()
        );
        if is_primary_ui_context(&context.declared_path) {
            primary.push(line);
        } else {
            supporting.push(line);
        }
    }

    let primary = if primary.is_empty() {
        "- None auto-classified; use the most task-specific source/page contract first".to_owned()
    } else {
        primary.join("\n")
    };
    let supporting = if supporting.is_empty() {
        "- None".to_owned()
    } else {
        supporting.join("\n")
    };

    format!(
        "PRIMARY — read before the first UI edit:\n{primary}\n\nSUPPORTING — consult only when an active parity gap requires it; do not serially reread all of these on every attempt:\n{supporting}"
    )
}

fn is_primary_ui_context(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.ends_with("source-migration-fidelity.md")
        || normalized.contains("/page-contracts/")
        || normalized.ends_with("/design-system.md")
        || normalized.ends_with("/information-architecture.md")
        || normalized.contains("/src/pages/")
        || normalized.ends_with("/src/components/Layout.tsx")
}

fn ui_convergence_prompt(task: &TaskSpec, attempt: u32, previous_feedback: Option<&str>) -> String {
    let target_priority = ui_target_priority(task);
    let mode = if attempt <= 1 && previous_feedback.is_none() {
        r#"UI convergence mode: INITIAL SOURCE-FIDELITY PASS
- Before editing, compare source and target once in this fixed order: shell -> role navigation -> page header/actions -> major section order -> metric geometry -> main table/list/card geometry -> spacing/typography -> i18n/responsive behavior.
- Turn that comparison into a short internal delta list. Do not start by redesigning the page from memory.
- Read PRIMARY UI context first. Open SUPPORTING context only when a concrete delta requires it.
- Once source ownership and target ownership are known, stop broad repository search. Spend the remaining time implementing and verifying the delta list.
- Work top-down so later spacing does not hide structural mismatches.
- Preserve presentation geometry when runtime values are unavailable; truthful state changes values/content, not page identity.
- Before reporting completion, repeat the same comparison order and explicitly report every remaining mismatch. A major required landmark mismatch means the task is not complete."#
    } else {
        r#"UI convergence mode: REVISION / DELTA-CLOSURE PASS
- The current worktree is the baseline. FIRST inspect the existing Git diff and the previous Harness feedback before opening broad documentation.
- Do not restart discovery, redesign the page, or replace already-correct structure.
- Convert the previous feedback into a small ordered delta list. Every edit in this attempt must map to one of those deltas or to evidence directly required to verify it.
- Re-read only the PRIMARY source/contract file needed for the active delta. SUPPORTING context is on-demand, not a checklist.
- Prefer the smallest correction layer: spacing/visual mismatch -> local CSS/layout first; missing landmark -> page/component structure; truthful-state issue -> data/content wiring. Do not escalate layers without evidence.
- Preserve all already-correct sections and behavior. Avoid touching unrelated files merely to make the page look uniformly rewritten.
- Close one delta cluster at a time, then verify it before moving to the next.
- Before reporting completion, compare only the rejected/failed areas plus their immediate layout dependencies against the source again. Report any residual mismatch instead of claiming completion."#
    };

    format!(
        "\nBurnCloud UI convergence policy:\n{mode}\n\nPreferred target ownership for this task (start here before widening within the allowlist):\n{target_priority}\n"
    )
}

fn ui_target_priority(task: &TaskSpec) -> String {
    let mut primary = Vec::new();
    let mut supporting = Vec::new();
    for path in &task.scope.allowed {
        if is_primary_ui_target(path) {
            primary.push(format!("- {path}"));
        } else {
            supporting.push(format!("- {path}"));
        }
    }

    if primary.is_empty() {
        primary.push(
            "- No target path auto-classified; inspect the narrowest page/layout/CSS owner first"
                .to_owned(),
        );
    }
    if supporting.is_empty() {
        supporting.push("- None".to_owned());
    }

    format!(
        "PRIMARY TARGETS:\n{}\nSUPPORTING TARGETS (only if the delta requires them):\n{}",
        primary.join("\n"),
        supporting.join("\n")
    )
}

fn is_primary_ui_target(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.contains("/critical_pages/")
        || normalized.ends_with("/functional_layout.rs")
        || normalized.ends_with("/product_ui.css")
        || normalized.ends_with("/app.rs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_context_prioritizes_page_contract_source_page_and_layout() {
        assert!(is_primary_ui_context(
            "../../docs/ui/page-contracts/buyer-overview.md"
        ));
        assert!(is_primary_ui_context(
            "../../../burncloud-ui/src/pages/buyer/BuyerOverview.tsx"
        ));
        assert!(is_primary_ui_context(
            "../../../burncloud-ui/src/components/Layout.tsx"
        ));
        assert!(!is_primary_ui_context(
            "../../../burncloud-ui/src/i18n/locales/ja.ts"
        ));
    }

    #[test]
    fn ui_target_priority_keeps_page_layout_and_css_in_the_fast_path() {
        assert!(is_primary_ui_target(
            "crates/client/src/critical_pages/dashboard.rs"
        ));
        assert!(is_primary_ui_target(
            "crates/client/src/functional_layout.rs"
        ));
        assert!(is_primary_ui_target("crates/client/src/product_ui.css"));
        assert!(!is_primary_ui_target(
            "crates/tests/tests/e2e/console_pages.rs"
        ));
    }
}
