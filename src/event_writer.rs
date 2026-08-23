use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::events::HarnessEvent;

pub const EVENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct EventWriter {
    path: PathBuf,
    sequence: Arc<AtomicU64>,
}

impl EventWriter {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let existing = fs::read_to_string(&path)
            .map(|content| content.lines().count() as u64)
            .unwrap_or(0);
        Self {
            path,
            sequence: Arc::new(AtomicU64::new(existing)),
        }
    }

    pub fn append(&self, event: &HarnessEvent) -> Result<()> {
        let sequence = self.next_sequence();
        let line = serde_json::to_string(&EventRecord::new(event, sequence))?;
        self.append_line(&line)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn append_line(&self, line: &str) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RunEventWriter {
    primary: EventWriter,
    latest: EventWriter,
}

impl RunEventWriter {
    pub fn create(state_dir: &Path, run_id: &str) -> Result<Self> {
        let runs_dir = state_dir.join("runs");
        let run_dir = runs_dir.join(run_id);
        fs::create_dir_all(&run_dir).with_context(|| {
            format!("failed to create event run directory {}", run_dir.display())
        })?;

        let latest_dir = runs_dir.join("latest");
        if latest_dir.is_dir() {
            fs::remove_dir_all(&latest_dir).with_context(|| {
                format!(
                    "failed to reset latest event directory {}",
                    latest_dir.display()
                )
            })?;
        } else if latest_dir.exists() {
            fs::remove_file(&latest_dir).with_context(|| {
                format!("failed to reset latest event path {}", latest_dir.display())
            })?;
        }
        fs::create_dir_all(&latest_dir).with_context(|| {
            format!(
                "failed to create latest event directory {}",
                latest_dir.display()
            )
        })?;
        fs::write(latest_dir.join("run_id"), format!("{run_id}\n"))?;

        Ok(Self {
            primary: EventWriter::new(run_dir.join("events.jsonl")),
            latest: EventWriter::new(latest_dir.join("events.jsonl")),
        })
    }

    pub fn append(&self, event: &HarnessEvent) -> Result<()> {
        // The run file and `latest` are mirrors of the same factual stream.
        // Generate sequence/timestamp once so both files contain byte-identical records.
        let sequence = self.primary.next_sequence();
        let line = serde_json::to_string(&EventRecord::new(event, sequence))?;
        self.primary.append_line(&line)?;
        self.latest.append_line(&line)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        self.primary.path()
    }
}

#[derive(Debug, Serialize)]
struct EventRecord<'a> {
    schema_version: u32,
    sequence: u64,
    timestamp_ms: u128,
    event: &'a HarnessEvent,
}

impl<'a> EventRecord<'a> {
    fn new(event: &'a HarnessEvent, sequence: u64) -> Self {
        Self {
            schema_version: EVENT_SCHEMA_VERSION,
            sequence,
            timestamp_ms: timestamp_ms(),
            event,
        }
    }
}

fn timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_state() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "burncloud-harness-events-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn run_writer_appends_to_run_and_latest() {
        let state = temp_state();
        let writer = RunEventWriter::create(&state, "run-1").unwrap();
        writer
            .append(&HarnessEvent::LoopStarted { attempt: 1 })
            .unwrap();
        writer
            .append(&HarnessEvent::TaskFinished {
                success: true,
                attempts: 1,
            })
            .unwrap();

        let primary = fs::read_to_string(writer.path()).unwrap();
        let latest = fs::read_to_string(state.join("runs/latest/events.jsonl")).unwrap();
        assert_eq!(primary, latest);
        assert_eq!(primary.lines().count(), 2);
        assert_eq!(
            fs::read_to_string(state.join("runs/latest/run_id")).unwrap(),
            "run-1\n"
        );

        let records = primary
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records[0]["schema_version"], EVENT_SCHEMA_VERSION);
        assert_eq!(records[0]["sequence"], 1);
        assert_eq!(records[1]["sequence"], 2);
        assert!(records[0]["timestamp_ms"].as_u64().is_some());

        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn new_run_replaces_latest_without_overwriting_history() {
        let state = temp_state();
        let first = RunEventWriter::create(&state, "run-1").unwrap();
        first
            .append(&HarnessEvent::LoopStarted { attempt: 1 })
            .unwrap();

        let second = RunEventWriter::create(&state, "run-2").unwrap();
        second
            .append(&HarnessEvent::LoopStarted { attempt: 2 })
            .unwrap();

        assert!(state.join("runs/run-1/events.jsonl").is_file());
        assert!(state.join("runs/run-2/events.jsonl").is_file());
        let latest = fs::read_to_string(state.join("runs/latest/events.jsonl")).unwrap();
        assert_eq!(latest.lines().count(), 1);
        assert!(latest.contains("\"attempt\":2"));

        fs::remove_dir_all(state).unwrap();
    }
}
