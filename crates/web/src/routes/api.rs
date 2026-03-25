//! JSON API endpoints.
//!
//! `GET  /api/page/*path`  — fetch a wiki page (title, rendered HTML, raw source).
//! `GET  /api/me`          — return the authenticated user, or 401.
//! `POST /api/preview`     — render org-mode text to an HTML fragment.
//! `POST /api/save/*path`  — write, commit, and optionally push a page.

use axum::{
  extract::{Path, State},
  http::StatusCode,
  response::{IntoResponse, Response},
  Json,
};
use org_wiki_lib::{CommitAuthor, CommitMessage, PageMeta};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::error;

use crate::{auth, web_base::AppState};

// ── page ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageResponse {
  pub title: String,
  pub html: String,
  pub raw_org: String,
  pub page_path: String,
  pub exists: bool,
}

/// `GET /api/page/*path` — fetch a wiki page as JSON.
///
/// Returns the rendered HTML, raw org source, and metadata for a page.
/// `exists: false` is returned (with empty fields) when the page is not found,
/// so the client can show a "create this page" prompt without a 404.
pub async fn page_handler(
  State(state): State<AppState>,
  Path(page_path): Path<String>,
) -> Response {
  let rel_path = normalize_page_path(&page_path);
  let page_key = rel_path.to_string_lossy().into_owned();

  let repo = state.wiki_repo.clone();
  let pandoc_bin = state.pandoc_bin.clone();
  let cache = state.cache.clone();

  let result = tokio::task::spawn_blocking(move || {
    let source = match repo.read_page(&rel_path) {
      Ok(s) => s,
      Err(_) => return Ok::<Option<_>, String>(None),
    };

    let meta = PageMeta::parse(&source);
    let file_stem = rel_path
      .file_stem()
      .map(|s| s.to_string_lossy().into_owned())
      .unwrap_or_else(|| page_key.clone());
    let title = meta.display_title(&file_stem).to_owned();

    // Use cached HTML if available; otherwise render and cache.
    let html = match cache.get(&page_key) {
      Ok(Some(cached)) => cached,
      _ => {
        let rendered = org_wiki_lib::export_to_html(&source, &pandoc_bin)
          .map_err(|e| e.to_string())?;
        cache.set(&page_key, &rendered).ok();
        rendered
      }
    };

    Ok(Some((title, html, source, page_key)))
  })
  .await;

  match result {
    Ok(Ok(Some((title, html, raw_org, page_path)))) => Json(PageResponse {
      title,
      html,
      raw_org,
      page_path,
      exists: true,
    })
    .into_response(),
    Ok(Ok(None)) => {
      // Page not found — return a placeholder so the client can offer
      // to create it rather than treating it as an error.
      Json(PageResponse {
        title: String::new(),
        html: String::new(),
        raw_org: String::new(),
        page_path: normalize_page_path(&page_path)
          .to_string_lossy()
          .into_owned(),
        exists: false,
      })
      .into_response()
    }
    Ok(Err(e)) => {
      error!("Page render failed for {page_path}: {e}");
      StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
    Err(e) => {
      error!("spawn_blocking panicked: {e}");
      StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
  }
}

// ── me ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct MeResponse {
  pub name: String,
  pub email: String,
}

/// `GET /api/me` — return the authenticated user's name and email.
///
/// Returns 401 if the session contains no authenticated user.
pub async fn me_handler(session: Session) -> Response {
  match auth::current_user(&session).await {
    Some(user) => Json(MeResponse {
      name: user.name,
      email: user.email,
    })
    .into_response(),
    None => StatusCode::UNAUTHORIZED.into_response(),
  }
}

// ── preview ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PreviewRequest {
  pub content: String,
}

/// `POST /api/preview` — render org-mode source to an HTML fragment.
///
/// Returns `text/html`.  The caller is responsible for embedding the fragment
/// inside a safe container (the editor preview pane).
pub async fn preview_handler(
  State(state): State<AppState>,
  Json(body): Json<PreviewRequest>,
) -> Response {
  let pandoc_bin = state.pandoc_bin.clone();
  let content = body.content.clone();

  let html = tokio::task::spawn_blocking(move || {
    org_wiki_lib::export_to_html(&content, &pandoc_bin)
  })
  .await;

  match html {
    Ok(Ok(fragment)) => (
      [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
      fragment,
    )
      .into_response(),
    Ok(Err(e)) => {
      error!("Pandoc export failed: {e}");
      (StatusCode::UNPROCESSABLE_ENTITY, format!("Export failed: {e}"))
        .into_response()
    }
    Err(e) => {
      error!("spawn_blocking panicked: {e}");
      StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
  }
}

// ── save ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SaveRequest {
  pub content: String,
  pub subject: String,
  pub body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SaveResponse {
  pub commit: String,
}

/// `POST /api/save/*path` — write the page, commit, invalidate cache, push.
///
/// Requires an authenticated session (enforced by the `require_auth`
/// middleware on the router).  The authenticated user is recorded as
/// `Co-authored-by:` in the commit message.
pub async fn save_handler(
  State(state): State<AppState>,
  session: Session,
  Path(page_path): Path<String>,
  Json(body): Json<SaveRequest>,
) -> Response {
  // Resolve the user for co-authorship — middleware guarantees presence.
  let user = auth::current_user(&session).await;
  let co_author = user.map(|u| CommitAuthor {
    name: u.name,
    email: u.email,
  });

  let rel_path = normalize_page_path(&page_path);
  let page_key = rel_path.to_string_lossy().into_owned();

  let repo = state.wiki_repo.clone();
  let cache = state.cache.clone();
  let message = CommitMessage {
    subject: body.subject.clone(),
    body: body.body.clone(),
  };
  let author_name = state.commit_author_name.clone();
  let author_email = state.commit_author_email.clone();
  let content = body.content.clone();

  let result = tokio::task::spawn_blocking(move || {
    let oid = repo.write_and_commit(
      &rel_path,
      &content,
      &message,
      &author_name,
      &author_email,
      co_author.as_ref(),
    )?;

    // Invalidate the HTML cache for this page so the next read re-renders.
    cache.invalidate(&page_key);

    // Push to remote (no-op if content_remote is None).
    repo.push()?;

    Ok::<_, org_wiki_lib::GitError>(oid.to_string())
  })
  .await;

  match result {
    Ok(Ok(commit_id)) => {
      Json(SaveResponse { commit: commit_id }).into_response()
    }
    Ok(Err(e)) => {
      error!("Save failed for {page_path}: {e}");
      (StatusCode::INTERNAL_SERVER_ERROR, format!("Save failed: {e}"))
        .into_response()
    }
    Err(e) => {
      error!("spawn_blocking panicked during save: {e}");
      StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
  }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Normalise a URL path segment into a relative `PathBuf`.
///
/// Ensures the path ends in `.org` and does not escape the repo root.
pub fn normalize_page_path(raw: &str) -> std::path::PathBuf {
  let trimmed = raw.trim_start_matches('/');

  // Append .org if the caller omitted it.
  let with_ext = if trimmed.ends_with(".org") {
    trimmed.to_owned()
  } else {
    format!("{trimmed}.org")
  };

  // Strip any path traversal components.
  let safe: std::path::PathBuf = with_ext
    .split('/')
    .filter(|c| !c.is_empty() && *c != ".." && *c != ".")
    .collect();

  safe
}
