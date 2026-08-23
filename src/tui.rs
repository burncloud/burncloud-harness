use std::{
    io::{self, Write},
    path::PathBuf,
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

pub fn list_runs() -> Result<()> {
    let state_dir = harness_state_dir()?;
    let runs = run_history::discover(&state_dir)?;
    if runs.is_empty() {
        println!("No Harness runs found.");
        return Ok(());
    }

    for run in runs {
        println!("{}\t{}", run.run_id, run.source.as_str());
    }
    Ok(())
}

pub fn run(requested_run: Option<&str>) -> Result<()> {
    let state_dir = harness_state_dir()?;
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
    state_dir: &std::path::Path,
    requested_run: Option<&str>,
    artifact: &mut RunArtifact,
    replay: &mut RunReplay,
) -> Result<()> {
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let widget = Paragraph::new(render_replay(replay)).block(
                Block::default()
                    .title("BurnCloud Harness Monitor")
                    .borders(Borders::ALL),
            );
            frame.render_widget(widget, area);
        })?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        if let TerminalEvent::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('r') => {
                    *artifact = run_history::resolve(state_dir, requested_run)?;
                    *replay = run_history::load(artifact)?;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn render_replay(replay: &RunReplay) -> String {
    let mut output = String::new();
    output.push_str(&format!("Run ID: {}\n", replay.run_id));
    output.push_str(&format!("Task: {}\n", replay.task));
    output.push_str(&format!("Status: {}\n", replay.status));
    output.push_str(&format!("Source: {}\n\n", replay.source.as_str()));
    output.push_str("Timeline:\n");

    for event in replay.events.iter().rev().take(20).rev() {
        output.push_str(&format!(
            "✓ {:<24} {}\n",
            event.name.to_uppercase(),
            event.detail
        ));
    }

    output.push_str("\nControls: q/esc quit · r refresh\n");
    output
}

fn harness_state_dir() -> Result<PathBuf> {
    let workspace = PathBuf::from(".").canonicalize()?;
    let git = GitRepo::new(workspace);
    git.ensure_repository()?;
    git.harness_state_dir()
}
