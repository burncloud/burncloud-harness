use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{bail, Context, Result};
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
        let existing = line_count(&path);
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
        // A live agent-output writer can be opened while the trajectory writer still owns
        // another EventWriter for the same run. Reconcile with the on-disk stream before
        // allocating the next sequence so the append-only event order stays monotonic.
        loop {
            let observed = self.sequence.load(Ordering::SeqCst);
            let on_disk = line_count(&self.path);
            let base = observed.max(on_disk);
            if self
                .sequence
                .compare_exchange(observed, base + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return base + 1;
            }
        }
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

        Ok(Self::from_paths(&runs_dir, run_id))
    }

    pub fn open(state_dir: &Path, run_id: &str) -> Result<Self> {
        let runs_dir = state_dir.join("runs");
        let run_events = runs_dir.join(run_id).join("events.jsonl");
        if !run_events.is_file() {
            bail!(
                "Harness event stream does not exist: {}",
                run_events.display()
            );
        }
        let latest_run = fs::read_to_string(runs_dir.join("latest/run_id"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        if latest_run != run_id {
            bail!(
                "Harness latest event stream belongs to run {}, not {}",
                latest_run,
                run_id
            );
        }
        Ok(Self::from_paths(&runs_dir, run_id))
    }

    fn from_paths(runs_dir: &Path, run_id: &str) -> Self {
        Self {
            primary: EventWriter::new(runs_dir.join(run_id).join("events.jsonl")),
            latest: EventWriter::new(runs_dir.join("latest/events.jsonl")),
        }
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

fn line_count(path: &Path) -> u64 {
    fs::read_to_string(path)
        .map(|content| content.lines().count() as u64)
        .unwrap_or(0)
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
    fn live_writer_continues_sequence_owned_by_existing_writer() {
        let state = temp_state();
        let writer = RunEventWriter::create(&state, "run-1").unwrap();
        writer
            .append(&HarnessEvent::LoopStarted { attempt: 1 })
            .unwrap();

        let live = RunEventWriter::open(&state, "run-1").unwrap();
        live.append(&HarnessEvent::AgentOutput {
            attempt: 1,
            stream: "stdout".to_owned(),
            line: "reading dashboard.rs".to_owned(),
        })
        .unwrap();

        writer
            .append(&HarnessEvent::AgentFinished {
                success: true,
                exit_code: Some(0),
                attempt: 1,
            })
            .unwrap();

        let primary = fs::read_to_string(writer.path()).unwrap();
        let records = primary
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records[0]["sequence"], 1);
        assert_eq!(records[1]["sequence"], 2);
        assert_eq!(records[2]["sequence"], 3);
        assert_eq!(
            primary,
            fs::read_to_string(state.join("runs/latest/events.jsonl")).unwrap()
        );

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
