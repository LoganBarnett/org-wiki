use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
  Trace,
  Debug,
  Info,
  Warn,
  Error,
}

impl FromStr for LogLevel {
  type Err = LogLevelParseError;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s.to_lowercase().as_str() {
      "trace" => Ok(LogLevel::Trace),
      "debug" => Ok(LogLevel::Debug),
      "info" => Ok(LogLevel::Info),
      "warn" | "warning" => Ok(LogLevel::Warn),
      "error" => Ok(LogLevel::Error),
      _ => Err(LogLevelParseError::InvalidLevel(s.to_string())),
    }
  }
}

impl From<LogLevel> for tracing::Level {
  fn from(level: LogLevel) -> Self {
    match level {
      LogLevel::Trace => tracing::Level::TRACE,
      LogLevel::Debug => tracing::Level::DEBUG,
      LogLevel::Info => tracing::Level::INFO,
      LogLevel::Warn => tracing::Level::WARN,
      LogLevel::Error => tracing::Level::ERROR,
    }
  }
}

impl std::fmt::Display for LogLevel {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      LogLevel::Trace => write!(f, "trace"),
      LogLevel::Debug => write!(f, "debug"),
      LogLevel::Info => write!(f, "info"),
      LogLevel::Warn => write!(f, "warn"),
      LogLevel::Error => write!(f, "error"),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
  Text,
  Json,
}

impl FromStr for LogFormat {
  type Err = LogFormatParseError;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s.to_lowercase().as_str() {
      "text" | "pretty" => Ok(LogFormat::Text),
      "json" => Ok(LogFormat::Json),
      _ => Err(LogFormatParseError::InvalidFormat(s.to_string())),
    }
  }
}

impl std::fmt::Display for LogFormat {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      LogFormat::Text => write!(f, "text"),
      LogFormat::Json => write!(f, "json"),
    }
  }
}

#[derive(Debug, Error)]
pub enum LogLevelParseError {
  #[error(
    "Invalid log level: {0}. Valid values are: trace, debug, info, warn, error"
  )]
  InvalidLevel(String),
}

#[derive(Debug, Error)]
pub enum LogFormatParseError {
  #[error("Invalid log format: {0}. Valid values are: text, json")]
  InvalidFormat(String),
}
