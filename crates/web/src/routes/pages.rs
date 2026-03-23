//! Wiki page routes.
//!
//! `GET /`            → redirect to index.org (or 404 if absent)
//! `GET /*path`       → render a wiki page
//! `GET /edit/*path`  → show the edit form for a wiki page

use axum::{
  extract::{Path, State},
  http::StatusCode,
  response::{IntoResponse, Redirect, Response},
};
use org_wiki_lib::PageMeta;
use tower_sessions::Session;
use tracing::error;

use crate::{auth, web_base::AppState};

// ── index ─────────────────────────────────────────────────────────────────────

/// `GET /` — redirect to the wiki index page.
pub async fn index_handler() -> impl IntoResponse {
  Redirect::to("/index.org")
}

// ── page view ─────────────────────────────────────────────────────────────────

/// `GET /*path` — render a wiki page.
///
/// Serves the cached HTML fragment if available; otherwise renders via Pandoc
/// and populates the cache.  Page reads are unauthenticated.
pub async fn page_handler(
  State(state): State<AppState>,
  session: Session,
  Path(page_path): Path<String>,
) -> Response {
  let rel_path = crate::routes::api::normalize_page_path(&page_path);
  let page_key = rel_path.to_string_lossy().into_owned();

  // Try cache first.
  let cached = match state.cache.get(&page_key) {
    Ok(c) => c,
    Err(e) => {
      error!("Cache read error for {page_key}: {e}");
      None
    }
  };

  let fragment = if let Some(html) = cached {
    html
  } else {
    // Read org source and render via Pandoc.
    let repo = state.wiki_repo.clone();
    let pandoc_bin = state.pandoc_bin.clone();
    let cache = state.cache.clone();
    let key_for_cache = page_key.clone();
    let rel = rel_path.clone();

    let result = tokio::task::spawn_blocking(move || {
      let source = repo
        .read_page(&rel)
        .map_err(|e| (axum::http::StatusCode::NOT_FOUND, e.to_string()))?;
      let html =
        org_wiki_lib::export_to_html(&source, &pandoc_bin).map_err(|e| {
          (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;
      cache.set(&key_for_cache, &html).ok();
      Ok::<_, (axum::http::StatusCode, String)>(html)
    })
    .await;

    match result {
      Ok(Ok(html)) => html,
      Ok(Err((status, msg))) => {
        if status == StatusCode::NOT_FOUND {
          return not_found_response(&page_path).into_response();
        }
        error!("Render error for {page_key}: {msg}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
      }
      Err(e) => {
        error!("spawn_blocking panic: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
      }
    }
  };

  // Extract title for the <title> element.
  let source_for_meta = {
    let repo = state.wiki_repo.clone();
    let rel = rel_path.clone();
    tokio::task::spawn_blocking(move || repo.read_page(&rel).ok())
      .await
      .unwrap_or(None)
  };
  let meta = source_for_meta
    .as_deref()
    .map(PageMeta::parse)
    .unwrap_or_default();
  let file_stem = rel_path
    .file_stem()
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| page_path.clone());
  let page_title = meta.display_title(&file_stem).to_owned();

  let user = auth::current_user(&session).await;

  let mut ctx = tera::Context::new();
  ctx.insert("site_title", &state.site_title);
  ctx.insert("page_title", &page_title);
  ctx.insert("page_path", &page_key);
  ctx.insert("content", &fragment);
  if let Some(u) = &user {
    ctx.insert("user", u);
  }

  match state.tera.render("page.html", &ctx) {
    Ok(html) => {
      ([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], html)
        .into_response()
    }
    Err(e) => {
      error!("Template render error: {e}");
      StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
  }
}

// ── edit ──────────────────────────────────────────────────────────────────────

/// `GET /edit/*path` — show the edit form for a wiki page.
///
/// Requires an authenticated session (enforced by the `require_auth`
/// middleware on the router).
pub async fn edit_handler(
  State(state): State<AppState>,
  session: Session,
  Path(page_path): Path<String>,
) -> Response {
  let rel_path = crate::routes::api::normalize_page_path(&page_path);
  let page_key = rel_path.to_string_lossy().into_owned();

  let repo = state.wiki_repo.clone();
  let rel = rel_path.clone();
  let source = tokio::task::spawn_blocking(move || {
    // A missing page is fine — creates a new one on save.
    repo.read_page(&rel).unwrap_or_default()
  })
  .await
  .unwrap_or_default();

  let meta = PageMeta::parse(&source);
  let file_stem = rel_path
    .file_stem()
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| page_path.clone());
  let page_title = meta.display_title(&file_stem).to_owned();

  let user = auth::current_user(&session).await;

  let mut ctx = tera::Context::new();
  ctx.insert("site_title", &state.site_title);
  ctx.insert("page_title", &page_title);
  ctx.insert("page_path", &page_key);
  ctx.insert("content", &source);
  if let Some(u) = &user {
    ctx.insert("user", u);
  }

  match state.tera.render("edit.html", &ctx) {
    Ok(html) => {
      ([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], html)
        .into_response()
    }
    Err(e) => {
      error!("Template render error: {e}");
      StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
  }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn not_found_response(path: &str) -> impl IntoResponse {
  let body = format!(
    "<h1>404 Not Found</h1><p>No page at <code>{path}</code>.</p>\
     <p><a href=\"/edit/{path}\">Create it</a></p>"
  );
  (
    StatusCode::NOT_FOUND,
    [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
    body,
  )
}
