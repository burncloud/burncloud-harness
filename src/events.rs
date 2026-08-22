use serde::Serialize;

/// Events are append-only facts produced during a harness run.
/// TUI and future analysis layers should consume events instead of owning state.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessEvent {
    TaskStarted { task: String },
    ContractLoaded { area: String },
    RouteSelected { routes: Vec<String> },
    InvariantsSelected { invariants: Vec<String> },
    LoopStarted { attempt: u32 },
    AgentStarted { attempt: u32, program: String },
    AgentFinished { attempt: u32, success: bool },
    DiffDetected { attempt: u32, paths: Vec<String> },
    VerificationStarted { attempt: u32, check: String },
    VerificationFinished {
        attempt: u32,
        check: String,
        success: bool,
    },
    TaskFinished { success: bool },
}
