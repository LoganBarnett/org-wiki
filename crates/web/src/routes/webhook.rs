//! Incoming webhook handler.
//!
//! `POST /webhook` — receive a push event from the git forge.
//!
//! On receipt the server:
//!   1. Verifies the HMAC-SHA256 signature (if `webhook_secret` is configured).
//!   2. Extracts the list of modified `.org` files from the push payload.
//!   3. Runs `git pull --ff-only` on the content repo.
//!   4. Invalidates the HTML cache for all affected pages.

use axum::{
  body::Bytes,
  extract::State,
  http::{HeaderMap, StatusCode},
  response::IntoResponse,
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use tracing::{error, info, warn};

use crate::web_base::AppState;

type HmacSha256 = Hmac<Sha256>;

// ── push payload (Gitea / GitHub compatible) ──────────────────────────────────

#[derive(Debug, Deserialize)]
struct PushPayload {
  #[serde(default)]
  commits: Vec<Commit>,
}

#[derive(Debug, Deserialize)]
struct Commit {
  #[serde(default)]
  added: Vec<String>,
  #[serde(default)]
  modified: Vec<String>,
  #[serde(default)]
  removed: Vec<String>,
}

// ── handler ───────────────────────────────────────────────────────────────────

/// `POST /webhook` — process a git push event.
pub async fn webhook_handler(
  State(state): State<AppState>,
  headers: HeaderMap,
  body: Bytes,
) -> impl IntoResponse {
  // 1. Verify HMAC signature if a secret is configured.
  if let Some(secret) = &state.webhook_secret {
    let sig_header = headers
      .get("x-hub-signature-256")
      .or_else(|| headers.get("x-gitea-signature"))
      .and_then(|v| v.to_str().ok())
      .unwrap_or("");

    if !verify_signature(&body, sig_header, secret) {
      warn!("Webhook rejected: invalid or missing HMAC signature");
      return StatusCode::UNAUTHORIZED.into_response();
    }
  }

  // 2. Parse push payload.
  let payload: PushPayload = match serde_json::from_slice(&body) {
    Ok(p) => p,
    Err(e) => {
      warn!("Webhook: failed to parse push payload: {e}");
      return StatusCode::BAD_REQUEST.into_response();
    }
  };

  // 3. Collect all affected .org file paths.
  let affected: Vec<String> = payload
    .commits
    .iter()
    .flat_map(|c| c.added.iter().chain(&c.modified).chain(&c.removed).cloned())
    .filter(|p| p.ends_with(".org"))
    .collect();

  info!(count = affected.len(), "webhook: affected org files");

  // 4. Pull and invalidate cache (blocking operations).
  let repo = state.wiki_repo.clone();
  let cache = state.cache.clone();

  let result = tokio::task::spawn_blocking(move || {
    repo.pull()?;
    for path in &affected {
      cache.invalidate(path);
    }
    Ok::<_, org_wiki_lib::GitError>(())
  })
  .await;

  match result {
    Ok(Ok(())) => StatusCode::OK.into_response(),
    Ok(Err(e)) => {
      error!("Webhook pull/invalidate failed: {e}");
      StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
    Err(e) => {
      error!("spawn_blocking panic in webhook handler: {e}");
      StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
  }
}

// ── HMAC verification ─────────────────────────────────────────────────────────

fn verify_signature(body: &[u8], signature_header: &str, secret: &str) -> bool {
  let hex_digest = match signature_header.strip_prefix("sha256=") {
    Some(h) => h,
    None => {
      warn!("Webhook signature header missing 'sha256=' prefix");
      return false;
    }
  };

  let expected = match hex::decode(hex_digest) {
    Ok(b) => b,
    Err(_) => {
      warn!("Webhook signature is not valid hex");
      return false;
    }
  };

  let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
    .expect("HMAC accepts any key size");
  mac.update(body);
  mac.verify_slice(&expected).is_ok()
}
