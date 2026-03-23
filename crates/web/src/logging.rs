use org_wiki_lib::{LogFormat, LogLevel};
use tracing_subscriber::{
  fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
};

// Web services write program output to stdout and all log output to stderr so
// that the two streams can be separated by process supervisors and log
// aggregators.
pub fn init_logging(level: LogLevel, format: LogFormat) {
  let env_filter = EnvFilter::try_from_default_env()
    .unwrap_or_else(|_| EnvFilter::new(level.to_string()));

  // On Unix, attempt to route structured log events directly to the systemd
  // journal.  Journald carries richer structured metadata (severity, target,
  // span fields) and avoids double-ingestion when the process runs under a
  // systemd unit.  Falls back to stderr below when the journal socket is
  // absent (macOS, Docker without socket mount, etc.).
  #[cfg(unix)]
  if let Ok(journald) = tracing_journald::layer() {
    tracing_subscriber::registry()
      .with(journald.with_filter(env_filter))
      .init();
    return;
  }

  match format {
    LogFormat::Text => {
      tracing_subscriber::registry()
        .with(
          fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_line_number(true)
            .with_filter(env_filter),
        )
        .init();
    }
    LogFormat::Json => {
      tracing_subscriber::registry()
        .with(
          fmt::layer()
            .json()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_line_number(true)
            .with_filter(env_filter),
        )
        .init();
    }
  }
}
