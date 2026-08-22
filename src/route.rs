use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result};

use crate::config::BurncloudArea;

const TASK_ROUTER_PATH: &str = "docs/agent/TASK_ROUTER.md";
const MAX_ROUTE_ROWS: usize = 2;

#[derive(Debug, Clone)]
pub struct RouteRow {
    pub behavior: String,
    pub primary: String,
    pub related: String,
    pub evidence: String,
}

#[derive(Debug, Clone)]
pub struct RouteSelection {
    pub rows: Vec<RouteRow>,
}

impl RouteSelection {
    pub fn labels(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| format!("{} -> {}", row.behavior, row.primary))
            .collect()
    }

    pub fn prompt_text(&self) -> String {
        if self.rows.is_empty() {
            return "No deterministic TASK_ROUTER row matched. Follow TASK_ROUTER.md's fallback discovery procedure and prove the entry/ownership path from source before editing.".to_owned();
        }

        self.rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                format!(
                    "{}. Behavior: {}\n   Primary: {}\n   Related: {}\n   Evidence/tests to inspect: {}",
                    index + 1,
                    row.behavior,
                    row.primary,
                    row.related,
                    row.evidence
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn resolve(root: &Path, goal: &str, area: BurncloudArea) -> Result<RouteSelection> {
    let path = root.join(TASK_ROUTER_PATH);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read BurnCloud task router {}", path.display()))?;
    Ok(select(parse_rows(&raw), goal, area))
}

fn parse_rows(markdown: &str) -> Vec<RouteRow> {
    markdown.lines().filter_map(parse_row).collect::<Vec<_>>()
}

fn parse_row(line: &str) -> Option<RouteRow> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|')
        || trimmed.contains("|---")
        || trimmed.contains("Task / user behavior")
    {
        return None;
    }

    let cells = trimmed
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect::<Vec<_>>();
    if cells.len() != 4 || cells[0].is_empty() {
        return None;
    }

    Some(RouteRow {
        behavior: cells[0].to_owned(),
        primary: cells[1].to_owned(),
        related: cells[2].to_owned(),
        evidence: cells[3].to_owned(),
    })
}

fn select(rows: Vec<RouteRow>, goal: &str, area: BurncloudArea) -> RouteSelection {
    let goal_tokens = tokens(goal);
    let area_tokens = area_tokens(area);

    let mut candidates = rows
        .into_iter()
        .map(|row| {
            let behavior_tokens = tokens(&row.behavior);
            let goal_overlap = behavior_tokens.intersection(&goal_tokens).count();
            let area_overlap = behavior_tokens.intersection(&area_tokens).count();
            (area_overlap, goal_overlap, row)
        })
        .collect::<Vec<_>>();

    // A declared task area is an ownership boundary, not a weak hint. If the
    // repository router contains rows that match the area, do not let words in
    // a long goal (for example "routing", "admin", or "token" in UI product
    // copy) pull unrelated backend rows into the candidate set.
    if !area_tokens.is_empty() && candidates.iter().any(|(area, _, _)| *area > 0) {
        candidates.retain(|(area, _, _)| *area > 0);
    }

    let mut scored = candidates
        .into_iter()
        .map(|(area_overlap, goal_overlap, row)| {
            let score = area_overlap * 8 + goal_overlap * 4;
            (score, row)
        })
        .filter(|(score, _)| *score > 0)
        .collect::<Vec<_>>();

    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.behavior.cmp(&right.behavior))
    });

    RouteSelection {
        rows: scored
            .into_iter()
            .take(MAX_ROUTE_ROWS)
            .map(|(_, row)| row)
            .collect(),
    }
}

fn tokens(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .map(str::trim)
        .filter(|token| token.len() >= 3)
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn area_tokens(area: BurncloudArea) -> BTreeSet<String> {
    let words: &[&str] = match area {
        BurncloudArea::Router => &[
            "router",
            "routing",
            "fallback",
            "provider",
            "passthrough",
            "retry",
            "streaming",
            "relay",
        ],
        BurncloudArea::Billing => &["billing", "cost", "quota", "usage", "settlement", "spend"],
        BurncloudArea::Auth => &[
            "auth",
            "authentication",
            "authorization",
            "login",
            "password",
            "jwt",
            "register",
        ],
        BurncloudArea::Channel => &["channel", "candidate", "ranking", "affinity"],
        BurncloudArea::Token => &["token", "credential", "key", "quota"],
        BurncloudArea::Ui => &["ui", "console", "page", "client", "styling"],
        BurncloudArea::Database => &["database", "sql", "dialect", "migration", "persistence"],
        BurncloudArea::Workspace => &[
            "workspace",
            "startup",
            "process",
            "cli",
            "dependency",
            "installer",
            "download",
            "update",
        ],
        BurncloudArea::Other => &[],
    };

    words.iter().map(|word| (*word).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTER: &str = r#"
| Task / user behavior | Primary source | Related source | Tests / evidence to inspect |
|---|---|---|---|
| Data-plane request entry, fallback routing | `crates/router/src/lib.rs` | `crates/server/src/lib.rs` | `relay.rs` |
| Provider passthrough / conversion / retry | `crates/router/src/lib.rs`, `passthrough.rs` | provider adapters | provider tests |
| Billing / cost / quota settlement | `crates/router/src/lib.rs` | billing crates | billing tests |
| UI / Console page behavior | affected crate under `crates/client/crates/` | `crates/client` shared components/routes | console page tests |
"#;

    #[test]
    fn provider_retry_goal_prefers_provider_row() {
        let selection = select(
            parse_rows(ROUTER),
            "Fix provider retry when upstream fails",
            BurncloudArea::Router,
        );

        assert_eq!(
            selection.rows[0].behavior,
            "Provider passthrough / conversion / retry"
        );
    }

    #[test]
    fn fallback_goal_keeps_fallback_row() {
        let selection = select(
            parse_rows(ROUTER),
            "Preserve fallback routing",
            BurncloudArea::Router,
        );

        assert!(selection
            .rows
            .iter()
            .any(|row| row.behavior.contains("fallback routing")));
    }

    #[test]
    fn ui_area_does_not_route_product_copy_to_backend_rows() {
        let selection = select(
            parse_rows(ROUTER),
            "Rebuild Buyer Overview without exposing routing topology, admin internals, or API token infrastructure",
            BurncloudArea::Ui,
        );

        assert_eq!(selection.rows.len(), 1);
        assert_eq!(selection.rows[0].behavior, "UI / Console page behavior");
    }
}
