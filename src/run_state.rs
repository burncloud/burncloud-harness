use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEvent {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckState {
    pub name: String,
    pub success: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureState {
    pub attempt: u32,
    pub class: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunState {
    pub run_id: String,
    pub task: String,
    pub area: String,
    pub stage: String,
    pub status: String,
    pub attempt: u32,
    pub max_loops: u32,
    pub allowed: Vec<String>,
    pub avoid: Vec<String>,
    pub routes: Vec<String>,
    pub changed_files: Vec<String>,
    pub violations: Vec<String>,
    pub invariants: Vec<String>,
    pub newly_required: Vec<String>,
    pub risk_findings: Vec<String>,
    pub checks: Vec<CheckState>,
    pub failures: Vec<FailureState>,
    pub timeline: Vec<TimelineEvent>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            run_id: String::new(),
            task: "unknown".to_owned(),
            area: "unknown".to_owned(),
            stage: "TASK".to_owned(),
            status: "TASK".to_owned(),
            attempt: 0,
            max_loops: 0,
            allowed: Vec::new(),
            avoid: Vec::new(),
            routes: Vec::new(),
            changed_files: Vec::new(),
            violations: Vec::new(),
            invariants: Vec::new(),
            newly_required: Vec::new(),
            risk_findings: Vec::new(),
            checks: Vec::new(),
            failures: Vec::new(),
            timeline: Vec::new(),
        }
    }
}

impl RunState {
    pub fn apply(&mut self, payload: &Value) {
        let name = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        if let Some(run_id) = payload.get("run_id").and_then(Value::as_str) {
            self.run_id = run_id.to_owned();
        }
        if let Some(task) = payload.get("task").and_then(Value::as_str) {
            self.task = task.to_owned();
        }
        if let Some(area) = payload.get("area").and_then(Value::as_str) {
            self.area = area.to_owned();
        }
        if let Some(attempt) = u32_field(payload, "attempt") {
            self.attempt = attempt;
        }
        if let Some(max_loops) = u32_field(payload, "max_loops") {
            self.max_loops = max_loops;
        }

        if matches!(name, "task_finished" | "run_finished") {
            if let Some(success) = payload.get("success").and_then(Value::as_bool) {
                self.stage = "DONE".to_owned();
                self.status = if success { "PASSED" } else { "FAILED" }.to_owned();
            }
        } else {
            self.stage = stage_for(name).to_owned();
            self.status = self.stage.clone();
        }

        match name {
            "contract_loaded" => {
                self.allowed = string_array(payload, "allowed_scope").unwrap_or_default();
                self.avoid = string_array(payload, "avoid_scope").unwrap_or_default();
            }
            "run_started" => {
                self.allowed = string_array(payload, "allowed").unwrap_or_default();
                self.avoid = string_array(payload, "avoid").unwrap_or_default();
            }
            "route_selected" | "task_routed" => {
                self.routes = string_array(payload, "routes").unwrap_or_default();
            }
            "loop_started" | "attempt_started" => {
                self.newly_required.clear();
                self.violations.clear();
                self.risk_findings.clear();
                self.checks.clear();
            }
            "invariant_selected" | "invariants_selected" => {
                self.invariants = string_array(payload, "invariants").unwrap_or_default();
                self.newly_required.clear();
            }
            "invariant_expanded" => {
                let expanded = string_array(payload, "invariants").unwrap_or_default();
                self.newly_required = expanded.clone();
                for invariant in expanded {
                    if !self.invariants.contains(&invariant) {
                        self.invariants.push(invariant);
                    }
                }
            }
            "diff_detected" => {
                self.changed_files = string_array(payload, "changed_files").unwrap_or_default();
            }
            "scope_evaluated" => {
                self.changed_files = string_array(payload, "changed_paths")
                    .unwrap_or_else(|| self.changed_files.clone());
                self.violations = string_array(payload, "violations").unwrap_or_default();
            }
            "risk_detected" | "risk_assessed" => {
                self.risk_findings = string_array(payload, "findings").unwrap_or_default();
            }
            "verification_started" => {
                if let Some(name) = payload.get("check").and_then(Value::as_str) {
                    upsert_check(&mut self.checks, name, None);
                }
            }
            "verification_finished" => {
                if let Some(name) = payload.get("check").and_then(Value::as_str) {
                    upsert_check(
                        &mut self.checks,
                        name,
                        payload.get("success").and_then(Value::as_bool),
                    );
                }
            }
            "check_finished" => {
                if let Some(name) = payload.get("name").and_then(Value::as_str) {
                    upsert_check(
                        &mut self.checks,
                        name,
                        payload.get("success").and_then(Value::as_bool),
                    );
                }
            }
            "failure_recorded" => {
                self.failures.push(FailureState {
                    attempt: u32_field(payload, "attempt").unwrap_or(self.attempt),
                    class: payload
                        .get("class")
                        .and_then(Value::as_str)
                        .unwrap_or("failure")
                        .to_owned(),
                    detail: payload
                        .get("detail")
                        .and_then(Value::as_str)
                        .unwrap_or("failure recorded")
                        .to_owned(),
                });
            }
            "retry_requested" | "attempt_failed" => {
                let detail = payload
                    .get("reason")
                    .or_else(|| payload.get("feedback"))
                    .and_then(Value::as_str)
                    .unwrap_or("retry requested")
                    .to_owned();
                if !self
                    .failures
                    .last()
                    .is_some_and(|failure| failure.detail == detail)
                {
                    self.failures.push(FailureState {
                        attempt: u32_field(payload, "attempt").unwrap_or(self.attempt),
                        class: "retry".to_owned(),
                        detail,
                    });
                }
            }
            _ => {}
        }

        self.timeline.push(TimelineEvent {
            name: name.to_owned(),
            detail: detail_for(payload, name),
        });
    }
}

fn upsert_check(checks: &mut Vec<CheckState>, name: &str, success: Option<bool>) {
    if let Some(check) = checks.iter_mut().find(|check| check.name == name) {
        check.success = success;
    } else {
        checks.push(CheckState {
            name: name.to_owned(),
            success,
        });
    }
}

fn stage_for(event: &str) -> &'static str {
    match event {
        "task_started" | "contract_loaded" | "run_started" => "TASK",
        "route_selected" | "invariant_selected" | "task_routed" | "invariants_selected" => "ROUTE",
        "loop_started" | "agent_started" | "agent_finished" | "attempt_started" => "AGENT",
        "diff_detected" | "git_head_checked" | "scope_evaluated" => "SCOPE",
        "invariant_impact_assessed" | "invariant_expanded" => "INVARIANTS",
        "risk_assessed" | "risk_detected" => "RISK",
        "verification_started" | "verification_finished" | "check_finished" => "VERIFY",
        "attempt_failed" | "retry_requested" | "failure_recorded" => "FEEDBACK",
        _ => "TASK",
    }
}

fn detail_for(payload: &Value, name: &str) -> String {
    if let Some(reason) = payload
        .get("reason")
        .or_else(|| payload.get("feedback"))
        .and_then(Value::as_str)
    {
        return reason.to_owned();
    }
    if let Some(detail) = payload.get("detail").and_then(Value::as_str) {
        if let Some(class) = payload.get("class").and_then(Value::as_str) {
            return format!("{class}: {detail}");
        }
        return detail.to_owned();
    }
    if let Some(values) = string_array(payload, "violations") {
        return if values.is_empty() {
            "scope accepted".to_owned()
        } else {
            format!("scope violations: {}", values.join(", "))
        };
    }
    if let Some(values) = string_array(payload, "findings") {
        return if values.is_empty() {
            "no risk findings".to_owned()
        } else {
            values.join("; ")
        };
    }
    if let Some(values) = string_array(payload, "invariants") {
        if !values.is_empty() {
            return format!("invariants: {}", values.join(", "));
        }
    }
    if let Some(check) = payload
        .get("check")
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
    {
        return check.to_owned();
    }
    if let Some(attempt) = payload.get("attempt").and_then(Value::as_u64) {
        return format!("attempt {attempt}");
    }
    name.replace('_', " ")
}

fn string_array(value: &Value, key: &str) -> Option<Vec<String>> {
    value
        .get(key)?
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

fn u32_field(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reducer_tracks_contract_decisions_checks_and_final_status() {
        let mut state = RunState::default();
        state.apply(&json!({
            "type": "task_started",
            "run_id": "run-1",
            "task": "buyer-overview",
            "area": "ui"
        }));
        state.apply(&json!({
            "type": "contract_loaded",
            "allowed_scope": ["crates/client/**"],
            "avoid_scope": ["crates/server/**"],
            "max_loops": 4
        }));
        state.apply(&json!({
            "type": "route_selected",
            "routes": ["buyer-overview", "shared-layout"]
        }));
        state.apply(&json!({
            "type": "diff_detected",
            "attempt": 1,
            "changed_files": ["src/main.rs"]
        }));
        state.apply(&json!({
            "type": "risk_detected",
            "attempt": 1,
            "findings": ["review required"]
        }));
        state.apply(&json!({
            "type": "failure_recorded",
            "attempt": 1,
            "class": "risk_review",
            "detail": "review required"
        }));
        state.apply(&json!({
            "type": "verification_finished",
            "attempt": 2,
            "check": "cargo test",
            "success": true
        }));
        state.apply(&json!({
            "type": "task_finished",
            "success": true,
            "attempts": 2
        }));

        assert_eq!(state.run_id, "run-1");
        assert_eq!(state.task, "buyer-overview");
        assert_eq!(state.max_loops, 4);
        assert_eq!(state.allowed, vec!["crates/client/**"]);
        assert_eq!(state.avoid, vec!["crates/server/**"]);
        assert_eq!(state.routes.len(), 2);
        assert_eq!(state.changed_files, vec!["src/main.rs"]);
        assert_eq!(state.risk_findings, vec!["review required"]);
        assert_eq!(state.failures.len(), 1);
        assert_eq!(state.checks[0].success, Some(true));
        assert_eq!(state.status, "PASSED");
        assert_eq!(state.stage, "DONE");
        assert_eq!(state.timeline.len(), 8);
    }

    #[test]
    fn reducer_marks_retry_as_feedback_stage() {
        let mut state = RunState::default();
        state.apply(&json!({
            "type": "retry_requested",
            "attempt": 2,
            "reason": "verification failed"
        }));

        assert_eq!(state.stage, "FEEDBACK");
        assert_eq!(state.failures[0].detail, "verification failed");
    }

    #[test]
    fn reducer_supports_legacy_trajectory_fields() {
        let mut state = RunState::default();
        state.apply(&json!({
            "type": "run_started",
            "run_id": "legacy",
            "task": "legacy-task",
            "area": "api",
            "max_loops": 3,
            "allowed": ["src/**"],
            "avoid": ["target/**"]
        }));
        state.apply(&json!({
            "type": "scope_evaluated",
            "attempt": 1,
            "changed_paths": ["src/lib.rs"],
            "violations": []
        }));
        state.apply(&json!({
            "type": "check_finished",
            "attempt": 1,
            "name": "cargo test",
            "success": false
        }));

        assert_eq!(state.changed_files, vec!["src/lib.rs"]);
        assert_eq!(state.allowed, vec!["src/**"]);
        assert_eq!(state.max_loops, 3);
        assert_eq!(state.checks[0].name, "cargo test");
        assert_eq!(state.checks[0].success, Some(false));
        assert_eq!(state.stage, "VERIFY");
    }
}
