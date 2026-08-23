use std::{
    io::{self, Write},
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
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};

use crate::{
    git::GitRepo,
    run_history::{self, RunArtifact, RunReplay},
    run_state::{CheckState, RunState},
};

const REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const PHASES: [&str; 6] = ["AGENT", "SCOPE", "INVARIANTS", "RISK", "VERIFY", "FEEDBACK"];

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
            Constraint::Length(9),
            Constraint::Length(12),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);

    draw_header(frame, rows[0], &replay.state, live);

    let top = split_horizontal(rows[1], 56, 44);
    draw_task(frame, top[0], &replay.state);
    draw_boundaries(frame, top[1], &replay.state);

    let middle = split_horizontal(rows[2], 62, 38);
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
    let mode = if live { "实时" } else { "回放" };
    let line = Line::from(vec![
        Span::styled(
            " BurnCloud Harness 监控台 ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        status,
        Span::raw("   "),
        Span::styled(format!("模式 {mode}"), Style::default().fg(Color::White)),
        Span::raw("   "),
        Span::styled(
            "事件 → 状态归约 → 观察器",
            Style::default().fg(Color::Magenta),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("控制平面")), area);
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
        .map(|event| event_summary_zh(&event.name, &event.detail))
        .unwrap_or_else(|| "等待事件".to_owned());
    let elapsed = state
        .total_elapsed_ms(now_ms())
        .map(format_duration)
        .unwrap_or_else(|| "--".to_owned());
    let lines = vec![
        kv("任务", &state.task),
        kv("运行 ID", value_or(&state.run_id, "-")),
        kv("区域", &state.area),
        kv("循环", &attempt),
        kv("阶段", stage_zh(&state.stage)),
        kv("总耗时", &elapsed),
        kv("当前", &current),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("任务契约"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_boundaries(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    if state.allowed.is_empty() {
        lines.push(Line::from(Span::styled(
            "允许  等待任务范围",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for path in state.allowed.iter().take(2) {
            lines.push(Line::from(vec![
                Span::styled(
                    "允许  ",
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
                "禁止  ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(compact(path, 70)),
        ]));
    }
    if state.violations.is_empty() {
        lines.push(Line::from(Span::styled(
            "边界  当前无越界修改",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for path in state.violations.iter().take(2) {
            lines.push(Line::from(Span::styled(
                format!("越界  {}", compact(path, 66)),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("硬边界"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_loop(frame: &mut Frame, area: Rect, state: &RunState) {
    let now = now_ms();
    let mut spans = Vec::new();
    for (index, phase) in PHASES.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" → ", Style::default().fg(Color::DarkGray)));
        }
        let active = state.stage == *phase;
        let style = if active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(format!(" {} ", stage_zh(phase)), style));
    }

    let mut lines = vec![Line::from(spans)];
    lines.push(Line::from(Span::styled(
        "本轮阶段耗时",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(timing_line(state, &PHASES[..3], now));
    lines.push(timing_line(state, &PHASES[3..], now));

    if state.failures.is_empty() {
        lines.push(Line::from(Span::styled(
            "尚未重试；只有证据产生反馈时才进入下一轮。",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "为什么重试",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
        for failure in state.failures.iter().rev().take(4).rev() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "#{} {}  ",
                        failure.attempt,
                        failure_class_zh(&failure.class)
                    ),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(compact(&failure.detail, 96)),
            ]));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("证据驱动循环"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn timing_line(state: &RunState, phases: &[&str], now: u64) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, phase) in phases.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        let elapsed = state
            .stage_elapsed_ms(phase, state.attempt, now)
            .map(format_duration)
            .unwrap_or_else(|| "--".to_owned());
        let running = if state.stage == *phase { " ↻" } else { "" };
        spans.push(Span::styled(
            format!("{} {}{}", stage_zh(phase), elapsed, running),
            if state.stage == *phase {
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

fn draw_invariants(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    for invariant in state.invariants.iter().take(5) {
        let is_new = state.newly_required.iter().any(|item| item == invariant);
        let marker = if is_new { "+新增 " } else { "保持  " };
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
            "尚未加载不变量",
            Style::default().fg(Color::DarkGray),
        )));
    }
    if !state.routes.is_empty() {
        lines.push(Line::from(Span::styled(
            "路由依据：",
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
            .block(panel("不变量 + 路由"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_changes_and_risk(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    if state.changed_files.is_empty() {
        lines.push(Line::from(Span::styled(
            "尚未发现文件变更",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "变更文件",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for path in state.changed_files.iter().take(6) {
            lines.push(Line::from(format!("• {}", compact(path, 82))));
        }
    }

    lines.push(Line::from(Span::styled(
        "风险",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    if state.risk_findings.is_empty() {
        lines.push(Line::from(Span::styled(
            "当前未发现确定性风险",
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
            .block(panel("变更路径 + 风险"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_checks(frame: &mut Frame, area: Rect, state: &RunState) {
    let now = now_ms();
    let mut lines = Vec::new();
    if state.checks.is_empty() {
        lines.push(Line::from(Span::styled(
            "本轮验证尚未开始",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for check in state.checks.iter().take(7) {
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
                Span::raw(compact(&check.name, 58)),
                Span::styled(
                    format!("  {}", check_elapsed(check, now)),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }
    }

    lines.push(Line::from(Span::styled(
        "最近事件",
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    )));
    for event in state.timeline.iter().rev().take(4).rev() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<14}", event_name_zh(&event.name)),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(event_detail_zh(&event.name, &event.detail)),
        ]));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("验证 + 最近事件"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, replay: &RunReplay, live: bool) {
    let mode = if live { "实时" } else { "回放" };
    let line = Line::from(vec![
        Span::styled(
            format!(" {mode} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" 运行={}  ", replay.state.run_id)),
        Span::styled(
            format!("来源={}  ", source_zh(replay.source.as_str())),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("自动刷新=250ms  "),
        Span::styled("q/esc 退出 · r 立即刷新", Style::default().fg(Color::White)),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("观察器")), area);
}

fn check_elapsed(check: &CheckState, now_ms: u64) -> String {
    if let Some(duration) = check.duration_ms {
        return format_duration(duration);
    }
    if let Some(started) = check.started_ms {
        return format!("{} ↻", format_duration(now_ms.saturating_sub(started)));
    }
    "--".to_owned()
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
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}小时{minutes}分")
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

fn event_summary_zh(name: &str, detail: &str) -> String {
    format!(
        "{} · {}",
        event_name_zh(name),
        event_detail_zh(name, detail)
    )
}

fn event_detail_zh(name: &str, detail: &str) -> String {
    match (name, detail) {
        ("scope_evaluated", "scope accepted") => "范围检查通过".to_owned(),
        ("risk_detected" | "risk_assessed", "no risk findings") => "未发现风险".to_owned(),
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

    #[test]
    fn duration_is_human_readable_in_chinese() {
        assert_eq!(format_duration(420), "420ms");
        assert_eq!(format_duration(1_500), "1.5秒");
        assert_eq!(format_duration(125_400), "2分5.4秒");
    }
}
