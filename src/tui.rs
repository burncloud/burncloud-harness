use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
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
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
    Frame, Terminal,
};

use crate::{
    git::GitRepo,
    run_history::{self, RunArtifact, RunReplay},
    run_state::{CheckState, RunState},
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const PHASES: [&str; 6] = ["AGENT", "SCOPE", "INVARIANTS", "RISK", "VERIFY", "FEEDBACK"];
const PANEL_GUTTER: u16 = 2;
const PANEL_PADDING: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelId {
    Summary,
    Boundaries,
    Loop,
    Context,
    AgentActivity,
    Checks,
    ChangesRisk,
    RecentEvents,
}

impl PanelId {
    fn title(self) -> &'static str {
        match self {
            Self::Summary => "运行摘要",
            Self::Boundaries => "硬边界",
            Self::Loop => "证据驱动循环",
            Self::Context => "运行上下文",
            Self::AgentActivity => "智能体活动",
            Self::Checks => "验证",
            Self::ChangesRisk => "变更与风险",
            Self::RecentEvents => "最近事件",
        }
    }

    fn coords(self) -> (usize, usize) {
        match self {
            Self::Summary => (0, 0),
            Self::Boundaries => (0, 1),
            Self::Loop => (1, 0),
            Self::Context => (1, 1),
            Self::AgentActivity => (2, 0),
            Self::Checks => (2, 1),
            Self::ChangesRisk => (3, 0),
            Self::RecentEvents => (3, 1),
        }
    }

    fn from_coords(row: usize, col: usize) -> Self {
        match (row, col) {
            (0, 0) => Self::Summary,
            (0, _) => Self::Boundaries,
            (1, 0) => Self::Loop,
            (1, _) => Self::Context,
            (2, 0) => Self::AgentActivity,
            (2, _) => Self::Checks,
            (3, 0) => Self::ChangesRisk,
            _ => Self::RecentEvents,
        }
    }

    fn moved(self, key: KeyCode) -> Self {
        let (row, col) = self.coords();
        let (next_row, next_col) = match key {
            KeyCode::Left => (row, col.saturating_sub(1)),
            KeyCode::Right => (row, (col + 1).min(1)),
            KeyCode::Up => (row.saturating_sub(1), col),
            KeyCode::Down => ((row + 1).min(3), col),
            _ => (row, col),
        };
        Self::from_coords(next_row, next_col)
    }
}

#[derive(Debug, Clone, Copy)]
struct UiState {
    focused: PanelId,
    zoomed: bool,
    scroll: u16,
}

impl UiState {
    fn escape_to_dashboard(&mut self) {
        self.zoomed = false;
        self.scroll = 0;
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            focused: PanelId::AgentActivity,
            zoomed: false,
            scroll: 0,
        }
    }
}

pub fn list_runs(workspace: &Path) -> Result<()> {
    let state_dir = harness_state_dir(workspace)?;
    let runs = run_history::discover(&state_dir)?;
    if runs.is_empty() {
        println!(
            "目标工作区 {} 中没有 Harness 运行记录。",
            workspace.display()
        );
        return Ok(());
    }
    for run in runs {
        println!("{}\t{}", run.run_id, source_zh(run.source.as_str()));
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
    drain_pending_input()?;
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

fn drain_pending_input() -> Result<()> {
    while event::poll(Duration::from_millis(0))? {
        let _ = event::read()?;
    }
    Ok(())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state_dir: &Path,
    requested_run: Option<&str>,
    artifact: &mut RunArtifact,
    replay: &mut RunReplay,
) -> Result<()> {
    let mut ui = UiState::default();
    loop {
        refresh_replay(state_dir, requested_run, artifact, replay)?;
        terminal.draw(|frame| draw(frame, replay, requested_run.is_none(), &ui))?;

        if event::poll(REFRESH_INTERVAL)? {
            if let TerminalEvent::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Esc => ui.escape_to_dashboard(),
                    KeyCode::Enter if !ui.zoomed => {
                        ui.zoomed = true;
                        ui.scroll = 0;
                    }
                    KeyCode::Up if ui.zoomed => ui.scroll = ui.scroll.saturating_sub(1),
                    KeyCode::Down if ui.zoomed => ui.scroll = ui.scroll.saturating_add(1),
                    KeyCode::PageUp if ui.zoomed => ui.scroll = ui.scroll.saturating_sub(8),
                    KeyCode::PageDown if ui.zoomed => ui.scroll = ui.scroll.saturating_add(8),
                    KeyCode::Home if ui.zoomed => ui.scroll = 0,
                    KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if !ui.zoomed => {
                        ui.focused = ui.focused.moved(key.code);
                        ui.scroll = 0;
                    }
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

fn draw(frame: &mut Frame, replay: &RunReplay, live: bool, ui: &UiState) {
    if ui.zoomed {
        draw_zoomed(frame, replay, ui);
    } else {
        draw_dashboard(frame, replay, live, ui);
    }
}

fn draw_dashboard(frame: &mut Frame, replay: &RunReplay, live: bool, ui: &UiState) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Min(18),
            Constraint::Length(3),
        ])
        .split(area);

    draw_header(frame, rows[0], &replay.state, live);
    let top = split_horizontal(rows[1], 57);
    draw_task_summary(frame, top[0], &replay.state);
    draw_boundaries(frame, top[1], &replay.state);
    let middle = split_horizontal(rows[2], 62);
    draw_loop_panel(frame, middle[0], &replay.state);
    draw_context(frame, middle[1], &replay.state);
    let bottom = split_horizontal(rows[3], 50);
    let bottom_left = split_vertical(bottom[0], 54);
    let bottom_right = split_vertical(bottom[1], 54);
    draw_agent_activity(frame, bottom_left[0], &replay.state);
    draw_changes_and_risk(frame, bottom_left[1], &replay.state);
    draw_checks(frame, bottom_right[0], &replay.state);
    draw_recent_events(frame, bottom_right[1], &replay.state);

    let focused_area = match ui.focused {
        PanelId::Summary => top[0],
        PanelId::Boundaries => top[1],
        PanelId::Loop => middle[0],
        PanelId::Context => middle[1],
        PanelId::AgentActivity => bottom_left[0],
        PanelId::ChangesRisk => bottom_left[1],
        PanelId::Checks => bottom_right[0],
        PanelId::RecentEvents => bottom_right[1],
    };
    frame.render_widget(focused_panel(ui.focused.title()), focused_area);
    draw_footer(frame, rows[4], replay, live, ui);
}

fn draw_zoomed(frame: &mut Frame, replay: &RunReplay, ui: &UiState) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);
    let lines = detailed_lines(ui.focused, &replay.state);
    let title = format!(" {} · 详细视图 ", ui.focused.title());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .padding(Padding::new(PANEL_PADDING, PANEL_PADDING, 0, 0));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((ui.scroll, 0)),
        rows[0],
    );
    let help = Line::from(vec![
        Span::styled(
            " 详细模式 ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{}  ", ui.focused.title())),
        Span::styled(
            "↑/↓ 滚动 · PgUp/PgDn 快速滚动 · Home 顶部 · Esc 返回 · q 退出",
            Style::default().fg(Color::White),
        ),
    ]);
    frame.render_widget(Paragraph::new(help).block(panel("观察器")), rows[1]);
}

fn detailed_lines(panel_id: PanelId, state: &RunState) -> Vec<Line<'static>> {
    match panel_id {
        PanelId::Summary => summary_lines(state),
        PanelId::Boundaries => boundary_lines(state, usize::MAX),
        PanelId::Loop => loop_lines(state, false),
        PanelId::Context => context_lines(state, usize::MAX),
        PanelId::AgentActivity => activity_lines(state, usize::MAX, false),
        PanelId::Checks => check_lines(state, usize::MAX),
        PanelId::ChangesRisk => change_lines(state, usize::MAX),
        PanelId::RecentEvents => event_lines(state, usize::MAX, false),
    }
}

fn draw_header(frame: &mut Frame, area: Rect, state: &RunState, live: bool) {
    let status = match state.status.as_str() {
        "PASSED" => Span::styled(
            " 通过 ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        "FAILED" => Span::styled(
            " 已停止 ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        _ => Span::styled(
            " 运行中 ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let elapsed = current_stage_elapsed_ms(state, now_ms())
        .map(format_duration)
        .unwrap_or_else(|| "--".into());
    let line = Line::from(vec![
        Span::styled(
            " BurnCloud Harness 监控台 ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        status,
        Span::raw("   "),
        Span::raw(format!(
            "{} {elapsed}   第 {} 轮   模式 {}",
            stage_zh(&state.stage),
            attempt_label(state),
            if live { "实时" } else { "回放" }
        )),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("控制平面")), area);
}

fn draw_task_summary(frame: &mut Frame, area: Rect, state: &RunState) {
    frame.render_widget(
        Paragraph::new(summary_lines(state))
            .block(panel("运行摘要"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn summary_lines(state: &RunState) -> Vec<Line<'static>> {
    let now = now_ms();
    let total = state
        .total_elapsed_ms(now)
        .map(format_duration)
        .unwrap_or_else(|| "--".into());
    let (passed, failed, running) = check_counts(state);
    let budget = match (state.agent_soft_limit_secs, state.agent_hard_limit_secs) {
        (Some(soft), Some(hard)) => format!(
            "soft {} · hard {}",
            format_duration(soft.saturating_mul(1_000)),
            format_duration(hard.saturating_mul(1_000))
        ),
        _ => "未配置".to_owned(),
    };
    vec![
        kv("任务", &state.task),
        kv("运行", value_or(&state.run_id, "-")),
        kv(
            "进度",
            &format!(
                "第 {} 轮 · {}",
                attempt_label(state),
                stage_zh(&state.stage)
            ),
        ),
        kv("总耗时", &total),
        kv("活动", &agent_activity_summary(state, now)),
        kv(
            "验证",
            &format!("通过 {passed} · 失败 {failed} · 运行 {running}"),
        ),
        kv(
            "状态",
            &format!(
                "变更 {} · 越界 {} · 风险 {}",
                state.changed_files.len(),
                state.violations.len(),
                state.risk_findings.len()
            ),
        ),
        kv("Agent预算", &budget),
    ]
}

fn draw_boundaries(frame: &mut Frame, area: Rect, state: &RunState) {
    frame.render_widget(
        Paragraph::new(boundary_lines(state, 3))
            .block(panel("硬边界"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn boundary_lines(state: &RunState, limit: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("允许 {} 条  ", state.allowed.len()),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("禁止 {} 条", state.avoid.len()),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    ])];
    for path in state.allowed.iter().take(limit) {
        lines.push(Line::from(vec![
            Span::styled("+  ", Style::default().fg(Color::Green)),
            Span::raw(path.clone()),
        ]));
    }
    for path in state.avoid.iter().take(limit) {
        lines.push(Line::from(vec![
            Span::styled("-  ", Style::default().fg(Color::Red)),
            Span::raw(path.clone()),
        ]));
    }
    if state.violations.is_empty() {
        lines.push(Line::from(Span::styled(
            "边界状态  当前无越界修改",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for violation in &state.violations {
            lines.push(Line::from(Span::styled(
                format!("越界  {violation}"),
                Style::default().fg(Color::Red),
            )));
        }
    }
    lines
}

fn draw_loop_panel(frame: &mut Frame, area: Rect, state: &RunState) {
    frame.render_widget(
        Paragraph::new(loop_lines(state, true))
            .block(panel("证据驱动循环"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn loop_lines(state: &RunState, compact_failure_detail: bool) -> Vec<Line<'static>> {
    let now = now_ms();
    let mut phase_spans = Vec::new();
    for (index, phase) in PHASES.iter().enumerate() {
        if index > 0 {
            phase_spans.push(Span::styled(" → ", Style::default().fg(Color::DarkGray)));
        }
        let style = if state.stage == *phase {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(Color::White)
        };
        phase_spans.push(Span::styled(format!(" {} ", stage_zh(phase)), style));
    }
    let mut lines = vec![
        Line::from(phase_spans),
        kv("当前阶段", stage_zh(&state.stage)),
        timing_line(state, &PHASES[..3], now),
        timing_line(state, &PHASES[3..], now),
    ];
    if state.failures.is_empty() {
        lines.push(Line::from(Span::styled(
            "决策  尚未产生重试或停止原因",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let limit = if compact_failure_detail {
            1
        } else {
            usize::MAX
        };
        let start = state.failures.len().saturating_sub(limit);
        for failure in &state.failures[start..] {
            let detail = if compact_failure_detail {
                compact(&failure.detail, 88)
            } else {
                failure.detail.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "决策 #{} {}  ",
                        failure.attempt,
                        failure_class_zh(&failure.class)
                    ),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(detail),
            ]));
        }
    }
    lines
}

fn timing_line(state: &RunState, phases: &[&str], now: u64) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, phase) in phases.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("    "));
        }
        let active = state.stage == *phase;
        let text = if active {
            format!("{} ↻", stage_zh(phase))
        } else {
            let elapsed = state
                .stage_elapsed_ms(phase, state.attempt, now)
                .map(format_duration)
                .unwrap_or_else(|| "--".into());
            format!("{} {elapsed}", stage_zh(phase))
        };
        spans.push(Span::styled(
            text,
            if active {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            },
        ));
    }
    Line::from(spans)
}

fn draw_context(frame: &mut Frame, area: Rect, state: &RunState) {
    frame.render_widget(
        Paragraph::new(context_lines(state, 3))
            .block(panel("运行上下文"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn context_lines(state: &RunState, limit: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("不变量 {}  ", state.invariants.len()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("新增 {}", state.newly_required.len()),
            Style::default().fg(Color::Yellow),
        ),
    ])];
    for invariant in state.invariants.iter().take(limit) {
        let is_new = state.newly_required.iter().any(|item| item == invariant);
        lines.push(Line::from(vec![
            Span::styled(
                if is_new { "+  " } else { "•  " },
                Style::default().fg(if is_new { Color::Yellow } else { Color::Cyan }),
            ),
            Span::raw(invariant.clone()),
        ]));
    }
    lines.push(Line::from(Span::styled(
        format!("路由 {}", state.routes.len()),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    for route in state.routes.iter().take(limit) {
        lines.push(Line::from(format!("• {route}")));
    }
    lines.push(Line::from(Span::styled(
        format!("关键事件 {}", state.timeline.len()),
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

fn draw_agent_activity(frame: &mut Frame, area: Rect, state: &RunState) {
    frame.render_widget(
        Paragraph::new(activity_lines(state, 5, true))
            .block(panel("智能体活动"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn activity_lines(state: &RunState, limit: usize, compact_lines: bool) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        agent_activity_summary(state, now_ms()),
        Style::default().fg(Color::DarkGray),
    ))];
    if state.agent_activity.is_empty() {
        lines.push(Line::from(Span::styled(
            "等待有意义的智能体活动",
            Style::default().fg(Color::DarkGray),
        )));
        return lines;
    }
    let start = state.agent_activity.len().saturating_sub(limit);
    for activity in &state.agent_activity[start..] {
        let (marker, color) = activity_marker(&activity.stream);
        let text = if compact_lines {
            compact(&activity.line, 84)
        } else {
            activity.line.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} #{}  ", activity.attempt),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(text),
        ]));
    }
    lines
}

fn activity_marker(stream: &str) -> (&'static str, Color) {
    match stream {
        "stderr" => ("错误", Color::Red),
        "change_intent" => ("计划", Color::Yellow),
        "change_result" => ("完成", Color::Green),
        _ => ("活动", Color::Cyan),
    }
}

fn draw_changes_and_risk(frame: &mut Frame, area: Rect, state: &RunState) {
    frame.render_widget(
        Paragraph::new(change_lines(state, 3))
            .block(panel("变更与风险"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn change_lines(state: &RunState, limit: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("变更 {}  ", state.changed_files.len()),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("风险 {}  ", state.risk_findings.len()),
            Style::default()
                .fg(if state.risk_findings.is_empty() {
                    Color::Green
                } else {
                    Color::Yellow
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("越界 {}", state.violations.len()),
            Style::default()
                .fg(if state.violations.is_empty() {
                    Color::Green
                } else {
                    Color::Red
                })
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    if state.changed_files.is_empty() {
        lines.push(Line::from(Span::styled(
            "尚未进入 Git 差异检查",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for path in state.changed_files.iter().take(limit) {
        lines.push(Line::from(format!("• {path}")));
    }
    for finding in state.risk_findings.iter().take(limit) {
        lines.push(Line::from(Span::styled(
            format!("风险  {finding}"),
            Style::default().fg(Color::Yellow),
        )));
    }
    for violation in state.violations.iter().take(limit) {
        lines.push(Line::from(Span::styled(
            format!("越界  {violation}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines
}

fn draw_checks(frame: &mut Frame, area: Rect, state: &RunState) {
    frame.render_widget(
        Paragraph::new(check_lines(state, 6))
            .block(panel("验证"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn check_lines(state: &RunState, limit: usize) -> Vec<Line<'static>> {
    let now = now_ms();
    let (passed, failed, running) = check_counts(state);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("通过 {passed}  "),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("失败 {failed}  "),
            Style::default()
                .fg(if failed == 0 {
                    Color::DarkGray
                } else {
                    Color::Red
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("运行 {running}"),
            Style::default()
                .fg(if running == 0 {
                    Color::DarkGray
                } else {
                    Color::Yellow
                })
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    if state.checks.is_empty() {
        lines.push(Line::from(Span::styled(
            "本轮验证尚未开始",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for check in state.checks.iter().take(limit) {
        let (marker, color) = match check.success {
            Some(true) => ("通过", Color::Green),
            Some(false) => ("失败", Color::Red),
            None => ("运行", Color::Yellow),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker}  "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(check.name.clone()),
            Span::styled(
                format!("  {}", check_elapsed(check, now)),
                Style::default().fg(Color::Cyan),
            ),
        ]));
    }
    lines
}

fn draw_recent_events(frame: &mut Frame, area: Rect, state: &RunState) {
    frame.render_widget(
        Paragraph::new(event_lines(state, 6, true))
            .block(panel("最近事件"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn event_lines(state: &RunState, limit: usize, compact_details: bool) -> Vec<Line<'static>> {
    if state.timeline.is_empty() {
        return vec![Line::from(Span::styled(
            "等待事件",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let start = state.timeline.len().saturating_sub(limit);
    state.timeline[start..]
        .iter()
        .map(|event| {
            let detail = if compact_details {
                event_detail_zh(&event.name, &event.detail)
            } else {
                event.detail.clone()
            };
            Line::from(vec![
                Span::styled(
                    format!("{:<10}", event_name_zh(&event.name)),
                    Style::default().fg(Color::Magenta),
                ),
                Span::raw(detail),
            ])
        })
        .collect()
}

fn draw_footer(frame: &mut Frame, area: Rect, replay: &RunReplay, live: bool, ui: &UiState) {
    let (passed, failed, running) = check_counts(&replay.state);
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", if live { "实时" } else { "回放" }),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " 焦点={}  运行={}  ",
            ui.focused.title(),
            replay.state.run_id
        )),
        Span::raw(format!(
            "变更={}  风险={}  验证={passed}/{failed}/{running}  ",
            replay.state.changed_files.len(),
            replay.state.risk_findings.len()
        )),
        Span::styled(
            "←↑↓→ 选择 · Enter 详情 · Esc 首页 · r 刷新 · q 退出",
            Style::default().fg(Color::White),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("观察器")), area);
}

fn focused_panel(title: &'static str) -> Block<'static> {
    panel(title).border_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

fn attempt_label(state: &RunState) -> String {
    if state.max_loops == 0 {
        state.attempt.to_string()
    } else {
        format!("{}/{}", state.attempt, state.max_loops)
    }
}

fn current_stage_elapsed_ms(state: &RunState, now: u64) -> Option<u64> {
    match state.stage.as_str() {
        "TASK" | "PREPARE" => preparation_elapsed_ms(state, now),
        "ROUTE" => state.stage_elapsed_ms("ROUTE", 0, now),
        "DONE" => Some(0),
        stage => state.stage_elapsed_ms(stage, state.attempt, now),
    }
}

fn preparation_elapsed_ms(state: &RunState, now: u64) -> Option<u64> {
    let started = state.started_ms?;
    let route_started = state
        .timings
        .iter()
        .find(|timing| timing.stage == "ROUTE")
        .map(|timing| timing.started_ms)
        .unwrap_or(now);
    Some(route_started.saturating_sub(started))
}

fn agent_activity_summary(state: &RunState, now: u64) -> String {
    let age = state
        .agent_last_output_ms
        .map(|timestamp| {
            format!(
                "最后活动 {} 前",
                format_duration(now.saturating_sub(timestamp))
            )
        })
        .unwrap_or_else(|| "尚无活动".into());
    let liveness = if state.agent_hard_timed_out {
        " · 已超时"
    } else if state.agent_idle_warning_active {
        " · 空闲告警"
    } else if state.agent_heartbeat_elapsed_secs.is_some() {
        " · 在线"
    } else {
        ""
    };
    format!("{age}{liveness}")
}

fn check_counts(state: &RunState) -> (usize, usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    let mut running = 0;
    for check in &state.checks {
        match check.success {
            Some(true) => passed += 1,
            Some(false) => failed += 1,
            None => running += 1,
        }
    }
    (passed, failed, running)
}

fn check_elapsed(check: &CheckState, now_ms: u64) -> String {
    if let Some(duration) = check.duration_ms {
        return format_duration(duration);
    }
    if let Some(started) = check.started_ms {
        return format!("{} ↻", format_duration(now_ms.saturating_sub(started)));
    }
    "--".into()
}

fn format_duration(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        return format!("{:.1}秒", ms as f64 / 1_000.0);
    }
    let minutes = ms / 60_000;
    let seconds = (ms % 60_000) as f64 / 1_000.0;
    if minutes < 60 {
        return format!("{minutes}分{seconds:.1}秒");
    }
    format!("{}小时{}分", minutes / 60, minutes % 60)
}

fn stage_zh(stage: &str) -> &'static str {
    match stage {
        "TASK" | "PREPARE" => "任务准备",
        "ROUTE" => "路由",
        "AGENT" => "智能体",
        "SCOPE" => "范围检查",
        "INVARIANTS" => "不变量",
        "RISK" => "风险检查",
        "VERIFY" => "验证",
        "FEEDBACK" => "反馈",
        "DONE" => "完成",
        _ => "未知",
    }
}

fn event_name_zh(name: &str) -> &'static str {
    match name {
        "task_started" | "run_started" => "任务开始",
        "contract_loaded" => "契约加载",
        "route_selected" | "task_routed" => "路由选择",
        "invariant_selected" | "invariants_selected" => "不变量选择",
        "loop_started" | "attempt_started" => "循环开始",
        "stage_started" => "阶段开始",
        "agent_started" => "智能体开始",
        "agent_output" => "智能体输出",
        "agent_heartbeat" => "智能体心跳",
        "agent_finished" => "智能体结束",
        "diff_detected" => "发现变更",
        "scope_evaluated" => "范围检查",
        "invariant_expanded" | "invariant_impact_assessed" => "不变量检查",
        "risk_detected" | "risk_assessed" => "风险检查",
        "verification_started" | "check_started" => "验证开始",
        "verification_finished" | "check_finished" => "验证结束",
        "failure_recorded" => "失败记录",
        "retry_requested" | "attempt_failed" => "请求重试",
        "task_finished" | "run_finished" => "任务结束",
        _ => "事件",
    }
}

fn event_detail_zh(name: &str, detail: &str) -> String {
    match (name, detail) {
        ("scope_evaluated", "scope accepted") => "范围检查通过".into(),
        ("risk_detected" | "risk_assessed", "no risk findings") => "未发现风险".into(),
        ("stage_started", stage) => format!("进入{}", stage_zh(stage)),
        _ => compact(detail, 72),
    }
}

fn failure_class_zh(class: &str) -> &'static str {
    match class {
        "agent_command" => "智能体失败",
        "git_history" => "Git 历史变更",
        "scope_violation" => "范围越界",
        "invariant_expansion" => "不变量扩展",
        "no_change" => "没有变更",
        "risk_block" => "风险阻断",
        "risk_review" => "风险复核",
        "verification" => "验证失败",
        "max_loops" => "达到循环上限",
        "retry" => "重试",
        _ => "失败",
    }
}

fn source_zh(source: &str) -> &'static str {
    match source {
        "events" => "事件流",
        "trajectory" => "轨迹",
        _ => "未知",
    }
}

fn split_horizontal(area: Rect, left_percent: u16) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left_percent),
            Constraint::Length(PANEL_GUTTER),
            Constraint::Min(0),
        ])
        .split(area);
    [chunks[0], chunks[2]]
}

fn split_vertical(area: Rect, top_percent: u16) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(top_percent),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
    [chunks[0], chunks[2]]
}

fn panel(title: &'static str) -> Block<'static> {
    Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::new(PANEL_PADDING, PANEL_PADDING, 0, 0))
}

fn kv(label: &'static str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}  "),
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
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
    fn focus_moves_across_dashboard_grid() {
        assert_eq!(
            PanelId::AgentActivity.moved(KeyCode::Right),
            PanelId::Checks
        );
        assert_eq!(
            PanelId::AgentActivity.moved(KeyCode::Down),
            PanelId::ChangesRisk
        );
        assert_eq!(PanelId::RecentEvents.moved(KeyCode::Up), PanelId::Checks);
        assert_eq!(PanelId::Summary.moved(KeyCode::Left), PanelId::Summary);
    }

    #[test]
    fn tui_starts_on_dashboard_and_esc_returns_to_dashboard() {
        let mut ui = UiState::default();
        assert!(!ui.zoomed);

        ui.escape_to_dashboard();
        assert!(!ui.zoomed);

        ui.zoomed = true;
        ui.scroll = 8;
        ui.escape_to_dashboard();
        assert!(!ui.zoomed);
        assert_eq!(ui.scroll, 0);
    }

    #[test]
    fn detailed_activity_keeps_full_agent_explanation() {
        let mut state = RunState::default();
        state.agent_activity.push(crate::run_state::AgentActivity {
            attempt: 1,
            stream: "change_intent".into(),
            line: "计划修改 dashboard.rs，因为 Buyer Overview 的指标卡几何和 source reference 不一致；验证方式是页面视觉对照。".into(),
            timestamp_ms: Some(1),
        });
        let lines = detailed_lines(PanelId::AgentActivity, &state);
        let rendered = format!("{:?}", lines);
        assert!(rendered.contains("计划修改 dashboard.rs"));
        assert!(rendered.contains("页面视觉对照"));
    }

    #[test]
    fn activity_summary_uses_heartbeat_as_status_not_elapsed_time() {
        let mut state = RunState::default();
        state.agent_last_output_ms = Some(1_000);
        state.agent_heartbeat_elapsed_secs = Some(125);
        let summary = agent_activity_summary(&state, 2_000);
        assert!(summary.contains("在线"));
        assert!(!summary.contains("125秒"));
    }

    #[test]
    fn active_phase_timing_does_not_repeat_live_elapsed_time() {
        let mut state = RunState::default();
        state.stage = "AGENT".into();
        state.attempt = 1;
        state.timings.push(crate::run_state::StageTiming {
            attempt: 1,
            stage: "AGENT".into(),
            started_ms: 1_000,
            duration_ms: None,
        });
        let rendered = format!("{:?}", timing_line(&state, &["AGENT"], 21_000));
        assert!(rendered.contains("智能体 ↻"));
        assert!(!rendered.contains("20.0秒"));
    }
}
