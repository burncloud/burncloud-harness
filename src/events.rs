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
    StageStarted {
        attempt: u32,
        stage: String,
    },
    AgentStarted {
        agent: String,
        attempt: u32,
    },
    AgentOutput {
        attempt: u32,
        stream: String,
        line: String,
    },
    AgentHeartbeat {
        attempt: u32,
        elapsed_secs: u64,
    },
    AgentFinished {
        success: bool,
        exit_code: Option<i32>,
        attempt: u32,
    },
    DiffDetected {
        attempt: u32,
        changed_files: Vec<String>,
    },
    ScopeEvaluated {
        attempt: u32,
        violations: Vec<String>,
        success: bool,
    },
    InvariantExpanded {
        attempt: u32,
        invariants: Vec<String>,
        reasons: Vec<String>,
    },
    RiskDetected {
        attempt: u32,
        findings: Vec<String>,
    },
    VerificationStarted {
        attempt: u32,
        check: String,
    },
    VerificationFinished {
        attempt: u32,
        check: String,
        success: bool,
    },
    FailureRecorded {
        attempt: u32,
        class: String,
        detail: String,
    },
    RetryRequested {
        attempt: u32,
        reason: String,
    },
    TaskFinished {
        success: bool,
        attempts: u32,
    },
}
