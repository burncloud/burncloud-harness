use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use tracing::{error, info, warn};

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

/// Legacy default observer name retained for compatibility.
/// CLI runs now forward these events into structured tracing output.
#[derive(Default)]
pub struct NoopObserver;

impl RunObserver for NoopObserver {
    fn on_event(&mut self, event: RunEvent) -> Result<()> {
        let mut observer = TracingObserver;
        observer.on_event(event)
    }
}

#[derive(Default)]
pub struct TracingObserver;

impl RunObserver for TracingObserver {
    fn on_event(&mut self, event: RunEvent) -> Result<()> {
        match event {
            RunEvent::Prepared {
                task,
                goal,
                area,
                max_loops,
                allowed,
                avoid,
                routes,
                invariants,
            } => {
                info!(
                    phase = "PREPARE",
                    task = %task,
                    goal = %goal,
                    area = %area,
                    max_loops,
                    allowed = ?allowed,
                    avoid = ?avoid,
                    routes = ?routes,
                    invariants = ?invariants,
                    "harness task prepared"
                );
            }
            RunEvent::Phase {
                attempt,
                phase,
                detail,
            } => {
                if phase == RunPhase::Agent {
                    start_agent_heartbeat(attempt);
                } else if phase == RunPhase::Scope {
                    stop_agent_heartbeat();
                }

                info!(
                    attempt,
                    phase = phase.as_str(),
                    detail = %detail,
                    "harness phase"
                );
            }
            RunEvent::Paths {
                attempt,
                changed,
                violations,
            } => {
                if violations.is_empty() {
                    info!(
                        attempt,
                        phase = "SCOPE",
                        changed_files = ?changed,
                        "diff accepted by scope"
                    );
                } else {
                    warn!(
                        attempt,
                        phase = "SCOPE",
                        changed_files = ?changed,
                        violations = ?violations,
                        "scope violation detected"
                    );
                }
            }
            RunEvent::Invariants {
                attempt,
                active,
                newly_required,
            } => {
                info!(
                    attempt,
                    phase = "INVARIANTS",
                    active = ?active,
                    newly_required = ?newly_required,
                    "invariant impact evaluated"
                );
            }
            RunEvent::Risks { attempt, findings } => {
                if findings.is_empty() {
                    info!(attempt, phase = "RISK", "no deterministic risk findings");
                } else {
                    warn!(
                        attempt,
                        phase = "RISK",
                        findings = ?findings,
                        "risk findings detected"
                    );
                }
            }
            RunEvent::Check {
                attempt,
                name,
                reason,
                success,
            } => match success {
                None => info!(
                    attempt,
                    phase = "VERIFY",
                    check = %name,
                    reason = %reason,
                    "verification started"
                ),
                Some(true) => info!(
                    attempt,
                    phase = "VERIFY",
                    check = %name,
                    "verification passed"
                ),
                Some(false) => warn!(
                    attempt,
                    phase = "VERIFY",
                    check = %name,
                    "verification failed"
                ),
            },
            RunEvent::Failure {
                attempt,
                class,
                detail,
            } => {
                warn!(
                    attempt,
                    phase = "FEEDBACK",
                    class = %class,
                    detail = %detail,
                    "harness decision requires retry or stop"
                );
            }
            RunEvent::Finished {
                success,
                attempts,
                changed_paths,
                trajectory_path,
            } => {
                stop_agent_heartbeat();
                if success {
                    info!(
                        phase = "DONE",
                        success,
                        attempts,
                        changed_files = ?changed_paths,
                        trajectory = %trajectory_path.display(),
                        "harness run finished"
                    );
                } else {
                    error!(
                        phase = "DONE",
                        success,
                        attempts,
                        changed_files = ?changed_paths,
                        trajectory = %trajectory_path.display(),
                        "harness run failed"
                    );
                }
            }
        }
        Ok(())
    }
}

struct AgentHeartbeat {
    stop: Arc<AtomicBool>,
}

static AGENT_HEARTBEAT: OnceLock<Mutex<Option<AgentHeartbeat>>> = OnceLock::new();

fn heartbeat_slot() -> &'static Mutex<Option<AgentHeartbeat>> {
    AGENT_HEARTBEAT.get_or_init(|| Mutex::new(None))
}

fn start_agent_heartbeat(attempt: u32) {
    stop_agent_heartbeat();

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    thread::spawn(move || {
        let started = Instant::now();
        loop {
            thread::sleep(Duration::from_secs(5));
            if thread_stop.load(Ordering::Relaxed) {
                break;
            }
            info!(
                attempt,
                phase = "AGENT",
                elapsed_secs = started.elapsed().as_secs(),
                "agent still running"
            );
        }
    });

    if let Ok(mut slot) = heartbeat_slot().lock() {
        *slot = Some(AgentHeartbeat { stop });
    }
}

fn stop_agent_heartbeat() {
    let heartbeat = heartbeat_slot()
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    if let Some(heartbeat) = heartbeat {
        heartbeat.stop.store(true, Ordering::Relaxed);
    }
}
