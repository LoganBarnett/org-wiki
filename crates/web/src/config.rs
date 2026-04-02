use clap::Parser;
use org_wiki_lib::{LogFormat, LogLevel};
use serde::Deserialize;
use std::path::PathBuf;
use thiserror::Error;
use tokio_listener::ListenerAddress;

#[derive(Debug, Error)]
pub enum ConfigError {
  #[error(
    "Failed to read configuration file at {path:?} during startup: {source}"
  )]
  FileRead {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to parse configuration file at {path:?}: {source}")]
  Parse {
    path: PathBuf,
    #[source]
    source: toml::de::Error,
  },

  #[error("Configuration validation failed: {0}")]
  Validation(String),

  #[error("Invalid listen address '{address}': {reason}")]
  InvalidListenAddress {
    address: String,
    reason: &'static str,
  },

  #[error("Failed to read secret file at {path:?}: {source}")]
  SecretFileRead {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
}

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct CliRaw {
  /// Log level (trace, debug, info, warn, error)
  #[arg(long, env = "LOG_LEVEL")]
  pub log_level: Option<String>,

  /// Log format (text, json)
  #[arg(long, env = "LOG_FORMAT")]
  pub log_format: Option<String>,

  /// Path to configuration file
  #[arg(short, long, env = "CONFIG_FILE")]
  pub config: Option<PathBuf>,

  /// Address to listen on: host:port for TCP, /path/to.sock for Unix socket,
  /// or sd-listen to inherit a socket from systemd
  #[arg(long, env = "LISTEN")]
  pub listen: Option<String>,

  /// Path to compiled frontend static assets
  #[arg(long, env = "FRONTEND_PATH")]
  pub frontend_path: Option<PathBuf>,

  // ── wiki ────────────────────────────────────────────────────────────────
  /// Path to the git repository holding the org-mode wiki content
  #[arg(long, env = "CONTENT_REPO")]
  pub content_repo: Option<PathBuf>,

  /// Git remote name to push to after each save (omit to disable push)
  #[arg(long, env = "CONTENT_REMOTE")]
  pub content_remote: Option<String>,

  /// Path to the Pandoc binary
  #[arg(long, env = "PANDOC_BIN")]
  pub pandoc_bin: Option<PathBuf>,

  /// Path to the org-fmt binary (omit to disable post-save formatting)
  #[arg(long, env = "ORG_FMT_BIN")]
  pub org_fmt_bin: Option<PathBuf>,

  /// Directory for cached HTML fragments (omit to disable caching)
  #[arg(long, env = "CACHE_DIR")]
  pub cache_dir: Option<PathBuf>,

  /// Human-readable site name shown in the HTML header
  #[arg(long, env = "SITE_TITLE")]
  pub site_title: Option<String>,

  // ── git commit identity ─────────────────────────────────────────────────
  /// Name used as the git Author on server-side commits
  #[arg(long, env = "COMMIT_AUTHOR_NAME")]
  pub commit_author_name: Option<String>,

  /// Email used as the git Author on server-side commits
  #[arg(long, env = "COMMIT_AUTHOR_EMAIL")]
  pub commit_author_email: Option<String>,

  // ── OIDC ────────────────────────────────────────────────────────────────
  /// OIDC provider issuer URL (must expose /.well-known/openid-configuration)
  #[arg(long, env = "OIDC_ISSUER")]
  pub oidc_issuer: Option<String>,

  /// OAuth2 client ID registered with the OIDC provider
  #[arg(long, env = "OIDC_CLIENT_ID")]
  pub oidc_client_id: Option<String>,

  /// OAuth2 client secret
  #[arg(long, env = "OIDC_CLIENT_SECRET")]
  pub oidc_client_secret: Option<String>,

  /// Path to a file containing the OAuth2 client secret
  #[arg(long, env = "OIDC_CLIENT_SECRET_FILE")]
  pub oidc_client_secret_file: Option<PathBuf>,

  /// Public base URL of this org-wiki instance (used to build the OIDC redirect URI)
  #[arg(long, env = "BASE_URL")]
  pub base_url: Option<String>,

  // ── webhook ─────────────────────────────────────────────────────────────
  /// Shared secret for HMAC-SHA256 verification of incoming webhooks.
  /// If omitted, the /webhook endpoint accepts all requests without verification.
  #[arg(long, env = "WEBHOOK_SECRET")]
  pub webhook_secret: Option<String>,

  /// Path to a file containing the webhook shared secret
  #[arg(long, env = "WEBHOOK_SECRET_FILE")]
  pub webhook_secret_file: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ConfigFileRaw {
  pub log_level: Option<String>,
  pub log_format: Option<String>,
  pub listen: Option<String>,
  pub frontend_path: Option<PathBuf>,
  // wiki
  pub content_repo: Option<PathBuf>,
  pub content_remote: Option<String>,
  pub pandoc_bin: Option<PathBuf>,
  pub org_fmt_bin: Option<PathBuf>,
  pub cache_dir: Option<PathBuf>,
  pub site_title: Option<String>,
  // git identity
  pub commit_author_name: Option<String>,
  pub commit_author_email: Option<String>,
  // OIDC
  pub oidc_issuer: Option<String>,
  pub oidc_client_id: Option<String>,
  pub oidc_client_secret: Option<String>,
  pub oidc_client_secret_file: Option<PathBuf>,
  pub base_url: Option<String>,
  // webhook
  pub webhook_secret: Option<String>,
  pub webhook_secret_file: Option<PathBuf>,
}

impl ConfigFileRaw {
  pub fn from_file(path: &PathBuf) -> Result<Self, ConfigError> {
    let contents = std::fs::read_to_string(path).map_err(|source| {
      ConfigError::FileRead {
        path: path.clone(),
        source,
      }
    })?;

    let config: ConfigFileRaw =
      toml::from_str(&contents).map_err(|source| ConfigError::Parse {
        path: path.clone(),
        source,
      })?;

    Ok(config)
  }
}

/// Validated, fully-resolved configuration.
#[derive(Debug, Clone)]
pub struct Config {
  pub log_level: LogLevel,
  pub log_format: LogFormat,
  pub listen_address: ListenerAddress,
  pub frontend_path: PathBuf,
  // wiki
  pub content_repo: PathBuf,
  pub content_remote: Option<String>,
  pub pandoc_bin: PathBuf,
  pub org_fmt_bin: Option<PathBuf>,
  pub cache_dir: Option<PathBuf>,
  pub site_title: String,
  // git identity
  pub commit_author_name: String,
  pub commit_author_email: String,
  // OIDC
  pub oidc_issuer: String,
  pub oidc_client_id: String,
  pub oidc_client_secret: String,
  pub base_url: String,
  // webhook
  pub webhook_secret: Option<String>,
}

impl Config {
  pub fn from_cli_and_file(cli: CliRaw) -> Result<Self, ConfigError> {
    let config_file = if let Some(config_path) = &cli.config {
      ConfigFileRaw::from_file(config_path)?
    } else {
      let default_config_path = PathBuf::from("config.toml");
      if default_config_path.exists() {
        ConfigFileRaw::from_file(&default_config_path)?
      } else {
        ConfigFileRaw::default()
      }
    };

    // ── logging ──────────────────────────────────────────────────────────

    let log_level_str = cli
      .log_level
      .or(config_file.log_level)
      .unwrap_or_else(|| "info".to_string());

    let log_level = log_level_str
      .parse::<LogLevel>()
      .map_err(|e| ConfigError::Validation(e.to_string()))?;

    let log_format_str = cli
      .log_format
      .or(config_file.log_format)
      .unwrap_or_else(|| "text".to_string());

    let log_format = log_format_str
      .parse::<LogFormat>()
      .map_err(|e| ConfigError::Validation(e.to_string()))?;

    // ── network ───────────────────────────────────────────────────────────

    let listen_str = cli
      .listen
      .or(config_file.listen)
      .unwrap_or_else(|| "127.0.0.1:3000".to_string());

    let listen_address =
      listen_str.parse::<ListenerAddress>().map_err(|reason| {
        ConfigError::InvalidListenAddress {
          address: listen_str.clone(),
          reason,
        }
      })?;

    let frontend_path = cli
      .frontend_path
      .or(config_file.frontend_path)
      .unwrap_or_else(|| PathBuf::from("frontend/public"));

    // ── wiki ─────────────────────────────────────────────────────────────

    let content_repo = cli
      .content_repo
      .or(config_file.content_repo)
      .ok_or_else(|| {
        ConfigError::Validation(
          "content_repo is required (path to the wiki git repository)"
            .to_owned(),
        )
      })?;

    let content_remote = cli.content_remote.or(config_file.content_remote);

    let pandoc_bin = cli
      .pandoc_bin
      .or(config_file.pandoc_bin)
      .unwrap_or_else(|| PathBuf::from("pandoc"));

    // Default to "org-fmt" on PATH so formatting is enabled out of the box.
    // Callers that want to disable formatting must set org_fmt_bin to None
    // explicitly; there is no config-file mechanism to clear a default.
    let org_fmt_bin = cli
      .org_fmt_bin
      .or(config_file.org_fmt_bin)
      .or_else(|| Some(PathBuf::from("org-fmt")));

    let cache_dir = cli.cache_dir.or(config_file.cache_dir);

    let site_title = cli
      .site_title
      .or(config_file.site_title)
      .unwrap_or_else(|| "Org Wiki".to_owned());

    // ── git identity ─────────────────────────────────────────────────────

    let commit_author_name = cli
      .commit_author_name
      .or(config_file.commit_author_name)
      .unwrap_or_else(|| "Org Wiki".to_owned());

    let commit_author_email = cli
      .commit_author_email
      .or(config_file.commit_author_email)
      .unwrap_or_else(|| "wiki@localhost".to_owned());

    // ── OIDC ─────────────────────────────────────────────────────────────

    let oidc_issuer =
      cli.oidc_issuer.or(config_file.oidc_issuer).ok_or_else(|| {
        ConfigError::Validation("oidc_issuer is required".to_owned())
      })?;

    let oidc_client_id = cli
      .oidc_client_id
      .or(config_file.oidc_client_id)
      .ok_or_else(|| {
        ConfigError::Validation("oidc_client_id is required".to_owned())
      })?;

    let secret_file_path = cli
      .oidc_client_secret_file
      .or(config_file.oidc_client_secret_file);

    let oidc_client_secret = cli
      .oidc_client_secret
      .or(config_file.oidc_client_secret)
      .map(Ok)
      .or_else(|| {
        secret_file_path.map(|path| {
          std::fs::read_to_string(&path)
            .map(|s| s.trim().to_owned())
            .map_err(|source| ConfigError::SecretFileRead { path, source })
        })
      })
      .transpose()?
      .ok_or_else(|| {
        ConfigError::Validation(
          "oidc_client_secret or oidc_client_secret_file is required"
            .to_owned(),
        )
      })?;

    let base_url = cli.base_url.or(config_file.base_url).ok_or_else(|| {
      ConfigError::Validation("base_url is required".to_owned())
    })?;

    // ── webhook ───────────────────────────────────────────────────────────

    let webhook_secret_file_path =
      cli.webhook_secret_file.or(config_file.webhook_secret_file);

    let webhook_secret = cli
      .webhook_secret
      .or(config_file.webhook_secret)
      .map(Ok)
      .or_else(|| {
        webhook_secret_file_path.map(|path| {
          std::fs::read_to_string(&path)
            .map(|s| s.trim().to_owned())
            .map_err(|source| ConfigError::SecretFileRead { path, source })
        })
      })
      .transpose()?;

    Ok(Config {
      log_level,
      log_format,
      listen_address,
      frontend_path,
      content_repo,
      content_remote,
      pandoc_bin,
      org_fmt_bin,
      cache_dir,
      site_title,
      commit_author_name,
      commit_author_email,
      oidc_issuer,
      oidc_client_id,
      oidc_client_secret,
      base_url,
      webhook_secret,
    })
  }
}
