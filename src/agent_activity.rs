use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratedAgentLine {
    pub stream: String,
    pub line: String,
}

const CHANGE_INTENT_PREFIX: &str = "HARNESS_CHANGE_INTENT ";
const CHANGE_RESULT_PREFIX: &str = "HARNESS_CHANGE_RESULT ";

pub fn curate(stream: &str, line: &str) -> Option<CuratedAgentLine> {
    let line = normalize(line);
    if line.is_empty() {
        return None;
    }

    if let Some(curated) = curate_change_protocol(&line) {
        return Some(curated);
    }

    if is_error(&line) {
        return Some(CuratedAgentLine {
            stream: "stderr".to_owned(),
            line: compact(&line, 600),
        });
    }

    if looks_mojibake(&line) || looks_like_source_dump(&line) {
        return None;
    }

    if is_progress(&line) {
        return Some(CuratedAgentLine {
            // Codex commonly writes normal progress to stderr. Live observers should classify
            // by meaning, not by the child process file descriptor.
            stream: "stdout".to_owned(),
            line: compact(&line, 400),
        });
    }

    let _ = stream;
    None
}

fn curate_change_protocol(line: &str) -> Option<CuratedAgentLine> {
    if let Some(payload) = line.strip_prefix(CHANGE_INTENT_PREFIX) {
        return Some(
            parse_change_intent(payload).unwrap_or_else(|| malformed_change_protocol(line)),
        );
    }
    if let Some(payload) = line.strip_prefix(CHANGE_RESULT_PREFIX) {
        return Some(
            parse_change_result(payload).unwrap_or_else(|| malformed_change_protocol(line)),
        );
    }
    None
}

fn parse_change_intent(payload: &str) -> Option<CuratedAgentLine> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let path = protocol_field(&value, "path")?;
    let reason = protocol_field(&value, "reason")?;
    let delta = protocol_field(&value, "delta")?;
    Some(CuratedAgentLine {
        stream: "change_intent".to_owned(),
        line: format!(
            "计划修改 {} | 原因: {} | 差异: {}",
            compact(path, 100),
            compact(reason, 220),
            compact(delta, 220)
        ),
    })
}

fn parse_change_result(payload: &str) -> Option<CuratedAgentLine> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let path = protocol_field(&value, "path")?;
    let summary = protocol_field(&value, "summary")?;
    let validation = protocol_field(&value, "validation")?;
    Some(CuratedAgentLine {
        stream: "change_result".to_owned(),
        line: format!(
            "修改完成 {} | 内容: {} | 验证: {}",
            compact(path, 100),
            compact(summary, 260),
            compact(validation, 180)
        ),
    })
}

fn protocol_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    let value = value.get(key)?.as_str()?.trim();
    (!value.is_empty()).then_some(value)
}

fn malformed_change_protocol(line: &str) -> CuratedAgentLine {
    CuratedAgentLine {
        stream: "stderr".to_owned(),
        line: format!(
            "变更说明格式无效；Agent 必须按 Harness JSON 协议重新报告: {}",
            compact(line, 320)
        ),
    }
}

fn normalize(value: &str) -> String {
    value.trim().replace('\t', " ")
}

fn is_error(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("error:")
        || lower.starts_with("fatal:")
        || lower.starts_with("failed:")
        || lower.contains("categoryinfo")
        || lower.contains("fullyqualifiederrorid")
        || lower.contains("runtimeexception")
        || lower.contains("methodnotfound")
        || lower.contains("panicked at")
        || lower.contains("command failed")
        || lower.contains("process didn't exit successfully")
        || lower.contains("process did not exit successfully")
        || lower.contains("need_scope_expansion")
}

fn is_progress(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "reading ",
        "read ",
        "inspecting ",
        "searching ",
        "found ",
        "editing ",
        "edited ",
        "writing ",
        "wrote ",
        "updating ",
        "updated ",
        "creating ",
        "created ",
        "modifying ",
        "modified ",
        "running ",
        "checking ",
        "executing ",
        "applying ",
        "applied ",
        "verifying ",
        "plan:",
        "report:",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn looks_like_source_dump(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();

    if line.chars().count() > 500 {
        return true;
    }

    [
        "use ",
        "pub use ",
        "fn ",
        "pub fn ",
        "let ",
        "const ",
        "struct ",
        "enum ",
        "impl ",
        "rsx!",
        "<div",
        "</div",
        "<span",
        "</span",
        "<button",
        "</button",
        "class=",
        "classname=",
        "onclick=",
        "onchange=",
        "import ",
        "export ",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix) || lower.contains(prefix))
        || matches!(trimmed, "{" | "}" | "}," | ");" | "/>" | "</>")
}

fn looks_mojibake(line: &str) -> bool {
    let suspicious = line
        .chars()
        .filter(|ch| {
            matches!(
                ch,
                'Ã' | 'Â' | 'â' | 'å' | 'æ' | 'ç' | 'é' | 'è' | 'ä' | 'ï' | 'ð' | '�'
            )
        })
        .count();
    suspicious >= 3
}

fn compact(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.to_owned()
    } else {
        format!("{}…", value.chars().take(limit).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_source_code_dump() {
        assert!(curate("stderr", r#"<div className=\"flex items-center gap-2\">"#).is_none());
        assert!(curate("stderr", "use dioxus::prelude::*;").is_none());
    }

    #[test]
    fn suppresses_mojibake() {
        assert!(curate("stderr", "ç›®å‰ Buyer é¡µé¢æ–‡æœ¬").is_none());
    }

    #[test]
    fn keeps_real_powershell_error() {
        let line = curate("stderr", "FullyQualifiedErrorId : MethodNotFound").unwrap();
        assert_eq!(line.stream, "stderr");
        assert!(line.line.contains("MethodNotFound"));
    }

    #[test]
    fn normal_stderr_progress_is_not_an_error() {
        let line = curate("stderr", "reading dashboard.rs").unwrap();
        assert_eq!(line.stream, "stdout");
        assert_eq!(line.line, "reading dashboard.rs");
    }

    #[test]
    fn generic_command_timing_noise_is_suppressed() {
        assert!(curate("stderr", "succeeded in 2395ms:").is_none());
    }

    #[test]
    fn curates_change_intent_with_engineering_reason() {
        let line = curate(
            "stderr",
            r#"HARNESS_CHANGE_INTENT {"path":"crates/client/src/critical_pages/dashboard.rs","reason":"match the source metric-card hierarchy","delta":"target cards are vertically stacked instead of four source-aligned slots"}"#,
        )
        .unwrap();
        assert_eq!(line.stream, "change_intent");
        assert!(line.line.contains("计划修改"));
        assert!(line.line.contains("dashboard.rs"));
        assert!(line.line.contains("原因:"));
        assert!(line.line.contains("差异:"));
    }

    #[test]
    fn curates_change_result_with_summary_and_validation() {
        let line = curate(
            "stdout",
            r#"HARNESS_CHANGE_RESULT {"path":"crates/client/src/product_ui.css","summary":"tighten metric-card gaps and align header spacing with the pinned source","validation":"compare Buyer Overview screenshot and run client-web-check"}"#,
        )
        .unwrap();
        assert_eq!(line.stream, "change_result");
        assert!(line.line.contains("修改完成"));
        assert!(line.line.contains("内容:"));
        assert!(line.line.contains("验证:"));
    }

    #[test]
    fn malformed_change_protocol_is_visible_as_an_error() {
        let line = curate("stderr", "HARNESS_CHANGE_INTENT not-json").unwrap();
        assert_eq!(line.stream, "stderr");
        assert!(line.line.contains("变更说明格式无效"));
    }

    #[test]
    fn ignores_unstructured_stderr_noise() {
        assert!(curate("stderr", "Today usage").is_none());
        assert!(curate("stderr", "DeepSeek V3 Standard Healthy").is_none());
    }
}
