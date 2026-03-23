use std::{path::PathBuf, sync::Arc};

use aide::{
  axum::{routing::get_with, ApiRouter},
  openapi::OpenApi,
  scalar::Scalar,
  transform::TransformOperation,
};
use axum::{
  http::{header, HeaderValue, StatusCode},
  response::{IntoResponse, Response},
  routing::get,
  Json, Router,
};
use openidconnect::core::CoreClient;
use prometheus::{Encoder, IntCounter, Registry, TextEncoder};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use tera::Tera;
use thiserror::Error;
use tower::ServiceBuilder;
use tower_http::{
  services::{ServeDir, ServeFile},
  set_header::SetResponseHeaderLayer,
};
use tracing::info;

use crate::config::Config;
use org_wiki_lib::{Cache, WikiRepo};

// ── AppState ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
  // Base infrastructure (from template)
  pub registry: Arc<Registry>,
  pub request_counter: IntCounter,
  pub frontend_path: PathBuf,
  // Wiki
  pub wiki_repo: WikiRepo,
  pub cache: Cache,
  pub pandoc_bin: PathBuf,
  pub site_title: String,
  pub commit_author_name: String,
  pub commit_author_email: String,
  // Templates
  pub tera: Arc<Tera>,
  // Auth
  pub oidc_client: Arc<CoreClient>,
  // Webhook
  pub webhook_secret: Option<String>,
}

#[derive(Debug, Error)]
pub enum AppStateError {
  #[error("Failed to open wiki repository: {0}")]
  WikiRepo(#[from] org_wiki_lib::GitError),

  #[error("Failed to load Tera templates from {path:?}: {source}")]
  Templates {
    path: PathBuf,
    #[source]
    source: tera::Error,
  },

  #[error("Invalid OIDC issuer URL: {0}")]
  InvalidIssuer(String),

  #[error("OIDC provider discovery failed: {0}")]
  OidcDiscovery(String),

  #[error("Invalid OIDC redirect URI: {0}")]
  InvalidRedirectUri(String),
}

impl AppState {
  /// Construct `AppState` from a validated `Config`.
  ///
  /// Performs OIDC discovery (an async HTTP call) and opens the wiki repo.
  pub async fn init(config: &Config) -> Result<Self, AppStateError> {
    // ── metrics ───────────────────────────────────────────────────────────
    let registry = Registry::new();
    let request_counter =
      IntCounter::new("http_requests_total", "Total HTTP requests")
        .expect("Failed to create counter");
    registry
      .register(Box::new(request_counter.clone()))
      .expect("Failed to register counter");

    // ── wiki repo ─────────────────────────────────────────────────────────
    let wiki_repo =
      WikiRepo::open(&config.content_repo, config.content_remote.clone())?;
    info!(path = ?config.content_repo, "opened wiki repository");

    let cache = match &config.cache_dir {
      Some(dir) => Cache::new(dir.clone()),
      None => Cache::disabled(),
    };

    // ── Tera templates ────────────────────────────────────────────────────
    let template_glob = format!("{}/**/*.html", config.template_dir.display());
    let tera =
      Tera::new(&template_glob).map_err(|source| AppStateError::Templates {
        path: config.template_dir.clone(),
        source,
      })?;
    info!(glob = %template_glob, "loaded Tera templates");

    // ── OIDC client ───────────────────────────────────────────────────────
    let issuer = openidconnect::IssuerUrl::new(config.oidc_issuer.clone())
      .map_err(|e| AppStateError::InvalidIssuer(e.to_string()))?;

    let provider_metadata =
      openidconnect::core::CoreProviderMetadata::discover_async(
        issuer,
        openidconnect::reqwest::async_http_client,
      )
      .await
      .map_err(|e| AppStateError::OidcDiscovery(e.to_string()))?;

    info!(issuer = %config.oidc_issuer, "OIDC discovery complete");

    let redirect_url = openidconnect::RedirectUrl::new(format!(
      "{}/auth/callback",
      config.base_url.trim_end_matches('/')
    ))
    .map_err(|e| AppStateError::InvalidRedirectUri(e.to_string()))?;

    let oidc_client = openidconnect::core::CoreClient::from_provider_metadata(
      provider_metadata,
      openidconnect::ClientId::new(config.oidc_client_id.clone()),
      Some(openidconnect::ClientSecret::new(config.oidc_client_secret.clone())),
    )
    .set_redirect_uri(redirect_url);

    Ok(Self {
      registry: Arc::new(registry),
      request_counter,
      frontend_path: config.frontend_path.clone(),
      wiki_repo,
      cache,
      pandoc_bin: config.pandoc_bin.clone(),
      site_title: config.site_title.clone(),
      commit_author_name: config.commit_author_name.clone(),
      commit_author_email: config.commit_author_email.clone(),
      tera: Arc::new(tera),
      oidc_client: Arc::new(oidc_client),
      webhook_secret: config.webhook_secret.clone(),
    })
  }

  /// Construct a minimal `AppState` for integration tests that only exercise
  /// base routes (healthz, metrics, OpenAPI, SPA fallback).
  ///
  /// Creates stub values for wiki/auth fields; none of those fields are
  /// accessed by `base_router`.  The temporary git repository is leaked
  /// (converted to a bare `PathBuf`) which is acceptable in tests.
  pub fn new_for_test(frontend_path: PathBuf) -> Self {
    use openidconnect::{
      core::CoreJsonWebKeySet, AuthUrl, ClientId, IssuerUrl,
    };
    use prometheus::{IntCounter, Registry};
    use tera::Tera;

    let registry = Registry::new();
    let request_counter =
      IntCounter::new("http_requests_total", "Total HTTP requests").unwrap();
    registry
      .register(Box::new(request_counter.clone()))
      .unwrap();

    // Initialise a bare-minimum git repo so WikiRepo::open succeeds.
    // The dir is intentionally leaked (into_path) — acceptable in tests.
    let repo_dir = tempfile::TempDir::new().unwrap().keep();
    git2::Repository::init(&repo_dir).unwrap();
    let wiki_repo = org_wiki_lib::WikiRepo::open(&repo_dir, None).unwrap();

    let oidc_client = openidconnect::core::CoreClient::new(
      ClientId::new("test".to_string()),
      None,
      IssuerUrl::new("https://example.com".to_string()).unwrap(),
      AuthUrl::new("https://example.com/auth".to_string()).unwrap(),
      None,
      None,
      CoreJsonWebKeySet::new(vec![]),
    );

    Self {
      registry: Arc::new(registry),
      request_counter,
      frontend_path,
      wiki_repo,
      cache: org_wiki_lib::Cache::disabled(),
      pandoc_bin: PathBuf::from("pandoc"),
      site_title: "Test Wiki".to_string(),
      commit_author_name: "Test".to_string(),
      commit_author_email: "test@example.com".to_string(),
      tera: Arc::new(Tera::default()),
      oidc_client: Arc::new(oidc_client),
      webhook_secret: None,
    }
  }
}

// ── base router ───────────────────────────────────────────────────────────────

#[derive(Serialize, JsonSchema)]
pub struct HealthResponse {
  status: String,
}

async fn healthz() -> Json<HealthResponse> {
  Json(HealthResponse {
    status: "healthy".to_string(),
  })
}

pub fn base_router(state: AppState) -> Router {
  aide::generate::extract_schemas(true);
  let frontend_path = state.frontend_path.clone();
  let mut api = OpenApi::default();

  let app_router = ApiRouter::new()
    .api_route(
      "/healthz",
      get_with(healthz, |op: TransformOperation| {
        op.description("Health check.")
      }),
    )
    .api_route(
      "/metrics",
      get_with(metrics_endpoint, |op: TransformOperation| {
        op.description("Prometheus metrics in text/plain format.")
      }),
    )
    .with_state(state)
    .finish_api_with(&mut api, |a| a.title("org-wiki"));

  let api = Arc::new(api);

  Router::new()
    .merge(app_router)
    .route(
      "/api-docs/openapi.json",
      get({
        let api = api.clone();
        move || async move { Json((*api).clone()) }
      }),
    )
    .route(
      "/scalar",
      get(
        Scalar::new("/api-docs/openapi.json")
          .with_title("org-wiki")
          .axum_handler(),
      ),
    )
    .fallback_service(
      ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
          header::CACHE_CONTROL,
          HeaderValue::from_static("no-store"),
        ))
        .service(
          ServeDir::new(&frontend_path)
            .fallback(ServeFile::new(frontend_path.join("index.html"))),
        ),
    )
}

async fn metrics_endpoint(
  axum::extract::State(state): axum::extract::State<AppState>,
) -> Response {
  let encoder = TextEncoder::new();
  let metric_families = state.registry.gather();
  let mut buffer = Vec::new();

  match encoder.encode(&metric_families, &mut buffer) {
    Ok(_) => {
      (StatusCode::OK, [("content-type", encoder.format_type())], buffer)
        .into_response()
    }
    Err(e) => (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(json!({
          "error": format!("Failed to encode metrics: {}", e)
      })),
    )
      .into_response(),
  }
}
