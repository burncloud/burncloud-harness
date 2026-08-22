use serde::Serialize;

/// Events are append-only facts produced during a harness run.
/// TUI and analysis layers consume events instead of owning state.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessEvent {
    TaskStarted {
        run_id: String,
        task: String,
        area: String,
    },
    ContractLoaded {
        allowed_scope: Vec<String>,
        avoid_scope: Vec<String>,
        max_loops: u32,
    },
    RouteSelected {
        routes: Vec<String>,
    },
    InvariantSelected {
        invariants: Vec<String>,
    },
    LoopStarted {
        attempt: u32,
    },
    AgentStarted {
        agent: String,
        attempt: u32,
    },
    AgentFinished {
        success: bool,
        exit_code: Option<i32>,
        attempt: u32,
    },
    DiffDetected {
        changed_files: Vec<String>,
    },
    VerificationStarted {
        check: String,
    },
    VerificationFinished {
        check: String,
        success: bool,
    },
    TaskFinished {
        success: bool,
        attempts: u32,
    },
}
