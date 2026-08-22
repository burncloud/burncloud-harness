use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result};

use crate::{config::BurncloudArea, route::RouteSelection};

const INVARIANTS_PATH: &str = "docs/agent/INVARIANTS.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invariant {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct InvariantSelection {
    pub items: Vec<Invariant>,
}

#[derive(Debug, Clone)]
pub struct InvariantImpact {
    pub required: InvariantSelection,
    pub newly_required: InvariantSelection,
    pub reasons: Vec<String>,
}

impl InvariantSelection {
    pub fn ids(&self) -> Vec<String> {
        self.items.iter().map(|item| item.id.clone()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn merge(&mut self, other: &InvariantSelection) {
        let mut seen = self
            .items
            .iter()
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>();

        for item in &other.items {
            if seen.insert(item.id.clone()) {
                self.items.push(item.clone());
            }
        }

        self.items.sort_by(|left, right| left.id.cmp(&right.id));
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
    let catalog = load_catalog(root)?;
    let prefixes = relevant_prefixes(area, goal, routes);
    Ok(select_by_prefixes(catalog, &prefixes))
}

pub fn assess_changed_paths(
    root: &Path,
    changed_paths: &[String],
    selected: &InvariantSelection,
) -> Result<InvariantImpact> {
    let catalog = load_catalog(root)?;
    let mut prefixes = BTreeSet::new();
    let mut reasons = Vec::new();

    for path in changed_paths {
        for (prefix, reason) in invariant_families_for_path(path) {
            prefixes.insert(prefix);
            reasons.push(format!("{path}: {reason}"));
        }
    }

    reasons.sort();
    reasons.dedup();

    let required = select_by_prefixes(catalog, &prefixes);
    let selected_ids = selected.ids().into_iter().collect::<BTreeSet<_>>();
    let newly_required = InvariantSelection {
        items: required
            .items
            .iter()
            .filter(|item| !selected_ids.contains(&item.id))
            .cloned()
            .collect(),
    };

    Ok(InvariantImpact {
        required,
        newly_required,
        reasons,
    })
}

fn load_catalog(root: &Path) -> Result<Vec<Invariant>> {
    let path = root.join(INVARIANTS_PATH);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read BurnCloud invariants {}", path.display()))?;
    Ok(parse_invariants(&raw))
}

fn select_by_prefixes(
    catalog: Vec<Invariant>,
    prefixes: &BTreeSet<&'static str>,
) -> InvariantSelection {
    InvariantSelection {
        items: catalog
            .into_iter()
            .filter(|item| prefixes.iter().any(|prefix| item.id.starts_with(prefix)))
            .collect(),
    }
}

fn invariant_families_for_path(path: &str) -> Vec<(&'static str, &'static str)> {
    let mut families = Vec::new();

    if path == "src/main.rs" {
        families.push((
            "INV-RUNTIME-",
            "root command dispatch participates in BurnCloud runtime startup",
        ));
    }

    if path == "crates/server/src/lib.rs" {
        families.push((
            "INV-RUNTIME-",
            "server app composition defines runtime route ordering and fallback behavior",
        ));
        families.push((
            "INV-ROUTER-",
            "server composition controls how router routes and fallback are mounted",
        ));
        families.push((
            "INV-AUTH-",
            "server composition applies the security boundary across management and data plane",
        ));
    }

    if path.starts_with("crates/server/src/api/") {
        families.push((
            "INV-AUTH-",
            "management API routes participate in authentication or authorization boundaries",
        ));
    }

    if path == "crates/server/src/api/auth.rs" {
        families.push((
            "INV-INTERNAL-",
            "security boundary middleware protects sensitive internal control-plane mutations",
        ));
    }

    if path == "crates/server/tests/security_invariants.rs" {
        families.push((
            "INV-AUTH-",
            "security invariant tests are executable evidence for authentication boundaries",
        ));
        families.push((
            "INV-INTERNAL-",
            "security invariant tests cover internal-secret fail-closed behavior",
        ));
    }

    if path.starts_with("crates/router/src/") {
        families.push((
            "INV-ROUTER-",
            "router runtime source can alter data-plane routing or fallback semantics",
        ));
    }

    if path == "crates/router/src/lib.rs" {
        families.push((
            "INV-BILLING-",
            "router request lifecycle participates in credential-scoped usage settlement",
        ));
    }

    if path == "crates/database/crates/router/src/token.rs"
        || path == "crates/router/tests/billing_invariants.rs"
        || path == "crates/router/tests/quota_tests.rs"
    {
        families.push((
            "INV-BILLING-",
            "path is direct evidence or implementation for quota and spend settlement invariants",
        ));
    }

    if path == "crates/database/src/placeholder.rs" {
        families.push((
            "INV-DB-",
            "placeholder abstraction is the cross-database SQL dialect boundary",
        ));
    }

    if path == "Cargo.toml" || path == "Cargo.lock" {
        families.push((
            "INV-WORKSPACE-",
            "root dependency graph participates in workspace dependency invariants",
        ));
    }

    families
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

    // UI product requirements often mention backend concepts specifically to say
    // that they must NOT be exposed ("admin", "token", "routing", "internal").
    // Those words are not evidence that backend runtime invariants are in scope.
    // If a UI task actually touches a backend boundary, assess_changed_paths()
    // will promote the required invariant from the real diff.
    if matches!(area, BurncloudArea::Ui) {
        return prefixes;
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
### INV-RUNTIME-001 — Runtime startup remains unified
### INV-ROUTER-001 — Router fallback remains explicit
### INV-AUTH-001 — Auth boundary remains separate
### INV-INTERNAL-001 — Internal mutations fail closed
### INV-BILLING-001 — Quota represents spend
### INV-BILLING-002 — Settlement is credential scoped
### INV-DB-001 — Placeholder syntax is abstracted
### INV-WORKSPACE-001 — Workspace dependencies are centralized
"#;

    fn catalog() -> Vec<Invariant> {
        parse_invariants(DOC)
    }

    #[test]
    fn parses_invariant_headings() {
        let items = catalog();
        assert_eq!(items.len(), 8);
        assert_eq!(items[0].id, "INV-RUNTIME-001");
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

    #[test]
    fn ui_product_copy_does_not_invent_backend_invariants() {
        let routes = RouteSelection {
            rows: vec![RouteRow {
                behavior: "UI / Console page behavior".into(),
                primary: "client".into(),
                related: "shared components".into(),
                evidence: "console tests".into(),
            }],
        };
        let prefixes = relevant_prefixes(
            BurncloudArea::Ui,
            "Hide admin token routing and internal infrastructure from Buyer UI",
            &routes,
        );
        assert!(prefixes.is_empty());
    }

    #[test]
    fn router_lib_implies_router_and_billing_families() {
        let families = invariant_families_for_path("crates/router/src/lib.rs")
            .into_iter()
            .map(|(family, _)| family)
            .collect::<BTreeSet<_>>();
        assert!(families.contains("INV-ROUTER-"));
        assert!(families.contains("INV-BILLING-"));
    }

    #[test]
    fn auth_middleware_implies_auth_and_internal_families() {
        let families = invariant_families_for_path("crates/server/src/api/auth.rs")
            .into_iter()
            .map(|(family, _)| family)
            .collect::<BTreeSet<_>>();
        assert!(families.contains("INV-AUTH-"));
        assert!(families.contains("INV-INTERNAL-"));
    }

    #[test]
    fn merge_deduplicates_invariant_ids() {
        let mut selected = InvariantSelection {
            items: vec![catalog()[1].clone()],
        };
        let incoming = InvariantSelection {
            items: vec![catalog()[1].clone(), catalog()[4].clone()],
        };
        selected.merge(&incoming);
        assert_eq!(selected.ids(), vec!["INV-BILLING-001", "INV-ROUTER-001"]);
    }
}
