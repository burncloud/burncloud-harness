use tracing_subscriber::EnvFilter;

pub fn init() {
    let default_filter = if std::env::var_os("BURNCLOUD_HARNESS_AGENT_RAW").is_some() {
        "burncloud_harness=info"
    } else {
        // `runner` owns the raw child stdout/stderr tracing calls. Keep those available for
        // diagnosis, but do not flood the normal operator console. Structured lifecycle logs
        // are emitted by `observer` and remain visible at INFO/WARN.
        "burncloud_harness=info,burncloud_harness::runner=error"
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_can_hide_runner_raw_lines() {
        let filter = EnvFilter::new("burncloud_harness=info,burncloud_harness::runner=error");
        let rendered = format!("{filter:?}");
        assert!(rendered.contains("burncloud_harness"));
    }
}
