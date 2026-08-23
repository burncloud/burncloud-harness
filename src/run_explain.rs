use crate::run_state::RunState;

pub fn render(state: &RunState) -> String {
    let mut output = String::new();
    output.push_str("BurnCloud Harness Run Explanation\n");
    output.push_str(&format!("Run: {}\n", value_or(&state.run_id, "unknown")));
    output.push_str(&format!("Task: {}\n", state.task));
    output.push_str(&format!("Area: {}\n", state.area));
    output.push_str(&format!("Result: {}\n", state.status));
    output.push_str(&format!("Final stage: {}\n", state.stage));
    output.push_str(&format!("Attempts observed: {}\n", state.attempt));

    output.push_str("\nChanged files:\n");
    if state.changed_files.is_empty() {
        output.push_str("- none recorded\n");
    } else {
        for path in &state.changed_files {
            output.push_str(&format!("- {path}\n"));
        }
    }

    output.push_str("\nWhy the Harness retried or stopped:\n");
    if state.failures.is_empty() {
        output.push_str("- no failure/retry decision recorded\n");
    } else {
        for failure in &state.failures {
            output.push_str(&format!(
                "- attempt {} · {} · {}\n",
                failure.attempt, failure.class, failure.detail
            ));
        }
    }

    output.push_str("\nRisk evidence:\n");
    if state.risk_findings.is_empty() {
        output.push_str("- no risk findings recorded\n");
    } else {
        for risk in &state.risk_findings {
            output.push_str(&format!("- {risk}\n"));
        }
    }

    output.push_str("\nVerification:\n");
    if state.checks.is_empty() {
        output.push_str("- no verification result recorded\n");
    } else {
        for check in &state.checks {
            let status = match check.success {
                Some(true) => "PASS",
                Some(false) => "FAIL",
                None => "RUNNING",
            };
            output.push_str(&format!("- {status} · {}\n", check.name));
        }
    }

    output.push_str("\nRecent event evidence:\n");
    for event in state.timeline.iter().rev().take(12).rev() {
        output.push_str(&format!("- {} · {}\n", event.name, event.detail));
    }

    output
}

fn value_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_state::{CheckState, FailureState, TimelineEvent};

    #[test]
    fn explanation_surfaces_decisions_and_checks() {
        let state = RunState {
            run_id: "run-1".to_owned(),
            task: "buyer-overview".to_owned(),
            area: "ui".to_owned(),
            stage: "DONE".to_owned(),
            status: "PASSED".to_owned(),
            attempt: 2,
            changed_files: vec!["src/main.rs".to_owned()],
            risk_findings: vec!["review required".to_owned()],
            checks: vec![CheckState {
                name: "cargo test".to_owned(),
                success: Some(true),
            }],
            failures: vec![FailureState {
                attempt: 1,
                class: "risk_review".to_owned(),
                detail: "review required".to_owned(),
            }],
            timeline: vec![TimelineEvent {
                name: "task_finished".to_owned(),
                detail: "task finished".to_owned(),
            }],
            ..RunState::default()
        };

        let explanation = render(&state);
        assert!(explanation.contains("Result: PASSED"));
        assert!(explanation.contains("src/main.rs"));
        assert!(explanation.contains("risk_review"));
        assert!(explanation.contains("PASS · cargo test"));
    }
}
