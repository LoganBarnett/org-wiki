use axum::{
  body::Body,
  http::{Request, StatusCode},
};
use org_wiki_web::web_base::{base_router, AppState};
use std::path::PathBuf;
use tower::ServiceExt;

// Tests that don't exercise the SPA fallback use a non-existent path since
// the registered API routes never touch the filesystem.
fn state_without_frontend() -> AppState {
  AppState::new_for_test(PathBuf::from("/nonexistent"))
}

#[tokio::test]
async fn test_healthz_endpoint() {
  let app = base_router(state_without_frontend());

  let response = app
    .oneshot(
      Request::builder()
        .uri("/healthz")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::OK);

  let body = axum::body::to_bytes(response.into_body(), usize::MAX)
    .await
    .unwrap();
  let body_str = String::from_utf8(body.to_vec()).unwrap();

  assert!(body_str.contains("healthy"));
}

#[tokio::test]
async fn test_metrics_endpoint() {
  let app = base_router(state_without_frontend());

  let response = app
    .oneshot(
      Request::builder()
        .uri("/metrics")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::OK);

  let body = axum::body::to_bytes(response.into_body(), usize::MAX)
    .await
    .unwrap();
  let body_str = String::from_utf8(body.to_vec()).unwrap();

  assert!(
    body_str.contains("http_requests_total"),
    "Metrics should contain http_requests_total counter"
  );
}

#[tokio::test]
async fn test_openapi_json_endpoint() {
  let app = base_router(state_without_frontend());

  let response = app
    .oneshot(
      Request::builder()
        .uri("/api-docs/openapi.json")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::OK);

  let body = axum::body::to_bytes(response.into_body(), usize::MAX)
    .await
    .unwrap();
  let body_str = String::from_utf8(body.to_vec()).unwrap();

  assert!(body_str.contains("openapi"), "Response should be an OpenAPI spec");
  assert!(body_str.contains("/healthz"), "Spec should document /healthz");
  assert!(body_str.contains("/metrics"), "Spec should document /metrics");
}

#[tokio::test]
async fn test_scalar_ui_endpoint() {
  let app = base_router(state_without_frontend());

  let response = app
    .oneshot(
      Request::builder()
        .uri("/scalar")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::OK);

  let body = axum::body::to_bytes(response.into_body(), usize::MAX)
    .await
    .unwrap();

  assert!(
    body.starts_with(b"<!doctype html>")
      || body.starts_with(b"<!DOCTYPE html>"),
    "Scalar endpoint should return HTML"
  );
}

#[tokio::test]
async fn test_spa_fallback_serves_index_html() {
  // Any path not matched by a registered route must return 200 with the SPA
  // index.html, not 404.  This covers direct navigation and page refresh at
  // client-side routes like /dashboard or /settings/profile.
  let frontend_dir = tempfile::tempdir().unwrap();
  std::fs::write(
    frontend_dir.path().join("index.html"),
    b"<!doctype html><title>org-wiki</title>",
  )
  .unwrap();

  let app =
    base_router(AppState::new_for_test(frontend_dir.path().to_path_buf()));

  for path in ["/some-page", "/nested/route", "/unknown"] {
    let response = app
      .clone()
      .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
      .await
      .unwrap();
    assert_eq!(
      response.status(),
      StatusCode::OK,
      "expected 200 for SPA path {path}"
    );
  }
}
