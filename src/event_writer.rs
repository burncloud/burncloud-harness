use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::events::HarnessEvent;

#[derive(Debug, Clone)]
pub struct EventWriter {
    path: PathBuf,
}

impl EventWriter {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn append(&self, event: &HarnessEvent) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let line = serde_json::to_string(&EventRecord::new(event))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        writeln!(file, "{line}")?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
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
        fs::create_dir_all(&run_dir)
            .with_context(|| format!("failed to create event run directory {}", run_dir.display()))?;

        let latest_dir = runs_dir.join("latest");
        if latest_dir.is_dir() {
            fs::remove_dir_all(&latest_dir).with_context(|| {
                format!("failed to reset latest event directory {}", latest_dir.display())
            })?;
        } else if latest_dir.exists() {
            fs::remove_file(&latest_dir).with_context(|| {
                format!("failed to reset latest event path {}", latest_dir.display())
            })?;
        }
        fs::create_dir_all(&latest_dir).with_context(|| {
            format!("failed to create latest event directory {}", latest_dir.display())
        })?;
        fs::write(latest_dir.join("run_id"), format!("{run_id}\n"))?;

        Ok(Self {
            primary: EventWriter::new(run_dir.join("events.jsonl")),
            latest: EventWriter::new(latest_dir.join("events.jsonl")),
        })
    }

    pub fn append(&self, event: &HarnessEvent) -> Result<()> {
        self.primary.append(event)?;
        self.latest.append(event)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        self.primary.path()
    }
}

#[derive(Debug, Serialize)]
struct EventRecord<'a> {
    timestamp: u64,
    event: &'a HarnessEvent,
}

impl<'a> EventRecord<'a> {
    fn new(event: &'a HarnessEvent) -> Self {
        Self {
            timestamp: timestamp(),
            event,
        }
    }
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
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
