use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::event_writer::EVENT_SCHEMA_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSource {
    EventStream,
    Trajectory,
}

impl RunSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EventStream => "events",
            Self::Trajectory => "trajectory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArtifact {
    pub run_id: String,
    pub path: PathBuf,
    pub source: RunSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayEvent {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReplay {
    pub run_id: String,
    pub task: String,
    pub status: String,
    pub source: RunSource,
    pub events: Vec<ReplayEvent>,
}

pub fn discover(state_dir: &Path) -> Result<Vec<RunArtifact>> {
    let runs_dir = state_dir.join("runs");
    if !runs_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut runs = BTreeMap::new();
    for entry in fs::read_dir(&runs_dir)
        .with_context(|| format!("failed to read run history {}", runs_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name == "latest" {
            continue;
        }

        if path.is_dir() {
            let events_path = path.join("events.jsonl");
            if events_path.is_file() {
                runs.insert(
                    name.clone(),
                    RunArtifact {
                        run_id: name,
                        path: events_path,
                        source: RunSource::EventStream,
                    },
                );
            }
            continue;
        }

        if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            let Some(run_id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            runs.entry(run_id.clone()).or_insert(RunArtifact {
                run_id,
                path,
                source: RunSource::Trajectory,
            });
        }
    }

    Ok(runs.into_values().rev().collect())
}

pub fn resolve(state_dir: &Path, requested_run: Option<&str>) -> Result<RunArtifact> {
    let runs = discover(state_dir)?;
    if let Some(run_id) = requested_run {
        if let Some(run) = runs.into_iter().find(|run| run.run_id == run_id) {
            return Ok(run);
        }
        bail!("Harness run {run_id} was not found");
    }

    runs.into_iter()
        .next()
        .context("no Harness runs found; execute a task before opening replay")
}

pub fn load(artifact: &RunArtifact) -> Result<RunReplay> {
    let content = fs::read_to_string(&artifact.path)
        .with_context(|| format!("failed to read run {}", artifact.path.display()))?;
    let complete_file = content.ends_with('\n');
    let line_count = content.lines().count();
    let mut task = "unknown".to_owned();
    let mut stage = "TASK".to_owned();
    let mut final_status = None;
    let mut events = Vec::new();
    let mut previous_sequence = 0;

    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) if !complete_file && index + 1 == line_count => break,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "invalid JSONL in {} at line {}",
                        artifact.path.display(),
                        index + 1
                    )
                });
            }
        };

        if artifact.source == RunSource::EventStream {
            validate_event_record(&value, &artifact.path, index + 1, &mut previous_sequence)?;
        }

        let payload = match artifact.source {
            RunSource::EventStream => value.get("event").unwrap_or(&value),
            RunSource::Trajectory => &value,
        };
        let name = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        if let Some(value) = payload.get("task").and_then(Value::as_str) {
            task = value.to_owned();
        }

        if matches!(name, "task_finished" | "run_finished") {
            if let Some(success) = payload.get("success").and_then(Value::as_bool) {
                final_status = Some(if success { "PASSED" } else { "FAILED" }.to_owned());
            }
        } else {
            stage = stage_for(name).to_owned();
        }

        events.push(ReplayEvent {
            name: name.to_owned(),
            detail: detail_for(payload, name),
        });
    }

    Ok(RunReplay {
        run_id: artifact.run_id.clone(),
        task,
        status: final_status.unwrap_or(stage),
        source: artifact.source,
        events,
    })
}

fn validate_event_record(
    value: &Value,
    path: &Path,
    line: usize,
    previous_sequence: &mut u64,
) -> Result<()> {
    if let Some(version) = value.get("schema_version").and_then(Value::as_u64) {
        if version != u64::from(EVENT_SCHEMA_VERSION) {
            bail!(
                "unsupported Harness event schema {version} in {} at line {line}; supported schema is {}",
                path.display(),
                EVENT_SCHEMA_VERSION
            );
        }
    }

    if let Some(sequence) = value.get("sequence").and_then(Value::as_u64) {
        if sequence <= *previous_sequence {
            bail!(
                "non-monotonic Harness event sequence {sequence} in {} at line {line}",
                path.display()
            );
        }
        *previous_sequence = sequence;
    }

    Ok(())
}

fn stage_for(event: &str) -> &'static str {
    match event {
        "task_started" | "contract_loaded" | "run_started" => "TASK",
        "route_selected" | "invariant_selected" | "task_routed" | "invariants_selected" => "ROUTE",
        "loop_started" | "agent_started" | "agent_finished" | "attempt_started"
        | "attempt_failed" | "retry_requested" | "failure_recorded" => "AGENT",
        "diff_detected"
        | "git_head_checked"
        | "scope_evaluated"
        | "invariant_impact_assessed"
        | "invariant_expanded" => "SCOPE",
        "risk_assessed" | "risk_detected" => "RISK",
        "verification_started" | "verification_finished" | "check_finished" => "VERIFY",
        _ => "TASK",
    }
}

fn detail_for(payload: &Value, name: &str) -> String {
    if let Some(reason) = payload.get("reason").and_then(Value::as_str) {
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
            return format!("new invariants: {}", values.join(", "));
        }
    }
    if let Some(check) = payload.get("check").and_then(Value::as_str) {
        return check.to_owned();
    }
    if let Some(check) = payload.get("name").and_then(Value::as_str) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_state(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "burncloud-harness-history-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn discovers_event_runs_and_legacy_trajectories() {
        let state = temp_state("discover");
        let runs = state.join("runs");
        fs::create_dir_all(runs.join("200")).unwrap();
        fs::create_dir_all(runs.join("100")).unwrap();
        fs::write(runs.join("200/events.jsonl"), "").unwrap();
        fs::write(runs.join("100/events.jsonl"), "").unwrap();
        fs::write(runs.join("100.jsonl"), "").unwrap();
        fs::write(runs.join("050.jsonl"), "").unwrap();

        let discovered = discover(&state).unwrap();
        assert_eq!(discovered.len(), 3);
        assert_eq!(discovered[0].run_id, "200");
        assert_eq!(discovered[1].run_id, "100");
        assert_eq!(discovered[1].source, RunSource::EventStream);
        assert_eq!(discovered[2].run_id, "050");
        assert_eq!(discovered[2].source, RunSource::Trajectory);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn loads_nested_event_stream_replay() {
        let state = temp_state("events");
        let path = state.join("events.jsonl");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"schema_version\":1,\"sequence\":1,\"timestamp_ms\":1,\"event\":{\"type\":\"task_started\",\"run_id\":\"200\",\"task\":\"buyer-overview\",\"area\":\"ui\"}}\n",
                "{\"schema_version\":1,\"sequence\":2,\"timestamp_ms\":2,\"event\":{\"type\":\"retry_requested\",\"attempt\":1,\"reason\":\"scope expanded\"}}\n",
                "{\"schema_version\":1,\"sequence\":3,\"timestamp_ms\":3,\"event\":{\"type\":\"task_finished\",\"success\":true,\"attempts\":1}}\n"
            ),
        )
        .unwrap();
        let replay = load(&RunArtifact {
            run_id: "200".to_owned(),
            path,
            source: RunSource::EventStream,
        })
        .unwrap();

        assert_eq!(replay.task, "buyer-overview");
        assert_eq!(replay.status, "PASSED");
        assert_eq!(replay.events[1].detail, "scope expanded");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn ignores_partial_last_event_while_tailing() {
        let state = temp_state("partial-tail");
        let path = state.join("events.jsonl");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"schema_version\":1,\"sequence\":1,\"timestamp_ms\":1,\"event\":{\"type\":\"task_started\",\"run_id\":\"200\",\"task\":\"live-task\",\"area\":\"ui\"}}\n",
                "{\"schema_version\":1,\"sequence\":2,\"timestamp_ms\":2,\"event\":{\"type\":\"risk_detected\""
            ),
        )
        .unwrap();

        let replay = load(&RunArtifact {
            run_id: "200".to_owned(),
            path,
            source: RunSource::EventStream,
        })
        .unwrap();
        assert_eq!(replay.task, "live-task");
        assert_eq!(replay.events.len(), 1);
        assert_eq!(replay.events[0].name, "task_started");
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn rejects_future_event_schema() {
        let state = temp_state("future-schema");
        let path = state.join("events.jsonl");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            &path,
            "{\"schema_version\":99,\"sequence\":1,\"event\":{\"type\":\"task_started\"}}\n",
        )
        .unwrap();

        let error = load(&RunArtifact {
            run_id: "future".to_owned(),
            path,
            source: RunSource::EventStream,
        })
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("unsupported Harness event schema"));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn loads_trajectory_as_legacy_replay() {
        let state = temp_state("trajectory");
        let path = state.join("100.jsonl");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"ts_ms\":1,\"type\":\"run_started\",\"run_id\":\"100\",\"task\":\"legacy-task\"}\n",
                "{\"ts_ms\":2,\"type\":\"risk_assessed\",\"attempt\":1,\"findings\":[]}\n"
            ),
        )
        .unwrap();
        let replay = load(&RunArtifact {
            run_id: "100".to_owned(),
            path,
            source: RunSource::Trajectory,
        })
        .unwrap();

        assert_eq!(replay.task, "legacy-task");
        assert_eq!(replay.status, "RISK");
        assert_eq!(replay.source, RunSource::Trajectory);
        fs::remove_dir_all(state).unwrap();
    }
}
