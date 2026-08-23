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
    widgets::{Block, Borders, Paragraph},
    Terminal,
};

use crate::{
    git::GitRepo,
    run_history::{self, RunArtifact, RunReplay},
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
        terminal.draw(|frame| {
            let area = frame.area();
            let widget = Paragraph::new(render_replay(replay, requested_run.is_none())).block(
                Block::default()
                    .title("BurnCloud Harness Monitor")
                    .borders(Borders::ALL),
            );
            frame.render_widget(widget, area);
        })?;

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

fn render_replay(replay: &RunReplay, live: bool) -> String {
    let state = &replay.state;
    let mut output = String::new();
    output.push_str(&format!("Mode: {}\n", if live { "LIVE" } else { "REPLAY" }));
    output.push_str(&format!("Run ID: {}\n", state.run_id));
    output.push_str(&format!("Task: {}\n", state.task));
    output.push_str(&format!("Area: {}\n", state.area));
    output.push_str(&format!("Stage: {}\n", state.stage));
    output.push_str(&format!("Status: {}\n", state.status));
    output.push_str(&format!("Attempt: {}\n", state.attempt));
    output.push_str(&format!("Source: {}\n\n", replay.source.as_str()));
    output.push_str("Timeline:\n");

    for event in state.timeline.iter().rev().take(20).rev() {
        output.push_str(&format!(
            "✓ {:<24} {}\n",
            event.name.to_uppercase(),
            event.detail
        ));
    }

    if let Some(failure) = state.failures.last() {
        output.push_str(&format!(
            "\nLatest decision: #{} {} · {}\n",
            failure.attempt, failure.class, failure.detail
        ));
    }
    output.push_str(&format!(
        "Changed files: {} · Risks: {} · Checks: {}\n",
        state.changed_files.len(),
        state.risk_findings.len(),
        state.checks.len()
    ));
    output.push_str("Auto-refresh: 250ms · Controls: q/esc quit · r refresh now\n");
    output
}

fn harness_state_dir(workspace: &Path) -> Result<PathBuf> {
    let workspace = workspace.canonicalize()?;
    let git = GitRepo::new(workspace);
    git.ensure_repository()?;
    git.harness_state_dir()
}
