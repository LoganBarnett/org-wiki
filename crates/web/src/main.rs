//! org-wiki-web — wiki web server
//!
//! Route layout:
//!   GET  /api/page/*path    → page title + rendered HTML + raw org (JSON)
//!   GET  /api/me            → authenticated user (JSON), 401 if not logged in
//!   POST /api/preview       → org → HTML fragment (requires auth)
//!   POST /api/save/*path    → commit + push       (requires auth)
//!   POST /webhook           → git push event      (HMAC-verified)
//!   GET  /auth/login        → initiate OIDC flow  (?next= supported)
//!   GET  /auth/callback     → OIDC callback
//!   GET  /auth/logout       → clear session
//!   GET  /healthz           → health check
//!   GET  /metrics           → Prometheus metrics
//!   *                       → Elm SPA (index.html fallback)

mod auth;
mod logging;
mod routes;
mod systemd;

use org_wiki_web::config::{CliRaw, Config, ConfigError};
use org_wiki_web::web_base::{self, AppState, AppStateError};

use axum::{
  extract::State,
  http::Request,
  middleware::{self, Next},
  routing::{get, post},
  Router,
};
use clap::Parser;
use logging::init_logging;
use thiserror::Error;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tower_sessions::{cookie::SameSite, MemoryStore, SessionManagerLayer};
use tracing::{error, info};

#[derive(Debug, Error)]
enum ApplicationError {
  #[error("Failed to load configuration during startup: {0}")]
  ConfigurationLoad(#[from] ConfigError),

  #[error("Failed to initialise application state: {0}")]
  StateInit(#[from] AppStateError),

  #[error("Failed to bind listener to {address}: {source}")]
  ListenerBind {
    address: String,
    source: std::io::Error,
  },

  #[error("Server encountered a runtime error: {0}")]
  ServerRuntime(#[source] std::io::Error),
}

#[tokio::main]
async fn main() -> Result<(), ApplicationError> {
  let cli = CliRaw::parse();

  let config = Config::from_cli_and_file(cli).map_err(|e| {
    eprintln!("Configuration error: {}", e);
    ApplicationError::ConfigurationLoad(e)
  })?;

  init_logging(config.log_level, config.log_format);

  info!("Starting org-wiki-web");

  let state = AppState::init(&config).await.map_err(|e| {
    error!("Failed to initialise app state: {e}");
    ApplicationError::StateInit(e)
  })?;

  info!("Binding to {}", config.listen_address);

  let app = create_app(state);

  let listener = tokio_listener::Listener::bind(
    &config.listen_address,
    &tokio_listener::SystemOptions::default(),
    &tokio_listener::UserOptions::default(),
  )
  .await
  .map_err(|source| {
    error!("Failed to bind to {}: {}", config.listen_address, source);
    ApplicationError::ListenerBind {
      address: config.listen_address.to_string(),
      source,
    }
  })?;

  info!("Server listening on {}", config.listen_address);

  systemd::notify_ready();
  systemd::spawn_watchdog();

  axum::serve(listener, app.into_make_service())
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|e| {
      error!("Server error: {}", e);
      ApplicationError::ServerRuntime(e)
    })?;

  info!("Shutting down org-wiki-web");
  Ok(())
}

fn create_app(state: AppState) -> Router {
  // Session middleware (in-memory store; swap for a persistent store later).
  let session_store = MemoryStore::default();
  // SameSite::Lax is required for OIDC: the provider redirects back via a
  // cross-site GET, and SameSite::Strict (the default) suppresses cookies on
  // cross-site navigations, causing the state/nonce lookup to fail.
  let session_layer = SessionManagerLayer::new(session_store)
    .with_secure(true)
    .with_same_site(SameSite::Lax);

  // Protected routes: inject state first so the router is Router<()>,
  // then wrap with the auth middleware (which itself is stateless).
  let protected = Router::new()
    .route("/api/preview", post(routes::api::preview_handler))
    .route("/api/save/{*path}", post(routes::api::save_handler))
    .with_state(state.clone())
    .layer(middleware::from_fn(auth::require_auth));

  // Public API routes (no auth required; /api/me returns 401 if unauthed).
  let api_routes = Router::new()
    .route("/api/page/{*path}", get(routes::api::page_handler))
    .route("/api/me", get(routes::api::me_handler))
    .with_state(state.clone());

  // Auth routes.
  let auth_routes = Router::new()
    .route("/auth/login", get(auth::login_handler))
    .route("/auth/callback", get(auth::callback_handler))
    .route("/auth/logout", get(auth::logout_handler))
    .with_state(state.clone());

  // Webhook (HMAC-verified internally).
  let webhook_route = Router::new()
    .route("/webhook", post(routes::webhook::webhook_handler))
    .with_state(state.clone());

  // All subrouters are now Router<()> — merge into base and apply outer layers.
  // The ServeDir fallback in base_router serves index.html for all other paths,
  // handing them to the Elm SPA.
  Router::new()
    .merge(web_base::base_router(state.clone()))
    .merge(protected)
    .merge(api_routes)
    .merge(auth_routes)
    .merge(webhook_route)
    .layer(session_layer)
    .layer(TraceLayer::new_for_http())
    .layer(middleware::from_fn_with_state(state, count_requests))
}

async fn count_requests(
  State(state): State<AppState>,
  req: Request<axum::body::Body>,
  next: Next,
) -> axum::response::Response {
  state.request_counter.inc();
  next.run(req).await
}

async fn shutdown_signal() {
  let ctrl_c = async {
    signal::ctrl_c()
      .await
      .expect("failed to install Ctrl+C handler");
  };

  #[cfg(unix)]
  let terminate = async {
    signal::unix::signal(signal::unix::SignalKind::terminate())
      .expect("failed to install signal handler")
      .recv()
      .await;
  };

  #[cfg(not(unix))]
  let terminate = std::future::pending::<()>();

  tokio::select! {
    _ = ctrl_c => {
      info!("Received Ctrl+C, shutting down gracefully");
    },
    _ = terminate => {
      info!("Received SIGTERM, shutting down gracefully");
    },
  }
}
