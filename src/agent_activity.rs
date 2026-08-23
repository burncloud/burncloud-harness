#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratedAgentLine {
    pub stream: String,
    pub line: String,
}

pub fn curate(stream: &str, line: &str) -> Option<CuratedAgentLine> {
    let line = normalize(line);
    if line.is_empty() {
        return None;
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
        "succeeded in ",
        "failed in ",
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
        let line = curate("stderr", "succeeded in 2395ms:").unwrap();
        assert_eq!(line.stream, "stdout");
        assert_eq!(line.line, "succeeded in 2395ms:");
    }

    #[test]
    fn ignores_unstructured_stderr_noise() {
        assert!(curate("stderr", "Today usage").is_none());
        assert!(curate("stderr", "DeepSeek V3 Standard Healthy").is_none());
    }
}
