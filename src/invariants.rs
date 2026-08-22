use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result};

use crate::{config::BurncloudArea, route::RouteSelection};

const INVARIANTS_PATH: &str = "docs/agent/INVARIANTS.md";

#[derive(Debug, Clone)]
pub struct Invariant {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct InvariantSelection {
    pub items: Vec<Invariant>,
}

impl InvariantSelection {
    pub fn ids(&self) -> Vec<String> {
        self.items.iter().map(|item| item.id.clone()).collect()
    }

    pub fn prompt_text(&self) -> String {
        if self.items.is_empty() {
            return "No invariant was selected automatically. You must still inspect docs/agent/INVARIANTS.md and identify any invariant affected by the verified execution path.".to_owned();
        }

        self.items
            .iter()
            .map(|item| format!("- {} — {}", item.id, item.title))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn resolve(
    root: &Path,
    area: BurncloudArea,
    goal: &str,
    routes: &RouteSelection,
) -> Result<InvariantSelection> {
    let path = root.join(INVARIANTS_PATH);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read BurnCloud invariants {}", path.display()))?;
    let catalog = parse_invariants(&raw);
    let prefixes = relevant_prefixes(area, goal, routes);

    Ok(InvariantSelection {
        items: catalog
            .into_iter()
            .filter(|item| prefixes.iter().any(|prefix| item.id.starts_with(prefix)))
            .collect(),
    })
}

fn parse_invariants(markdown: &str) -> Vec<Invariant> {
    markdown
        .lines()
        .filter_map(|line| {
            let heading = line.trim().strip_prefix("### ")?;
            if !heading.starts_with("INV-") {
                return None;
            }
            let (id, title) = heading.split_once(" — ")?;
            Some(Invariant {
                id: id.trim().to_owned(),
                title: title.trim().to_owned(),
            })
        })
        .collect()
}

fn relevant_prefixes(
    area: BurncloudArea,
    goal: &str,
    routes: &RouteSelection,
) -> BTreeSet<&'static str> {
    let mut prefixes = BTreeSet::new();

    match area {
        BurncloudArea::Router => {
            prefixes.insert("INV-ROUTER-");
        }
        BurncloudArea::Billing => {
            prefixes.insert("INV-BILLING-");
        }
        BurncloudArea::Auth => {
            prefixes.insert("INV-AUTH-");
        }
        BurncloudArea::Channel => {}
        BurncloudArea::Token => {
            prefixes.insert("INV-AUTH-");
        }
        BurncloudArea::Ui => {}
        BurncloudArea::Database => {
            prefixes.insert("INV-DB-");
        }
        BurncloudArea::Workspace => {
            prefixes.insert("INV-WORKSPACE-");
            prefixes.insert("INV-RUNTIME-");
        }
        BurncloudArea::Other => {}
    }

    let context = format!(
        "{} {}",
        goal.to_ascii_lowercase(),
        routes
            .rows
            .iter()
            .map(|row| row.behavior.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    );

    if contains_any(
        &context,
        &["billing", "quota", "settlement", "spend", "cost", "usage"],
    ) {
        prefixes.insert("INV-BILLING-");
    }
    if contains_any(
        &context,
        &["auth", "jwt", "credential", "token", "password", "admin"],
    ) {
        prefixes.insert("INV-AUTH-");
    }
    if contains_any(
        &context,
        &["internal", "circuit-breaker", "price sync", "prices sync"],
    ) {
        prefixes.insert("INV-INTERNAL-");
    }
    if contains_any(
        &context,
        &[
            "server startup",
            "create_app",
            "fallback service",
            "liveview",
        ],
    ) {
        prefixes.insert("INV-RUNTIME-");
    }
    if contains_any(
        &context,
        &["database", "sql", "sqlite", "postgres", "placeholder"],
    ) {
        prefixes.insert("INV-DB-");
    }
    if contains_any(&context, &["workspace", "dependency", "cargo", "clippy"]) {
        prefixes.insert("INV-WORKSPACE-");
    }

    prefixes
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{RouteRow, RouteSelection};

    const DOC: &str = r#"
### INV-ROUTER-001 — Router fallback remains explicit
### INV-AUTH-001 — Auth boundary remains separate
### INV-BILLING-001 — Quota represents spend
### INV-DB-001 — Placeholder syntax is abstracted
"#;

    #[test]
    fn parses_invariant_headings() {
        let items = parse_invariants(DOC);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].id, "INV-ROUTER-001");
    }

    #[test]
    fn router_billing_route_selects_router_and_billing_invariants() {
        let routes = RouteSelection {
            rows: vec![RouteRow {
                behavior: "Streaming usage / response handling".into(),
                primary: "router".into(),
                related: "billing".into(),
                evidence: "tests".into(),
            }],
        };
        let prefixes = relevant_prefixes(BurncloudArea::Router, "fix usage", &routes);
        assert!(prefixes.contains("INV-ROUTER-"));
        assert!(prefixes.contains("INV-BILLING-"));
    }
}
