use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{event_writer::EVENT_SCHEMA_VERSION, run_state::RunState};

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
pub struct RunReplay {
    pub source: RunSource,
    pub state: RunState,
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
    let mut previous_sequence = 0;
    let mut state = RunState::default();
    state.run_id = artifact.run_id.clone();

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
        let timestamp_ms = match artifact.source {
            RunSource::EventStream => value.get("timestamp_ms").and_then(Value::as_u64),
            RunSource::Trajectory => value.get("ts_ms").and_then(Value::as_u64),
        };
        state.apply_at(payload, timestamp_ms);
    }

    Ok(RunReplay {
        source: artifact.source,
        state,
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
    fn event_stream_uses_shared_reducer_and_timestamps() {
        let state = temp_state("events");
        let path = state.join("events.jsonl");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"schema_version\":1,\"sequence\":1,\"timestamp_ms\":1000,\"event\":{\"type\":\"task_started\",\"run_id\":\"200\",\"task\":\"buyer-overview\",\"area\":\"ui\"}}\n",
                "{\"schema_version\":1,\"sequence\":2,\"timestamp_ms\":2000,\"event\":{\"type\":\"stage_started\",\"attempt\":1,\"stage\":\"RISK\"}}\n",
                "{\"schema_version\":1,\"sequence\":3,\"timestamp_ms\":2500,\"event\":{\"type\":\"risk_detected\",\"attempt\":1,\"findings\":[\"review\"]}}\n",
                "{\"schema_version\":1,\"sequence\":4,\"timestamp_ms\":3000,\"event\":{\"type\":\"task_finished\",\"success\":true,\"attempts\":1}}\n"
            ),
        )
        .unwrap();
        let replay = load(&RunArtifact {
            run_id: "200".to_owned(),
            path,
            source: RunSource::EventStream,
        })
        .unwrap();

        assert_eq!(replay.state.task, "buyer-overview");
        assert_eq!(replay.state.status, "PASSED");
        assert_eq!(replay.state.risk_findings, vec!["review"]);
        assert_eq!(replay.state.total_elapsed_ms(9_999), Some(2_000));
        assert_eq!(replay.state.stage_elapsed_ms("RISK", 1, 9_999), Some(1_000));
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
        assert_eq!(replay.state.task, "live-task");
        assert_eq!(replay.state.timeline.len(), 1);
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
    fn legacy_trajectory_uses_same_reducer() {
        let state = temp_state("trajectory");
        let path = state.join("100.jsonl");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"ts_ms\":10,\"type\":\"run_started\",\"run_id\":\"100\",\"task\":\"legacy-task\",\"area\":\"api\"}\n",
                "{\"ts_ms\":20,\"type\":\"risk_assessed\",\"attempt\":1,\"findings\":[]}\n"
            ),
        )
        .unwrap();
        let replay = load(&RunArtifact {
            run_id: "100".to_owned(),
            path,
            source: RunSource::Trajectory,
        })
        .unwrap();

        assert_eq!(replay.state.task, "legacy-task");
        assert_eq!(replay.state.stage, "RISK");
        assert_eq!(replay.source, RunSource::Trajectory);
        fs::remove_dir_all(state).unwrap();
    }
}
