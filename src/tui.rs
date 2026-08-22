use anyhow::Result;
use crossterm::{execute, terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}};
use ratatui::{backend::CrosstermBackend, widgets::{Block, Borders, Paragraph}, Terminal};
use serde_json::Value;
use std::{fs, io::{self, Write}, path::PathBuf};

pub fn run() -> Result<()> {
    let events = load_latest_events()?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    terminal.draw(|frame| {
        let area = frame.area();
        let lines = render_events(&events);
        let widget = Paragraph::new(lines)
            .block(Block::default().title("BurnCloud Harness Monitor").borders(Borders::ALL));
        frame.render_widget(widget, area);
    })?;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn load_latest_events() -> Result<Vec<Value>> {
    let path = PathBuf::from(".git/burncloud-harness/runs/latest/events.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path)?;
    Ok(content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect())
}

fn render_events(events: &[Value]) -> String {
    let mut output = String::new();
    output.push_str("Task: latest\n\nTimeline:\n");

    for event in events.iter().rev().take(15).rev() {
        let name = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        output.push_str(&format!("✓ {}\n", name.to_uppercase()));
    }

    output
}
