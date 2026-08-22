use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    Prepare,
    Agent,
    Scope,
    Invariants,
    Risk,
    Verify,
    Feedback,
    Done,
}

impl RunPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "PREPARE",
            Self::Agent => "AGENT",
            Self::Scope => "SCOPE",
            Self::Invariants => "INVARIANTS",
            Self::Risk => "RISK",
            Self::Verify => "VERIFY",
            Self::Feedback => "FEEDBACK",
            Self::Done => "DONE",
        }
    }
}

#[derive(Debug, Clone)]
pub enum RunEvent {
    Prepared {
        task: String,
        goal: String,
        area: String,
        max_loops: u32,
        allowed: Vec<String>,
        avoid: Vec<String>,
        routes: Vec<String>,
        invariants: Vec<String>,
    },
    Phase {
        attempt: u32,
        phase: RunPhase,
        detail: String,
    },
    Paths {
        attempt: u32,
        changed: Vec<String>,
        violations: Vec<String>,
    },
    Invariants {
        attempt: u32,
        active: Vec<String>,
        newly_required: Vec<String>,
    },
    Risks {
        attempt: u32,
        findings: Vec<String>,
    },
    Check {
        attempt: u32,
        name: String,
        reason: String,
        success: Option<bool>,
    },
    Failure {
        attempt: u32,
        class: String,
        detail: String,
    },
    Finished {
        success: bool,
        attempts: u32,
        changed_paths: Vec<String>,
        trajectory_path: PathBuf,
    },
}

pub trait RunObserver {
    fn on_event(&mut self, event: RunEvent) -> Result<()>;
}

#[derive(Default)]
pub struct NoopObserver;

impl RunObserver for NoopObserver {
    fn on_event(&mut self, _event: RunEvent) -> Result<()> {
        Ok(())
    }
}
