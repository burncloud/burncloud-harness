use std::{io, path::PathBuf};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};

use crate::{
    config::TaskSpec,
    observer::{RunEvent, RunObserver, RunPhase},
    runner::{self, RunSummary},
};

pub fn run(task: TaskSpec) -> Result<RunSummary> {
    enable_raw_mode().context("failed to enable terminal raw mode")?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
        let _ = disable_raw_mode();
        return Err(error).context("failed to enter alternate terminal screen");
    }

    let backend = CrosstermBackend::new(stdout);
    let terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            return Err(error).context("failed to initialize Ratatui terminal");
        }
    };

    let mut console = ConsoleObserver::new(terminal);
    console.render()?;
    let result = runner::run_with_observer(task, &mut console);

    if let Err(error) = &result {
        console.state.final_status = Some(false);
        console.state.phase = Some(RunPhase::Done);
        console.state.phase_detail = error.to_string();
        console.render()?;
    }

    console.wait_for_close()?;
    drop(console);
    result
}

#[derive(Debug, Clone, Default)]
struct DashboardState {
    task: String,
    goal: String,
    area: String,
    max_loops: u32,
    attempt: u32,
    phase: Option<RunPhase>,
    phase_detail: String,
    allowed: Vec<String>,
    avoid: Vec<String>,
    routes: Vec<String>,
    invariants: Vec<String>,
    newly_required: Vec<String>,
    changed_paths: Vec<String>,
    violations: Vec<String>,
    risks: Vec<String>,
    checks: Vec<CheckRow>,
    failures: Vec<LoopFailure>,
    final_status: Option<bool>,
    trajectory_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct CheckRow {
    name: String,
    reason: String,
    success: Option<bool>,
}

#[derive(Debug, Clone)]
struct LoopFailure {
    attempt: u32,
    class: String,
    detail: String,
}

impl DashboardState {
    fn apply(&mut self, event: RunEvent) {
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
                self.task = task;
                self.goal = goal;
                self.area = area;
                self.max_loops = max_loops;
                self.allowed = allowed;
                self.avoid = avoid;
                self.routes = routes;
                self.invariants = invariants;
                self.phase = Some(RunPhase::Prepare);
                self.phase_detail = "BurnCloud task contract and hard boundaries loaded".into();
            }
            RunEvent::Phase {
                attempt,
                phase,
                detail,
            } => {
                self.attempt = attempt;
                self.phase = Some(phase);
                self.phase_detail = detail;
                if phase == RunPhase::Agent {
                    self.newly_required.clear();
                    self.risks.clear();
                    self.checks.clear();
                }
            }
            RunEvent::Paths {
                attempt,
                changed,
                violations,
            } => {
                self.attempt = attempt;
                self.changed_paths = changed;
                self.violations = violations;
            }
            RunEvent::Invariants {
                attempt,
                active,
                newly_required,
            } => {
                self.attempt = attempt;
                self.invariants = active;
                self.newly_required = newly_required;
            }
            RunEvent::Risks { attempt, findings } => {
                self.attempt = attempt;
                self.risks = findings;
            }
            RunEvent::Check {
                attempt,
                name,
                reason,
                success,
            } => {
                self.attempt = attempt;
                if let Some(existing) = self.checks.iter_mut().find(|check| check.name == name) {
                    existing.reason = reason;
                    existing.success = success;
                } else {
                    self.checks.push(CheckRow {
                        name,
                        reason,
                        success,
                    });
                }
            }
            RunEvent::Failure {
                attempt,
                class,
                detail,
            } => {
                self.attempt = attempt;
                self.phase = Some(RunPhase::Feedback);
                self.phase_detail = format!("{class}: {}", compact(&detail, 180));
                self.failures.push(LoopFailure {
                    attempt,
                    class,
                    detail,
                });
            }
            RunEvent::Finished {
                success,
                attempts,
                changed_paths,
                trajectory_path,
            } => {
                self.final_status = Some(success);
                self.attempt = attempts;
                self.phase = Some(RunPhase::Done);
                self.phase_detail = if success {
                    "All Harness gates passed".into()
                } else {
                    "Harness stopped the run".into()
                };
                self.changed_paths = changed_paths;
                self.trajectory_path = Some(trajectory_path);
            }
        }
    }
}

struct ConsoleObserver {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    state: DashboardState,
}

impl ConsoleObserver {
    fn new(terminal: Terminal<CrosstermBackend<io::Stdout>>) -> Self {
        Self {
            terminal,
            state: DashboardState::default(),
        }
    }

    fn render(&mut self) -> Result<()> {
        let state = self.state.clone();
        self.terminal
            .draw(|frame| draw_dashboard(frame, &state))
            .context("failed to render BurnCloud Harness console")?;
        Ok(())
    }

    fn wait_for_close(&mut self) -> Result<()> {
        self.render()?;
        loop {
            let event = event::read().context("failed to read terminal input")?;
            if let TerminalEvent::Key(key) = event {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q'))
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    return Ok(());
                }
            }
        }
    }
}

impl RunObserver for ConsoleObserver {
    fn on_event(&mut self, event: RunEvent) -> Result<()> {
        self.state.apply(event);
        self.render()
    }
}

impl Drop for ConsoleObserver {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

fn draw_dashboard(frame: &mut Frame, state: &DashboardState) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Length(11),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);

    draw_header(frame, rows[0], state);

    let top = split_horizontal(rows[1], 56, 44);
    draw_task(frame, top[0], state);
    draw_boundaries(frame, top[1], state);

    let middle = split_horizontal(rows[2], 58, 42);
    draw_loop(frame, middle[0], state);
    draw_invariants(frame, middle[1], state);

    let bottom = split_horizontal(rows[3], 48, 52);
    draw_paths(frame, bottom[0], state);
    draw_checks(frame, bottom[1], state);

    draw_footer(frame, rows[4], state);
}

fn draw_header(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let status = match state.final_status {
        Some(true) => Span::styled(
            " PASS ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Some(false) => Span::styled(
            " STOPPED ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        None => Span::styled(
            " RUNNING ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let line = Line::from(vec![
        Span::styled(
            " BurnCloud Harness Console ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        status,
        Span::raw("  "),
        Span::styled(
            "HARNESS = BOUNDARIES + FEEDBACK",
            Style::default().fg(Color::White),
        ),
        Span::raw("   "),
        Span::styled(
            "LOOP = ATTEMPT -> EVIDENCE -> RETRY",
            Style::default().fg(Color::Magenta),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("CONTROL PLANE")), area);
}

fn draw_task(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let loop_text = if state.max_loops == 0 {
        "not started".to_owned()
    } else {
        format!("{}/{}", state.attempt, state.max_loops)
    };
    let lines = vec![
        kv("Task", value_or(&state.task, "waiting for task")),
        kv("Goal", value_or(&state.goal, "-")),
        kv("Area", value_or(&state.area, "-")),
        kv("Loop", &loop_text),
        kv(
            "Phase",
            state.phase.map(RunPhase::as_str).unwrap_or("PREPARE"),
        ),
        kv(
            "Why now",
            value_or(&state.phase_detail, "initializing console"),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("TASK CONTRACT"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_boundaries(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let mut lines = Vec::new();
    if state.allowed.is_empty() {
        lines.push(Line::from(Span::styled(
            "ALLOW  waiting for task scope",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for path in state.allowed.iter().take(4) {
            lines.push(Line::from(vec![
                Span::styled(
                    "ALLOW  ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(path),
            ]));
        }
    }
    for path in state.avoid.iter().take(3) {
        lines.push(Line::from(vec![
            Span::styled(
                "DENY   ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(path),
        ]));
    }
    if !state.violations.is_empty() {
        lines.push(Line::from(Span::styled(
            "--- VIOLATION ---",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        for path in state.violations.iter().take(2) {
            lines.push(Line::from(Span::styled(
                path,
                Style::default().fg(Color::Red),
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("HARD BOUNDARY"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_loop(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let phases = [
        RunPhase::Agent,
        RunPhase::Scope,
        RunPhase::Invariants,
        RunPhase::Risk,
        RunPhase::Verify,
        RunPhase::Feedback,
    ];
    let mut spans = Vec::new();
    for (index, phase) in phases.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" -> ", Style::default().fg(Color::DarkGray)));
        }
        let active = state.phase == Some(*phase);
        let style = if active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(format!(" {} ", phase.as_str()), style));
    }

    let mut lines = vec![Line::from(spans)];
    if state.failures.is_empty() {
        lines.push(Line::from(Span::styled(
            "No retry yet. A loop only happens when evidence produces feedback.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "WHY THE LOOP RETRIED",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
        for failure in state.failures.iter().rev().take(5).rev() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("#{} {}  ", failure.attempt, failure.class),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(compact(&failure.detail, 100)),
            ]));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("EVIDENCE-DRIVEN LOOP"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_invariants(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let mut lines = Vec::new();
    for invariant in state.invariants.iter().take(6) {
        let is_new = state.newly_required.iter().any(|item| item == invariant);
        let marker = if is_new { "+NEW " } else { "KEEP " };
        let color = if is_new { Color::Yellow } else { Color::Cyan };
        lines.push(Line::from(vec![
            Span::styled(
                marker,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(invariant),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No candidate invariant loaded yet",
            Style::default().fg(Color::DarkGray),
        )));
    }
    if !state.routes.is_empty() {
        lines.push(Line::from(Span::styled(
            "Route evidence:",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
        for route in state.routes.iter().take(2) {
            lines.push(Line::from(compact(route, 90)));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("INVARIANTS / ROUTING"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_paths(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let mut lines = Vec::new();
    if state.changed_paths.is_empty() {
        lines.push(Line::from(Span::styled(
            "No actual diff observed yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for path in state.changed_paths.iter().take(10) {
            lines.push(Line::from(vec![
                Span::styled("EDIT  ", Style::default().fg(Color::Cyan)),
                Span::raw(path),
            ]));
        }
    }
    if !state.risks.is_empty() {
        lines.push(Line::from(Span::styled(
            "Risk findings:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        for risk in state.risks.iter().take(4) {
            lines.push(Line::from(compact(risk, 110)));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("ACTUAL DIFF / RISK"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_checks(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let mut lines = Vec::new();
    if state.checks.is_empty() {
        lines.push(Line::from(Span::styled(
            "Verification has not started",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for check in state.checks.iter().take(8) {
            let (marker, color) = match check.success {
                Some(true) => ("PASS", Color::Green),
                Some(false) => ("FAIL", Color::Red),
                None => ("RUN ", Color::Yellow),
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{marker}  "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(&check.name, Style::default().fg(Color::White)),
                Span::raw(" — "),
                Span::styled(
                    compact(&check.reason, 70),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }
    if let Some(last) = state.failures.last() {
        lines.push(Line::from(Span::styled(
            "Latest feedback:",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(compact(&last.detail, 180)));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("VERIFICATION / FEEDBACK"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let instruction = if state.final_status.is_some() {
        "Press Enter / q / Esc to close"
    } else {
        "Harness owns the boundaries. Codex acts inside them. Failed evidence becomes the next Loop input."
    };
    let trajectory = state
        .trajectory_path
        .as_ref()
        .map(|path| format!("  trajectory={}", path.display()))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(instruction, Style::default().fg(Color::White)),
            Span::styled(trajectory, Style::default().fg(Color::DarkGray)),
        ]))
        .block(panel("MENTAL MODEL")),
        area,
    );
}

fn split_horizontal(area: Rect, left: u16, right: u16) -> [Rect; 2] {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(left), Constraint::Percentage(right)])
        .split(area);
    [columns[0], columns[1]]
}

fn panel(title: &'static str) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
}

fn kv<'a>(key: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{key:<8}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(value),
    ])
}

fn value_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() {
        fallback
    } else {
        value
    }
}

fn compact(value: &str, limit: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= limit {
        normalized
    } else {
        format!("{}…", normalized.chars().take(limit).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_event_makes_loop_reason_visible() {
        let mut state = DashboardState::default();
        state.apply(RunEvent::Prepared {
            task: "router-fix".into(),
            goal: "fix fallback".into(),
            area: "router".into(),
            max_loops: 3,
            allowed: vec!["crates/router/**".into()],
            avoid: vec![],
            routes: vec![],
            invariants: vec!["INV-ROUTER-001".into()],
        });
        state.apply(RunEvent::Failure {
            attempt: 1,
            class: "verification".into(),
            detail: "billing invariant failed".into(),
        });

        assert_eq!(state.failures.len(), 1);
        assert_eq!(state.failures[0].attempt, 1);
        assert_eq!(state.phase, Some(RunPhase::Feedback));
    }

    #[test]
    fn invariant_expansion_is_marked_as_new() {
        let mut state = DashboardState::default();
        state.apply(RunEvent::Invariants {
            attempt: 1,
            active: vec!["INV-ROUTER-001".into(), "INV-BILLING-001".into()],
            newly_required: vec!["INV-BILLING-001".into()],
        });
        assert_eq!(state.newly_required, vec!["INV-BILLING-001"]);
    }
}
