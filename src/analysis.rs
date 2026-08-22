use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Debug, Default)]
pub struct AnalysisReport {
    pub runs: usize,
    pub passed: usize,
    pub failed: usize,
    pub incomplete: usize,
    pub attempts: usize,
    pub agent_failures: usize,
    pub scope_violation_events: usize,
    pub parse_errors: usize,
    pub areas: BTreeMap<String, usize>,
    pub failure_classes: BTreeMap<String, usize>,
    pub scope_paths: BTreeMap<String, usize>,
    pub invariant_expansions: BTreeMap<String, usize>,
    pub risk_codes: BTreeMap<String, usize>,
    pub failed_checks: BTreeMap<String, usize>,
}

impl AnalysisReport {
    pub fn success_rate(&self) -> f64 {
        let finished = self.passed + self.failed;
        if finished == 0 {
            0.0
        } else {
            self.passed as f64 * 100.0 / finished as f64
        }
    }

    pub fn average_attempts(&self) -> f64 {
        if self.runs == 0 {
            0.0
        } else {
            self.attempts as f64 / self.runs as f64
        }
    }

    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str("BurnCloud Harness Trajectory Analysis\n");
        output.push_str(&format!(
            "runs={} pass={} fail={} incomplete={} success_rate={:.1}% avg_attempts={:.2}\n",
            self.runs,
            self.passed,
            self.failed,
            self.incomplete,
            self.success_rate(),
            self.average_attempts()
        ));
        output.push_str(&format!(
            "agent_failures={} scope_violation_events={} parse_errors={}\n",
            self.agent_failures, self.scope_violation_events, self.parse_errors
        ));

        append_ranked(&mut output, "Task areas", &self.areas);
        append_ranked(&mut output, "Failure classes", &self.failure_classes);
        append_ranked(
            &mut output,
            "Invariant expansions",
            &self.invariant_expansions,
        );
        append_ranked(&mut output, "Final-diff risk signals", &self.risk_codes);
        append_ranked(
            &mut output,
            "Failed verification gates",
            &self.failed_checks,
        );
        append_ranked(&mut output, "Scope violation paths", &self.scope_paths);

        let repeated = self.repeated_signals(3);
        if !repeated.is_empty() {
            output.push_str("\nRepeated signals worth harness review (>=3):\n");
            for signal in repeated {
                output.push_str("- ");
                output.push_str(&signal);
                output.push('\n');
            }
        }

        output
    }

    fn repeated_signals(&self, threshold: usize) -> Vec<String> {
        let mut signals = Vec::new();
        collect_repeated(
            &mut signals,
            "failure class",
            &self.failure_classes,
            threshold,
        );
        collect_repeated(
            &mut signals,
            "invariant expansion",
            &self.invariant_expansions,
            threshold,
        );
        collect_repeated(&mut signals, "risk", &self.risk_codes, threshold);
        collect_repeated(
            &mut signals,
            "verification failure",
            &self.failed_checks,
            threshold,
        );
        collect_repeated(
            &mut signals,
            "scope violation",
            &self.scope_paths,
            threshold,
        );
        signals.sort();
        signals
    }
}

pub fn analyze(state_dir: &Path, limit: usize) -> Result<AnalysisReport> {
    let runs_dir = state_dir.join("runs");
    if !runs_dir.is_dir() {
        return Ok(AnalysisReport::default());
    }

    let mut paths = fs::read_dir(&runs_dir)
        .with_context(|| format!("failed to read trajectory directory {}", runs_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect::<Vec<PathBuf>>();

    paths.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    if limit > 0 {
        paths.truncate(limit);
    }

    let mut report = AnalysisReport::default();
    for path in paths {
        analyze_run(&path, &mut report)?;
    }
    Ok(report)
}

fn analyze_run(path: &Path, report: &mut AnalysisReport) -> Result<()> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read trajectory {}", path.display()))?;
    report.runs += 1;
    let mut finished = false;

    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(_) => {
                report.parse_errors += 1;
                continue;
            }
        };
        apply_event(&event, report, &mut finished);
    }

    if !finished {
        report.incomplete += 1;
    }
    Ok(())
}

fn apply_event(event: &Value, report: &mut AnalysisReport, finished: &mut bool) {
    match event.get("type").and_then(Value::as_str) {
        Some("run_started") => {
            if let Some(area) = event.get("area").and_then(Value::as_str) {
                increment(&mut report.areas, area);
            }
        }
        Some("attempt_started") => report.attempts += 1,
        Some("agent_finished") => {
            if event.get("success").and_then(Value::as_bool) == Some(false) {
                report.agent_failures += 1;
            }
        }
        Some("failure_recorded") => {
            if let Some(class) = event.get("class").and_then(Value::as_str) {
                increment(&mut report.failure_classes, class);
            }
        }
        Some("scope_evaluated") => {
            let violations = strings(event.get("violations"));
            if !violations.is_empty() {
                report.scope_violation_events += 1;
                for path in violations {
                    increment(&mut report.scope_paths, &path);
                }
            }
        }
        Some("invariant_impact_assessed") => {
            for id in strings(event.get("newly_required")) {
                increment(&mut report.invariant_expansions, &id);
            }
        }
        Some("risk_assessed") => {
            for finding in strings(event.get("findings")) {
                if let Some(code) = finding.split_whitespace().next() {
                    increment(&mut report.risk_codes, code);
                }
            }
        }
        Some("check_finished") => {
            if event.get("success").and_then(Value::as_bool) == Some(false) {
                if let Some(name) = event.get("name").and_then(Value::as_str) {
                    increment(&mut report.failed_checks, name);
                }
            }
        }
        Some("run_finished") => {
            *finished = true;
            if event.get("success").and_then(Value::as_bool) == Some(true) {
                report.passed += 1;
            } else {
                report.failed += 1;
            }
        }
        _ => {}
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn increment(map: &mut BTreeMap<String, usize>, key: &str) {
    *map.entry(key.to_owned()).or_default() += 1;
}

fn append_ranked(output: &mut String, title: &str, values: &BTreeMap<String, usize>) {
    if values.is_empty() {
        return;
    }
    output.push('\n');
    output.push_str(title);
    output.push_str(":\n");
    for (key, count) in ranked(values, 8) {
        output.push_str(&format!("- {count}x {key}\n"));
    }
}

fn ranked(values: &BTreeMap<String, usize>, limit: usize) -> Vec<(String, usize)> {
    let mut items = values
        .iter()
        .map(|(key, count)| (key.clone(), *count))
        .collect::<Vec<_>>();
    items.sort_by(|(left_key, left_count), (right_key, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_key.cmp(right_key))
    });
    items.truncate(limit);
    items
}

fn collect_repeated(
    output: &mut Vec<String>,
    kind: &str,
    values: &BTreeMap<String, usize>,
    threshold: usize,
) {
    for (key, count) in ranked(values, usize::MAX) {
        if count >= threshold {
            output.push(format!("{kind}: {key} ({count}x)"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aggregates_burncloud_failure_signals() {
        let mut report = AnalysisReport::default();
        let mut finished = false;

        apply_event(
            &json!({"type":"run_started","area":"router"}),
            &mut report,
            &mut finished,
        );
        apply_event(
            &json!({"type":"attempt_started","attempt":1}),
            &mut report,
            &mut finished,
        );
        apply_event(
            &json!({"type":"failure_recorded","attempt":1,"class":"verification","detail":"billing failed"}),
            &mut report,
            &mut finished,
        );
        apply_event(
            &json!({"type":"invariant_impact_assessed","newly_required":["INV-BILLING-001"]}),
            &mut report,
            &mut finished,
        );
        apply_event(
            &json!({"type":"risk_assessed","findings":["ASSERTION_WEAKENING [review] crates/router/tests/quota_tests.rs: weaker"]}),
            &mut report,
            &mut finished,
        );
        apply_event(
            &json!({"type":"check_finished","name":"billing-invariants","success":false}),
            &mut report,
            &mut finished,
        );
        apply_event(
            &json!({"type":"run_finished","success":false}),
            &mut report,
            &mut finished,
        );

        assert_eq!(report.areas["router"], 1);
        assert_eq!(report.attempts, 1);
        assert_eq!(report.failure_classes["verification"], 1);
        assert_eq!(report.invariant_expansions["INV-BILLING-001"], 1);
        assert_eq!(report.risk_codes["ASSERTION_WEAKENING"], 1);
        assert_eq!(report.failed_checks["billing-invariants"], 1);
        assert_eq!(report.failed, 1);
        assert!(finished);
    }

    #[test]
    fn ranking_is_count_first_then_name() {
        let values = BTreeMap::from([
            ("z".to_owned(), 2usize),
            ("b".to_owned(), 3usize),
            ("a".to_owned(), 3usize),
        ]);
        assert_eq!(
            ranked(&values, 3),
            vec![("a".into(), 3), ("b".into(), 3), ("z".into(), 2)]
        );
    }
}
