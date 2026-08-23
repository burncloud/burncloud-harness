use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode},
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
    git::GitRepo,
    run_history::{self, RunArtifact, RunReplay},
    run_state::RunState,
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

pub fn list_runs(workspace: &Path) -> Result<()> {
    let state_dir = harness_state_dir(workspace)?;
    let runs = run_history::discover(&state_dir)?;
    if runs.is_empty() {
        println!(
            "No Harness runs found in target workspace {}.",
            workspace.display()
        );
        return Ok(());
    }

    for run in runs {
        println!("{}\t{}", run.run_id, run.source.as_str());
    }
    Ok(())
}

pub fn run(workspace: &Path, requested_run: Option<&str>) -> Result<()> {
    let state_dir = harness_state_dir(workspace)?;
    let mut artifact = run_history::resolve(&state_dir, requested_run)?;
    let mut replay = run_history::load(&artifact)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let loop_result = run_loop(
        &mut terminal,
        &state_dir,
        requested_run,
        &mut artifact,
        &mut replay,
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    loop_result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state_dir: &Path,
    requested_run: Option<&str>,
    artifact: &mut RunArtifact,
    replay: &mut RunReplay,
) -> Result<()> {
    loop {
        refresh_replay(state_dir, requested_run, artifact, replay)?;
        terminal.draw(|frame| draw_dashboard(frame, replay, requested_run.is_none()))?;

        if event::poll(REFRESH_INTERVAL)? {
            if let TerminalEvent::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('r') => {
                        refresh_replay(state_dir, requested_run, artifact, replay)?;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn refresh_replay(
    state_dir: &Path,
    requested_run: Option<&str>,
    artifact: &mut RunArtifact,
    replay: &mut RunReplay,
) -> Result<()> {
    let next_artifact = run_history::resolve(state_dir, requested_run)?;
    let next_replay = run_history::load(&next_artifact)?;
    *artifact = next_artifact;
    *replay = next_replay;
    Ok(())
}

fn draw_dashboard(frame: &mut Frame, replay: &RunReplay, live: bool) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);

    draw_header(frame, rows[0], &replay.state, live);

    let top = split_horizontal(rows[1], 56, 44);
    draw_task(frame, top[0], &replay.state);
    draw_boundaries(frame, top[1], &replay.state);

    let middle = split_horizontal(rows[2], 60, 40);
    draw_loop(frame, middle[0], &replay.state);
    draw_invariants(frame, middle[1], &replay.state);

    let bottom = split_horizontal(rows[3], 50, 50);
    draw_changes_and_risk(frame, bottom[0], &replay.state);
    draw_checks(frame, bottom[1], &replay.state);

    draw_footer(frame, rows[4], replay, live);
}

fn draw_header(frame: &mut Frame, area: Rect, state: &RunState, live: bool) {
    let status = match state.status.as_str() {
        "PASSED" => Span::styled(
            " PASS ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        "FAILED" => Span::styled(
            " STOPPED ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        _ => Span::styled(
            " RUNNING ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let mode = if live { "LIVE" } else { "REPLAY" };
    let line = Line::from(vec![
        Span::styled(
            " BurnCloud Harness Monitor ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        status,
        Span::raw("   "),
        Span::styled(
            format!("MODE {mode}"),
            Style::default().fg(Color::White),
        ),
        Span::raw("   "),
        Span::styled(
            "EVENTS -> REDUCER -> OBSERVER",
            Style::default().fg(Color::Magenta),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("CONTROL PLANE")), area);
}

fn draw_task(frame: &mut Frame, area: Rect, state: &RunState) {
    let attempt = if state.max_loops == 0 {
        state.attempt.to_string()
    } else {
        format!("{}/{}", state.attempt, state.max_loops)
    };
    let current = state
        .timeline
        .last()
        .map(|event| compact(&event.detail, 100))
        .unwrap_or_else(|| "waiting for events".to_owned());
    let lines = vec![
        kv("Task", &state.task),
        kv("Run", value_or(&state.run_id, "-")),
        kv("Area", &state.area),
        kv("Attempt", &attempt),
        kv("Stage", &state.stage),
        kv("Current", &current),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("TASK CONTRACT"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_boundaries(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    if state.allowed.is_empty() {
        lines.push(Line::from(Span::styled(
            "ALLOW  waiting for scope",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for path in state.allowed.iter().take(2) {
            lines.push(Line::from(vec![
                Span::styled(
                    "ALLOW  ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(compact(path, 70)),
            ]));
        }
    }
    for path in state.avoid.iter().take(2) {
        lines.push(Line::from(vec![
            Span::styled(
                "DENY   ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(compact(path, 70)),
        ]));
    }
    if state.violations.is_empty() {
        lines.push(Line::from(Span::styled(
            "BOUNDARY  no active violations",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for path in state.violations.iter().take(2) {
            lines.push(Line::from(Span::styled(
                format!("VIOLATION  {}", compact(path, 66)),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
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

fn draw_loop(frame: &mut Frame, area: Rect, state: &RunState) {
    let phases = ["AGENT", "SCOPE", "INVARIANTS", "RISK", "VERIFY", "FEEDBACK"];
    let mut spans = Vec::new();
    for (index, phase) in phases.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" -> ", Style::default().fg(Color::DarkGray)));
        }
        let active = state.stage == *phase;
        let style = if active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(format!(" {phase} "), style));
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
                Span::raw(compact(&failure.detail, 105)),
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

fn draw_invariants(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    for invariant in state.invariants.iter().take(5) {
        let is_new = state.newly_required.iter().any(|item| item == invariant);
        let marker = if is_new { "+NEW " } else { "KEEP " };
        let color = if is_new { Color::Yellow } else { Color::Cyan };
        lines.push(Line::from(vec![
            Span::styled(
                marker,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(compact(invariant, 70)),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No invariant loaded yet",
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
            lines.push(Line::from(compact(route, 72)));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("INVARIANTS + ROUTE"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_changes_and_risk(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    if state.changed_files.is_empty() {
        lines.push(Line::from(Span::styled(
            "No changed files detected yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "CHANGED FILES",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for path in state.changed_files.iter().take(6) {
            lines.push(Line::from(format!("• {}", compact(path, 82))));
        }
    }

    lines.push(Line::from(Span::styled(
        "RISK",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    if state.risk_findings.is_empty() {
        lines.push(Line::from(Span::styled(
            "No active deterministic risk findings",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for finding in state.risk_findings.iter().take(3) {
            lines.push(Line::from(Span::styled(
                compact(finding, 88),
                Style::default().fg(Color::Yellow),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("CHANGED PATHS + RISK"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_checks(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    if state.checks.is_empty() {
        lines.push(Line::from(Span::styled(
            "Verification has not started for this attempt",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for check in state.checks.iter().take(7) {
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
                Span::raw(compact(&check.name, 74)),
            ]));
        }
    }

    lines.push(Line::from(Span::styled(
        "RECENT EVENTS",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    )));
    for event in state.timeline.iter().rev().take(4).rev() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<18}", event.name.to_uppercase()),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(compact(&event.detail, 72)),
        ]));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("VERIFICATION + EVENTS"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, replay: &RunReplay, live: bool) {
    let mode = if live { "LIVE" } else { "REPLAY" };
    let line = Line::from(vec![
        Span::styled(
            format!(" {mode} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" run={}  ", replay.state.run_id)),
        Span::styled(
            format!("source={}  ", replay.source.as_str()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("auto=250ms  "),
        Span::styled(
            "q/esc quit · r refresh",
            Style::default().fg(Color::White),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("OBSERVER")), area);
}

fn split_horizontal(area: Rect, left: u16, right: u16) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(left), Constraint::Percentage(right)])
        .split(area)
}

fn panel(title: &'static str) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
}

fn kv(label: &'static str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<8}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_owned()),
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

fn harness_state_dir(workspace: &Path) -> Result<PathBuf> {
    let workspace = workspace.canonicalize()?;
    let git = GitRepo::new(workspace);
    git.ensure_repository()?;
    git.harness_state_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_collapses_multiline_failure_output() {
        let value = "verification failed\n  Diff in src/main.rs\n    let value = 1;";
        assert_eq!(
            compact(value, 200),
            "verification failed Diff in src/main.rs let value = 1;"
        );
    }

    #[test]
    fn compact_truncates_long_failure_output() {
        assert_eq!(compact("abcdefgh", 5), "abcde…");
    }
}
