use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::{
    event_writer::RunEventWriter,
    events::HarnessEvent,
};

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

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentCommand => "agent_command",
            Self::GitHistory => "git_history",
            Self::ScopeViolation => "scope_violation",
            Self::InvariantExpansion => "invariant_expansion",
            Self::NoChange => "no_change",
            Self::RiskBlock => "risk_block",
            Self::RiskReview => "risk_review",
            Self::Verification => "verification",
            Self::MaxLoops => "max_loops",
        }
    }
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
        baseline_head: &'a str,
        allowed: &'a [String],
        avoid: &'a [String],
        context_files: &'a [String],
        agent_program: &'a str,
        agent_args: &'a [String],
        agent_append_prompt: bool,
        resumed_from: Option<&'a str>,
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
        diff_fingerprint: &'a str,
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

#[derive(Debug, Clone)]
pub struct ResumeProvenance {
    pub run_id: String,
    pub task: String,
    pub goal: String,
    pub area: String,
    pub max_loops: u32,
    pub baseline_head: String,
    pub allowed: Vec<String>,
    pub avoid: Vec<String>,
    pub context_files: Vec<String>,
    pub agent_program: String,
    pub agent_args: Vec<String>,
    pub agent_append_prompt: bool,
    pub changed_paths: Vec<String>,
    pub diff_fingerprint: String,
}

pub fn load_resume_provenances(state_dir: &Path) -> Result<Vec<ResumeProvenance>> {
    let runs_dir = state_dir.join("runs");
    if !runs_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = fs::read_dir(&runs_dir)
        .with_context(|| format!("failed to read trajectory directory {}", runs_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    paths.sort();

    let mut candidates = Vec::new();
    for path in paths {
        if let Some(candidate) = load_resume_provenance(&path)? {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

fn load_resume_provenance(path: &Path) -> Result<Option<ResumeProvenance>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open trajectory {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut started: Option<ResumeProvenance> = None;
    let mut last_checkpoint: Option<(Vec<String>, String)> = None;
    let mut finished = false;

    for line in reader.lines() {
        let line = line.with_context(|| format!("failed to read trajectory {}", path.display()))?;
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match event_type {
            "run_started" => {
                let Some(run_id) = string_field(&value, "run_id") else {
                    return Ok(None);
                };
                let Some(task) = string_field(&value, "task") else {
                    return Ok(None);
                };
                let Some(goal) = string_field(&value, "goal") else {
                    return Ok(None);
                };
                let Some(area) = string_field(&value, "area") else {
                    return Ok(None);
                };
                let Some(max_loops) = value
                    .get("max_loops")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                else {
                    return Ok(None);
                };
                let Some(baseline_head) = string_field(&value, "baseline_head") else {
                    return Ok(None);
                };
                let Some(allowed) = string_vec_field(&value, "allowed") else {
                    return Ok(None);
                };
                let Some(avoid) = string_vec_field(&value, "avoid") else {
                    return Ok(None);
                };
                let Some(context_files) = string_vec_field(&value, "context_files") else {
                    return Ok(None);
                };
                let Some(agent_program) = string_field(&value, "agent_program") else {
                    return Ok(None);
                };
                let Some(agent_args) = string_vec_field(&value, "agent_args") else {
                    return Ok(None);
                };
                let Some(agent_append_prompt) =
                    value.get("agent_append_prompt").and_then(Value::as_bool)
                else {
                    return Ok(None);
                };

                started = Some(ResumeProvenance {
                    run_id,
                    task,
                    goal,
                    area,
                    max_loops,
                    baseline_head,
                    allowed,
                    avoid,
                    context_files,
                    agent_program,
                    agent_args,
                    agent_append_prompt,
                    changed_paths: Vec::new(),
                    diff_fingerprint: String::new(),
                });
            }
            "scope_evaluated" => {
                let Some(changed_paths) = string_vec_field(&value, "changed_paths") else {
                    return Ok(None);
                };
                let Some(diff_fingerprint) = string_field(&value, "diff_fingerprint") else {
                    return Ok(None);
                };
                last_checkpoint = Some((changed_paths, diff_fingerprint));
            }
            "run_finished" => {
                finished = true;
            }
            _ => {}
        }
    }

    if finished {
        return Ok(None);
    }

    let Some(mut provenance) = started else {
        return Ok(None);
    };
    let Some((changed_paths, diff_fingerprint)) = last_checkpoint else {
        return Ok(None);
    };
    if changed_paths.is_empty() {
        return Ok(None);
    }

    provenance.changed_paths = changed_paths;
    provenance.diff_fingerprint = diff_fingerprint;
    Ok(Some(provenance))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

fn string_vec_field(value: &Value, key: &str) -> Option<Vec<String>> {
    value
        .get(key)?
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

pub struct TrajectoryWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    events: RunEventWriter,
    agent_program: Option<String>,
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
        let events = RunEventWriter::create(state_dir, run_id)?;

        Ok(Self {
            path,
            writer: BufWriter::new(file),
            events,
            agent_program: None,
        })
    }

    pub fn record(&mut self, event: Event<'_>) -> Result<()> {
        self.record_event_stream(&event)?;
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

    pub fn event_path(&self) -> &Path {
        self.events.path()
    }

    fn record_event_stream(&mut self, event: &Event<'_>) -> Result<()> {
        match event {
            Event::RunStarted {
                run_id,
                task,
                area,
                max_loops,
                allowed,
                avoid,
                agent_program,
                ..
            } => {
                self.agent_program = Some((*agent_program).to_owned());
                self.events.append(&HarnessEvent::TaskStarted {
                    run_id: (*run_id).to_owned(),
                    task: (*task).to_owned(),
                    area: (*area).to_owned(),
                })?;
                self.events.append(&HarnessEvent::ContractLoaded {
                    allowed_scope: allowed.to_vec(),
                    avoid_scope: avoid.to_vec(),
                    max_loops: *max_loops,
                })?;
            }
            Event::TaskRouted { routes } => {
                self.events.append(&HarnessEvent::RouteSelected {
                    routes: routes.to_vec(),
                })?;
            }
            Event::InvariantsSelected { invariants } => {
                self.events.append(&HarnessEvent::InvariantSelected {
                    invariants: invariants.to_vec(),
                })?;
            }
            Event::AttemptStarted { attempt } => {
                self.events.append(&HarnessEvent::LoopStarted { attempt: *attempt })?;
                self.events.append(&HarnessEvent::AgentStarted {
                    agent: self
                        .agent_program
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned()),
                    attempt: *attempt,
                })?;
            }
            Event::AgentFinished {
                attempt,
                success,
                exit_code,
                ..
            } => {
                self.events.append(&HarnessEvent::AgentFinished {
                    success: *success,
                    exit_code: *exit_code,
                    attempt: *attempt,
                })?;
            }
            Event::ScopeEvaluated { changed_paths, .. } => {
                self.events.append(&HarnessEvent::DiffDetected {
                    changed_files: changed_paths.to_vec(),
                })?;
            }
            Event::CheckFinished { name, success, .. } => {
                self.events.append(&HarnessEvent::VerificationStarted {
                    check: (*name).to_owned(),
                })?;
                self.events.append(&HarnessEvent::VerificationFinished {
                    check: (*name).to_owned(),
                    success: *success,
                })?;
            }
            Event::RunFinished {
                success, attempts, ..
            } => {
                self.events.append(&HarnessEvent::TaskFinished {
                    success: *success,
                    attempts: *attempts,
                })?;
            }
            Event::GitHeadChecked { .. }
            | Event::InvariantImpactAssessed { .. }
            | Event::RiskAssessed { .. }
            | Event::FailureRecorded { .. }
            | Event::AttemptFailed { .. } => {}
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_trajectory_without_provenance_is_not_resumable() {
        let unique = unix_ms();
        let root = std::env::temp_dir().join(format!(
            "burncloud-harness-trajectory-historical-{}-{unique}",
            std::process::id()
        ));
        let runs = root.join("runs");
        fs::create_dir_all(&runs).unwrap();
        fs::write(
            runs.join("old.jsonl"),
            r#"{"type":"run_started","run_id":"old","task":"x","goal":"y","area":"ui","max_loops":3}
{"type":"scope_evaluated","attempt":1,"changed_paths":["crates/client/x.rs"],"violations":[]}
"#,
        )
        .unwrap();

        assert!(load_resume_provenances(&root).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unfinished_checkpoint_with_provenance_is_resumable() {
        let unique = unix_ms();
        let root = std::env::temp_dir().join(format!(
            "burncloud-harness-trajectory-unfinished-{}-{unique}",
            std::process::id()
        ));
        let runs = root.join("runs");
        fs::create_dir_all(&runs).unwrap();
        fs::write(
            runs.join("new.jsonl"),
            r#"{"type":"run_started","run_id":"new","task":"x","goal":"y","area":"ui","max_loops":3,"baseline_head":"abc","allowed":["crates/client/**"],"avoid":[],"context_files":["../../target.md"],"agent_program":"codex","agent_args":["exec"],"agent_append_prompt":true,"resumed_from":null}
{"type":"scope_evaluated","attempt":1,"changed_paths":["crates/client/x.rs"],"violations":[],"diff_fingerprint":"fnv1a64:1234"}
"#,
        )
        .unwrap();

        let candidates = load_resume_provenances(&root).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].run_id, "new");
        assert_eq!(candidates[0].diff_fingerprint, "fnv1a64:1234");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trajectory_records_runner_event_stream() {
        let unique = unix_ms();
        let root = std::env::temp_dir().join(format!(
            "burncloud-harness-trajectory-events-{}-{unique}",
            std::process::id()
        ));
        let allowed = vec!["src/**".to_owned()];
        let avoid = vec!["Cargo.lock".to_owned()];
        let routes = vec!["ui".to_owned()];
        let invariants = vec!["no-regression".to_owned()];
        let changed = vec!["src/main.rs".to_owned()];
        let mut writer = TrajectoryWriter::create(&root, "run-1").unwrap();

        writer
            .record(Event::RunStarted {
                run_id: "run-1",
                task: "test-task",
                goal: "test goal",
                area: "ui",
                max_loops: 3,
                baseline_head: "abc",
                allowed: &allowed,
                avoid: &avoid,
                context_files: &[],
                agent_program: "codex",
                agent_args: &[],
                agent_append_prompt: true,
                resumed_from: None,
            })
            .unwrap();
        writer.record(Event::TaskRouted { routes: &routes }).unwrap();
        writer
            .record(Event::InvariantsSelected {
                invariants: &invariants,
            })
            .unwrap();
        writer.record(Event::AttemptStarted { attempt: 1 }).unwrap();
        writer
            .record(Event::AgentFinished {
                attempt: 1,
                success: true,
                exit_code: Some(0),
                stdout: "",
                stderr: "",
            })
            .unwrap();
        writer
            .record(Event::ScopeEvaluated {
                attempt: 1,
                changed_paths: &changed,
                violations: &[],
                diff_fingerprint: "fnv1a64:test",
            })
            .unwrap();
        writer
            .record(Event::CheckFinished {
                attempt: 1,
                name: "cargo test",
                command: "cargo test",
                reason: "required",
                success: true,
                exit_code: Some(0),
                stdout: "",
                stderr: "",
            })
            .unwrap();
        writer
            .record(Event::RunFinished {
                success: true,
                attempts: 1,
                changed_paths: &changed,
            })
            .unwrap();

        let content = fs::read_to_string(root.join("runs/run-1/events.jsonl")).unwrap();
        let types = content
            .lines()
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                value["event"]["type"].as_str().unwrap().to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            types,
            vec![
                "task_started",
                "contract_loaded",
                "route_selected",
                "invariant_selected",
                "loop_started",
                "agent_started",
                "agent_finished",
                "diff_detected",
                "verification_started",
                "verification_finished",
                "task_finished",
            ]
        );
        assert_eq!(
            content,
            fs::read_to_string(root.join("runs/latest/events.jsonl")).unwrap()
        );

        fs::remove_dir_all(root).unwrap();
    }
}
