use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEvent {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActivity {
    pub attempt: u32,
    pub stream: String,
    pub line: String,
    pub timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckState {
    pub name: String,
    pub success: Option<bool>,
    pub started_ms: Option<u64>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureState {
    pub attempt: u32,
    pub class: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageTiming {
    pub attempt: u32,
    pub stage: String,
    pub started_ms: u64,
    pub duration_ms: Option<u64>,
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
    pub agent_activity: Vec<AgentActivity>,
    pub agent_last_output_ms: Option<u64>,
    pub agent_heartbeat_elapsed_secs: Option<u64>,
    pub timings: Vec<StageTiming>,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
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
            agent_activity: Vec::new(),
            agent_last_output_ms: None,
            agent_heartbeat_elapsed_secs: None,
            timings: Vec::new(),
            started_ms: None,
            finished_ms: None,
        }
    }
}

impl RunState {
    pub fn apply(&mut self, payload: &Value) {
        self.apply_at(payload, None);
    }

    pub fn apply_at(&mut self, payload: &Value, timestamp_ms: Option<u64>) {
        let name = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        if self.started_ms.is_none() {
            self.started_ms = timestamp_ms;
        }
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

        let next_stage = if matches!(name, "task_finished" | "run_finished") {
            "DONE"
        } else if name == "stage_started" {
            payload
                .get("stage")
                .and_then(Value::as_str)
                .unwrap_or("TASK")
        } else {
            stage_for(name)
        };
        self.transition_stage(next_stage, timestamp_ms);

        if matches!(name, "task_finished" | "run_finished") {
            if let Some(success) = payload.get("success").and_then(Value::as_bool) {
                self.status = if success { "PASSED" } else { "FAILED" }.to_owned();
                self.finished_ms = timestamp_ms;
            }
        } else {
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
                self.agent_activity.clear();
                self.agent_last_output_ms = None;
                self.agent_heartbeat_elapsed_secs = None;
            }
            "agent_output" => {
                let line = payload
                    .get("line")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !line.trim().is_empty() {
                    self.agent_activity.push(AgentActivity {
                        attempt: u32_field(payload, "attempt").unwrap_or(self.attempt),
                        stream: payload
                            .get("stream")
                            .and_then(Value::as_str)
                            .unwrap_or("stdout")
                            .to_owned(),
                        line: line.to_owned(),
                        timestamp_ms,
                    });
                    if self.agent_activity.len() > 50 {
                        let remove = self.agent_activity.len() - 50;
                        self.agent_activity.drain(0..remove);
                    }
                    self.agent_last_output_ms = timestamp_ms;
                }
            }
            "agent_heartbeat" => {
                self.agent_heartbeat_elapsed_secs = payload.get("elapsed_secs").and_then(Value::as_u64);
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
            "verification_started" | "check_started" => {
                if let Some(name) = check_name(payload) {
                    upsert_check(&mut self.checks, name, None, timestamp_ms, None);
                }
            }
            "verification_finished" => {
                if let Some(name) = check_name(payload) {
                    finish_check(
                        &mut self.checks,
                        name,
                        payload.get("success").and_then(Value::as_bool),
                        timestamp_ms,
                    );
                }
            }
            "check_finished" => {
                if let Some(name) = check_name(payload) {
                    finish_check(
                        &mut self.checks,
                        name,
                        payload.get("success").and_then(Value::as_bool),
                        timestamp_ms,
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

        if !matches!(name, "agent_output" | "agent_heartbeat") {
            self.timeline.push(TimelineEvent {
                name: name.to_owned(),
                detail: detail_for(payload, name),
            });
        }
    }

    pub fn total_elapsed_ms(&self, now_ms: u64) -> Option<u64> {
        let started = self.started_ms?;
        Some(self.finished_ms.unwrap_or(now_ms).saturating_sub(started))
    }

    pub fn stage_elapsed_ms(&self, stage: &str, attempt: u32, now_ms: u64) -> Option<u64> {
        let mut found = false;
        let mut total = 0_u64;
        for timing in &self.timings {
            if timing.stage == stage && timing.attempt == attempt {
                found = true;
                total = total.saturating_add(
                    timing
                        .duration_ms
                        .unwrap_or_else(|| now_ms.saturating_sub(timing.started_ms)),
                );
            }
        }
        found.then_some(total)
    }

    fn transition_stage(&mut self, next_stage: &str, timestamp_ms: Option<u64>) {
        if self.stage == next_stage {
            return;
        }

        if let Some(timestamp_ms) = timestamp_ms {
            if let Some(active) = self
                .timings
                .iter_mut()
                .rev()
                .find(|timing| timing.duration_ms.is_none())
            {
                active.duration_ms = Some(timestamp_ms.saturating_sub(active.started_ms));
            }
            if next_stage != "DONE" {
                self.timings.push(StageTiming {
                    attempt: self.attempt,
                    stage: next_stage.to_owned(),
                    started_ms: timestamp_ms,
                    duration_ms: None,
                });
            }
        }
        self.stage = next_stage.to_owned();
    }
}

fn upsert_check(
    checks: &mut Vec<CheckState>,
    name: &str,
    success: Option<bool>,
    started_ms: Option<u64>,
    duration_ms: Option<u64>,
) {
    if let Some(check) = checks.iter_mut().find(|check| check.name == name) {
        check.success = success;
        if check.started_ms.is_none() {
            check.started_ms = started_ms;
        }
        if duration_ms.is_some() {
            check.duration_ms = duration_ms;
        }
    } else {
        checks.push(CheckState {
            name: name.to_owned(),
            success,
            started_ms,
            duration_ms,
        });
    }
}

fn finish_check(
    checks: &mut Vec<CheckState>,
    name: &str,
    success: Option<bool>,
    finished_ms: Option<u64>,
) {
    if let Some(check) = checks.iter_mut().find(|check| check.name == name) {
        check.success = success;
        if let (Some(started), Some(finished)) = (check.started_ms, finished_ms) {
            check.duration_ms = Some(finished.saturating_sub(started));
        }
    } else {
        upsert_check(checks, name, success, None, None);
    }
}

fn check_name(payload: &Value) -> Option<&str> {
    payload
        .get("check")
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
}

fn stage_for(event: &str) -> &'static str {
    match event {
        "task_started" | "contract_loaded" | "run_started" => "TASK",
        "route_selected" | "invariant_selected" | "task_routed" | "invariants_selected" => "ROUTE",
        "loop_started" | "agent_started" | "agent_output" | "agent_heartbeat" | "agent_finished"
        | "attempt_started" => "AGENT",
        "diff_detected" | "git_head_checked" | "scope_evaluated" => "SCOPE",
        "invariant_impact_assessed" | "invariant_expanded" => "INVARIANTS",
        "risk_assessed" | "risk_detected" => "RISK",
        "verification_started" | "verification_finished" | "check_started" | "check_finished" => {
            "VERIFY"
        }
        "attempt_failed" | "retry_requested" | "failure_recorded" => "FEEDBACK",
        _ => "TASK",
    }
}

fn detail_for(payload: &Value, name: &str) -> String {
    if name == "stage_started" {
        return payload
            .get("stage")
            .and_then(Value::as_str)
            .unwrap_or("stage")
            .to_owned();
    }
    if name == "agent_output" {
        return payload
            .get("line")
            .and_then(Value::as_str)
            .unwrap_or("agent output")
            .to_owned();
    }
    if name == "agent_heartbeat" {
        return payload
            .get("elapsed_secs")
            .and_then(Value::as_u64)
            .map(|seconds| format!("agent alive for {seconds}s"))
            .unwrap_or_else(|| "agent heartbeat".to_owned());
    }
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
    if let Some(check) = check_name(payload) {
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
        state.apply_at(
            &json!({
                "type": "task_started",
                "run_id": "run-1",
                "task": "buyer-overview",
                "area": "ui"
            }),
            Some(1_000),
        );
        state.apply_at(
            &json!({
                "type": "contract_loaded",
                "allowed_scope": ["crates/client/**"],
                "avoid_scope": ["crates/server/**"],
                "max_loops": 4
            }),
            Some(1_010),
        );
        state.apply_at(
            &json!({
                "type": "route_selected",
                "routes": ["buyer-overview", "shared-layout"]
            }),
            Some(1_020),
        );
        state.apply_at(
            &json!({"type": "stage_started", "attempt": 1, "stage": "AGENT"}),
            Some(1_100),
        );
        state.apply_at(
            &json!({
                "type": "diff_detected",
                "attempt": 1,
                "changed_files": ["src/main.rs"]
            }),
            Some(2_100),
        );
        state.apply_at(
            &json!({
                "type": "risk_detected",
                "attempt": 1,
                "findings": ["review required"]
            }),
            Some(2_200),
        );
        state.apply_at(
            &json!({
                "type": "verification_started",
                "attempt": 1,
                "check": "cargo test"
            }),
            Some(2_300),
        );
        state.apply_at(
            &json!({
                "type": "verification_finished",
                "attempt": 1,
                "check": "cargo test",
                "success": true
            }),
            Some(2_800),
        );
        state.apply_at(
            &json!({
                "type": "task_finished",
                "success": true,
                "attempts": 1
            }),
            Some(2_900),
        );

        assert_eq!(state.run_id, "run-1");
        assert_eq!(state.task, "buyer-overview");
        assert_eq!(state.max_loops, 4);
        assert_eq!(state.allowed, vec!["crates/client/**"]);
        assert_eq!(state.avoid, vec!["crates/server/**"]);
        assert_eq!(state.routes.len(), 2);
        assert_eq!(state.changed_files, vec!["src/main.rs"]);
        assert_eq!(state.risk_findings, vec!["review required"]);
        assert_eq!(state.checks[0].success, Some(true));
        assert_eq!(state.checks[0].duration_ms, Some(500));
        assert_eq!(state.stage_elapsed_ms("AGENT", 1, 2_900), Some(1_000));
        assert_eq!(state.total_elapsed_ms(2_900), Some(1_900));
        assert_eq!(state.status, "PASSED");
        assert_eq!(state.stage, "DONE");
    }

    #[test]
    fn reducer_tracks_live_agent_activity_without_flooding_timeline() {
        let mut state = RunState::default();
        state.apply_at(
            &json!({"type": "task_started", "run_id": "run-1"}),
            Some(1_000),
        );
        state.apply_at(
            &json!({"type": "loop_started", "attempt": 1}),
            Some(1_010),
        );
        state.apply_at(
            &json!({
                "type": "agent_output",
                "attempt": 1,
                "stream": "stdout",
                "line": "Reading crates/client/src/lib.rs"
            }),
            Some(2_000),
        );
        state.apply_at(
            &json!({
                "type": "agent_heartbeat",
                "attempt": 1,
                "elapsed_secs": 5
            }),
            Some(6_000),
        );

        assert_eq!(state.stage, "AGENT");
        assert_eq!(state.agent_activity.len(), 1);
        assert_eq!(state.agent_activity[0].stream, "stdout");
        assert_eq!(state.agent_last_output_ms, Some(2_000));
        assert_eq!(state.agent_heartbeat_elapsed_secs, Some(5));
        assert_eq!(state.timeline.len(), 2);
    }

    #[test]
    fn reducer_tracks_live_stage_elapsed_time() {
        let mut state = RunState::default();
        state.apply_at(
            &json!({"type": "task_started", "run_id": "run-1"}),
            Some(1_000),
        );
        state.apply_at(
            &json!({"type": "stage_started", "attempt": 2, "stage": "AGENT"}),
            Some(2_000),
        );

        assert_eq!(state.stage, "AGENT");
        assert_eq!(state.stage_elapsed_ms("AGENT", 2, 7_500), Some(5_500));
        assert_eq!(state.total_elapsed_ms(7_500), Some(6_500));
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
        state.apply_at(
            &json!({
                "type": "run_started",
                "run_id": "legacy",
                "task": "legacy-task",
                "area": "api",
                "max_loops": 3,
                "allowed": ["src/**"],
                "avoid": ["target/**"]
            }),
            Some(10),
        );
        state.apply_at(
            &json!({
                "type": "scope_evaluated",
                "attempt": 1,
                "changed_paths": ["src/lib.rs"],
                "violations": []
            }),
            Some(20),
        );
        state.apply_at(
            &json!({
                "type": "check_finished",
                "attempt": 1,
                "name": "cargo test",
                "success": false
            }),
            Some(30),
        );

        assert_eq!(state.changed_files, vec!["src/lib.rs"]);
        assert_eq!(state.allowed, vec!["src/**"]);
        assert_eq!(state.max_loops, 3);
        assert_eq!(state.checks[0].name, "cargo test");
        assert_eq!(state.checks[0].success, Some(false));
        assert_eq!(state.stage, "VERIFY");
    }
}
