use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    AgentCommand,
    GitHistory,
    ScopeViolation,
    InvariantExpansion,
    NoChange,
    RiskBlock,
    RiskReview,
    Verification,
    MaxLoops,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event<'a> {
    RunStarted {
        run_id: &'a str,
        task: &'a str,
        goal: &'a str,
        area: &'a str,
        max_loops: u32,
    },
    TaskRouted {
        routes: &'a [String],
    },
    InvariantsSelected {
        invariants: &'a [String],
    },
    AttemptStarted {
        attempt: u32,
    },
    AgentFinished {
        attempt: u32,
        success: bool,
        exit_code: Option<i32>,
        stdout: &'a str,
        stderr: &'a str,
    },
    GitHeadChecked {
        attempt: u32,
        baseline: &'a str,
        current: &'a str,
        unchanged: bool,
    },
    ScopeEvaluated {
        attempt: u32,
        changed_paths: &'a [String],
        violations: &'a [String],
    },
    InvariantImpactAssessed {
        attempt: u32,
        required: &'a [String],
        newly_required: &'a [String],
        reasons: &'a [String],
    },
    RiskAssessed {
        attempt: u32,
        findings: &'a [String],
    },
    CheckFinished {
        attempt: u32,
        name: &'a str,
        command: &'a str,
        reason: &'a str,
        success: bool,
        exit_code: Option<i32>,
        stdout: &'a str,
        stderr: &'a str,
    },
    FailureRecorded {
        attempt: u32,
        class: FailureClass,
        detail: &'a str,
    },
    AttemptFailed {
        attempt: u32,
        feedback: &'a str,
    },
    RunFinished {
        success: bool,
        attempts: u32,
        changed_paths: &'a [String],
    },
}

pub struct TrajectoryWriter {
    path: PathBuf,
    writer: BufWriter<File>,
}

impl TrajectoryWriter {
    pub fn create(state_dir: &Path, run_id: &str) -> Result<Self> {
        let dir = state_dir.join("runs");
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create trajectory directory {}", dir.display()))?;
        let path = dir.join(format!("{run_id}.jsonl"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to create trajectory {}", path.display()))?;

        Ok(Self {
            path,
            writer: BufWriter::new(file),
        })
    }

    pub fn record(&mut self, event: Event<'_>) -> Result<()> {
        let envelope = EventEnvelope {
            ts_ms: unix_ms(),
            event,
        };
        serde_json::to_writer(&mut self.writer, &envelope)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Serialize)]
struct EventEnvelope<'a> {
    ts_ms: u128,
    #[serde(flatten)]
    event: Event<'a>,
}

pub fn new_run_id() -> String {
    format!("{}-{}", unix_ms(), std::process::id())
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
