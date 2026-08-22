use std::{fs::OpenOptions, io::Write, path::Path};

use anyhow::Result;
use serde::Serialize;

use crate::events::HarnessEvent;

#[derive(Debug, Clone)]
pub struct EventWriter {
    path: std::path::PathBuf,
}

impl EventWriter {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn append(&self, event: &HarnessEvent) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let line = serde_json::to_string(&EventRecord::new(event))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        writeln!(file, "{line}")?;
        Ok(())
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
