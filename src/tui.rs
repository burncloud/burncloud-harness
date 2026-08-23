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
    draw_loop(frame, middle[0], &replay.state);
    draw_context(frame, middle[1], &replay.state);

    let bottom = split_horizontal(rows[3], 50);
    let bottom_left = split_vertical(bottom[0], 54);
    draw_agent_activity(frame, bottom_left[0], &replay.state);
    draw_changes_and_risk(frame, bottom_left[1], &replay.state);

    let bottom_right = split_vertical(bottom[1], 54);
    draw_checks(frame, bottom_right[0], &replay.state);
    draw_recent_events(frame, bottom_right[1], &replay.state);

    draw_footer(frame, rows[4], replay, live);
}

fn draw_header(frame: &mut Frame, area: Rect, state: &RunState, live: bool) {
    let now = now_ms();
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
    let attempt = attempt_label(state);
    let stage_elapsed = current_stage_elapsed_ms(state, now)
        .map(format_duration)
        .unwrap_or_else(|| "--".to_owned());

    let line = Line::from(vec![
        Span::styled(
            " BurnCloud Harness 监控台 ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        status,
        Span::raw("   "),
        Span::styled(
            format!("{} {stage_elapsed}", stage_zh(&state.stage)),
            Style::default().fg(Color::White),
        ),
        Span::raw("   "),
        Span::styled(format!("第 {attempt} 轮"), Style::default().fg(Color::Cyan)),
        Span::raw("   "),
        Span::styled(format!("模式 {mode}"), Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("控制平面")), area);
}

fn draw_task_summary(frame: &mut Frame, area: Rect, state: &RunState) {
    let now = now_ms();
    let total = state
        .total_elapsed_ms(now)
        .map(format_duration)
        .unwrap_or_else(|| "--".to_owned());
    let stage_elapsed = current_stage_elapsed_ms(state, now)
        .map(format_duration)
        .unwrap_or_else(|| "--".to_owned());
    let activity = agent_activity_summary(state, now);
    let (passed, failed, running) = check_counts(state);

    let lines = vec![
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
        kv("阶段耗时", &stage_elapsed),
        kv("总耗时", &total),
        kv("活动", &activity),
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
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("运行摘要"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_boundaries(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
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
    ]));

    if state.allowed.is_empty() {
        lines.push(Line::from(Span::styled(
            "等待任务范围",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for path in state.allowed.iter().take(3) {
            lines.push(Line::from(vec![
                Span::styled("+  ", Style::default().fg(Color::Green)),
                Span::raw(compact(path, 68)),
            ]));
        }
    }

    for path in state.avoid.iter().take(2) {
        lines.push(Line::from(vec![
            Span::styled("-  ", Style::default().fg(Color::Red)),
            Span::raw(compact(path, 68)),
        ]));
    }

    let boundary = if state.violations.is_empty() {
        Span::styled(
            "边界状态  当前无越界修改",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::styled(
            format!("边界状态  发现 {} 项越界", state.violations.len()),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    };
    lines.push(Line::from(boundary));

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

    let current_elapsed = current_stage_elapsed_ms(state, now)
        .map(format_duration)
        .unwrap_or_else(|| "--".to_owned());
    let mut lines = vec![Line::from(spans)];
    lines.push(Line::from(vec![
        Span::styled(
            "当前阶段  ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} · {current_elapsed}", stage_zh(&state.stage)),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "本轮各阶段",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(timing_line(state, &PHASES[..3], now));
    lines.push(timing_line(state, &PHASES[3..], now));

    if state.failures.is_empty() {
        lines.push(Line::from(Span::styled(
            "决策  尚未产生重试或停止原因",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let failure = state.failures.last().expect("failure list is not empty");
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "决策  #{} {}  ",
                    failure.attempt,
                    failure_class_zh(&failure.class)
                ),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(compact(&failure.detail, 88)),
        ]));
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
            spans.push(Span::raw("    "));
        }
        let elapsed = state
            .stage_elapsed_ms(phase, state.attempt, now)
            .map(format_duration)
            .unwrap_or_else(|| "--".to_owned());
        let running = if state.stage == *phase { " ↻" } else { "" };
        spans.push(Span::styled(
            format!("{} {elapsed}{running}", stage_zh(phase)),
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

fn draw_context(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
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
    ]));

    if state.invariants.is_empty() {
        lines.push(Line::from(Span::styled(
            "尚未加载不变量",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for invariant in state.invariants.iter().take(3) {
            let is_new = state.newly_required.iter().any(|item| item == invariant);
            let marker = if is_new { "+  " } else { "•  " };
            lines.push(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(if is_new { Color::Yellow } else { Color::Cyan }),
                ),
                Span::raw(compact(invariant, 68)),
            ]));
        }
    }

    lines.push(Line::from(Span::styled(
        format!("路由 {}", state.routes.len()),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    for route in state.routes.iter().take(2) {
        lines.push(Line::from(format!("• {}", compact(route, 70))));
    }
    lines.push(Line::from(Span::styled(
        format!("关键事件 {}", state.timeline.len()),
        Style::default().fg(Color::DarkGray),
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("运行上下文"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_agent_activity(frame: &mut Frame, area: Rect, state: &RunState) {
    let now = now_ms();
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        agent_activity_summary(state, now),
        Style::default().fg(Color::DarkGray),
    )));

    if state.agent_activity.is_empty() {
        lines.push(Line::from(Span::styled(
            "等待有意义的智能体活动",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for activity in state.agent_activity.iter().rev().take(5).rev() {
            let (marker, color) = if activity.stream == "stderr" {
                ("错误", Color::Red)
            } else {
                ("活动", Color::Cyan)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{marker}  "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(compact(&activity.line, 84)),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("智能体活动"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_changes_and_risk(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
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
    ]));

    if state.changed_files.is_empty() {
        lines.push(Line::from(Span::styled(
            "尚未进入 Git 差异检查",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for path in state.changed_files.iter().take(3) {
            lines.push(Line::from(format!("• {}", compact(path, 84))));
        }
    }

    if let Some(finding) = state.risk_findings.first() {
        lines.push(Line::from(Span::styled(
            format!("风险  {}", compact(finding, 78)),
            Style::default().fg(Color::Yellow),
        )));
    }
    if let Some(violation) = state.violations.first() {
        lines.push(Line::from(Span::styled(
            format!("越界  {}", compact(violation, 78)),
            Style::default().fg(Color::Red),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("变更与风险"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_checks(frame: &mut Frame, area: Rect, state: &RunState) {
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
    } else {
        for check in state.checks.iter().take(6) {
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
                Span::raw(compact(&check.name, 56)),
                Span::styled(
                    format!("  {}", check_elapsed(check, now)),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("验证"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_recent_events(frame: &mut Frame, area: Rect, state: &RunState) {
    let mut lines = Vec::new();
    for event in state.timeline.iter().rev().take(6).rev() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<10}", event_name_zh(&event.name)),
                Style::default().fg(Color::Magenta),
            ),
            Span::raw(event_detail_zh(&event.name, &event.detail)),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "等待事件",
            Style::default().fg(Color::DarkGray),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("最近事件"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, replay: &RunReplay, live: bool) {
    let mode = if live { "实时" } else { "回放" };
    let (passed, failed, running) = check_counts(&replay.state);
    let line = Line::from(vec![
        Span::styled(
            format!(" {mode} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" 运行={}  ", replay.state.run_id)),
        Span::styled(
            format!("事件={}  ", replay.state.timeline.len()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(format!(
            "变更={}  风险={}  验证={passed}/{failed}/{running}  ",
            replay.state.changed_files.len(),
            replay.state.risk_findings.len()
        )),
        Span::styled(
            format!("来源={}  ", source_zh(replay.source.as_str())),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("刷新=250ms  "),
        Span::styled("q/esc 退出 · r 刷新", Style::default().fg(Color::White)),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("观察器")), area);
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
        .unwrap_or_else(|| "尚无活动".to_owned());
    let heartbeat = state
        .agent_heartbeat_elapsed_secs
        .map(|seconds| format!(" · 心跳 {seconds}秒"))
        .unwrap_or_default();
    format!("{age}{heartbeat}")
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

fn split_horizontal(area: Rect, left_percent: u16) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left_percent),
            Constraint::Length(1),
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
