//! OIDC authentication — login, callback, logout, and session guard.
//!
//! Flow:
//!   1. `GET /auth/login`    → store csrf+nonce in session, redirect to provider
//!   2. `GET /auth/callback` → verify state, exchange code, store user in session
//!   3. `GET /auth/logout`   → clear session, redirect to /
//!
//! `require_auth` is an Axum middleware function that enforces an authenticated
//! session on protected routes.

use axum::{
  extract::{Query, Request, State},
  http::StatusCode,
  middleware::Next,
  response::{IntoResponse, Redirect, Response},
};
use openidconnect::{
  core::{CoreAuthenticationFlow, CoreUserInfoClaims},
  AuthorizationCode, CsrfToken, Nonce, OAuth2TokenResponse, Scope,
  TokenResponse,
};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::{info, warn};

use crate::web_base::AppState;

// ── session keys ─────────────────────────────────────────────────────────────

const KEY_USER: &str = "user";
const KEY_OIDC_STATE: &str = "oidc_state";
const KEY_OIDC_NONCE: &str = "oidc_nonce";
/// Destination the user was trying to reach before being redirected to login.
const KEY_RETURN_TO: &str = "return_to";

// ── types ─────────────────────────────────────────────────────────────────────

/// Authenticated user stored in the session.
///
/// Populated from the OIDC ID token `name` and `email` claims.
/// Also used as the `CommitAuthor` for wiki edits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
  pub name: String,
  pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
  code: String,
  state: String,
}

// ── handlers ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
  /// Relative path to return to after successful authentication.
  next: Option<String>,
}

/// `GET /auth/login` — redirect the user to the OIDC provider.
///
/// Accepts an optional `?next=<path>` query parameter.  When present and a
/// valid relative path, it is stored in the session as `return_to` so the
/// callback handler can redirect back after authentication.
pub async fn login_handler(
  State(state): State<AppState>,
  session: Session,
  Query(params): Query<LoginQuery>,
) -> Response {
  // Store client-supplied return destination.  Validate as a relative path
  // to prevent open-redirect attacks.
  if let Some(next) = params
    .next
    .filter(|n| n.starts_with('/') && !n.starts_with("//"))
  {
    session.insert(KEY_RETURN_TO, next).await.ok();
  }
  let (auth_url, csrf_token, nonce) = state
    .oidc_client
    .authorize_url(
      CoreAuthenticationFlow::AuthorizationCode,
      CsrfToken::new_random,
      Nonce::new_random,
    )
    .add_scope(Scope::new("email".to_string()))
    .add_scope(Scope::new("profile".to_string()))
    .url();

  if let Err(e) = session
    .insert(KEY_OIDC_STATE, csrf_token.secret().clone())
    .await
  {
    warn!("Failed to write OIDC state to session: {e}");
    return (StatusCode::INTERNAL_SERVER_ERROR, "Session error")
      .into_response();
  }
  if let Err(e) = session.insert(KEY_OIDC_NONCE, nonce.secret().clone()).await {
    warn!("Failed to write OIDC nonce to session: {e}");
    return (StatusCode::INTERNAL_SERVER_ERROR, "Session error")
      .into_response();
  }

  Redirect::to(auth_url.as_str()).into_response()
}

/// `GET /auth/callback` — receive the authorization code from the OIDC provider.
pub async fn callback_handler(
  State(state): State<AppState>,
  session: Session,
  Query(params): Query<CallbackQuery>,
) -> Response {
  // 1. Verify CSRF state.
  let stored_state: String = match session.get(KEY_OIDC_STATE).await {
    Ok(Some(s)) => s,
    _ => {
      warn!("OIDC callback: missing state in session");
      return (
        StatusCode::BAD_REQUEST,
        "Invalid session — please try signing in again.",
      )
        .into_response();
    }
  };

  if params.state != stored_state {
    warn!("OIDC callback: state mismatch");
    return (
      StatusCode::BAD_REQUEST,
      "State mismatch — possible CSRF attempt.",
    )
      .into_response();
  }

  let nonce_secret: String = match session.get(KEY_OIDC_NONCE).await {
    Ok(Some(n)) => n,
    _ => {
      warn!("OIDC callback: missing nonce in session");
      return (
        StatusCode::BAD_REQUEST,
        "Invalid session — please try signing in again.",
      )
        .into_response();
    }
  };

  // 2. Exchange authorization code for tokens.
  let token_response = match state
    .oidc_client
    .exchange_code(AuthorizationCode::new(params.code))
    .request_async(openidconnect::reqwest::async_http_client)
    .await
  {
    Ok(t) => t,
    Err(e) => {
      warn!("OIDC token exchange failed: {e}");
      return (
        StatusCode::BAD_GATEWAY,
        "Authentication failed — could not exchange code.",
      )
        .into_response();
    }
  };

  // 3. Verify ID token and extract claims.
  let id_token = match token_response.id_token() {
    Some(t) => t,
    None => {
      warn!("OIDC token response contained no ID token");
      return (
        StatusCode::BAD_GATEWAY,
        "Authentication failed — no ID token returned.",
      )
        .into_response();
    }
  };

  let nonce = Nonce::new(nonce_secret);
  let claims =
    match id_token.claims(&state.oidc_client.id_token_verifier(), &nonce) {
      Ok(c) => c,
      Err(e) => {
        warn!("ID token verification failed: {e}");
        return (
          StatusCode::BAD_GATEWAY,
          "Authentication failed — ID token invalid.",
        )
          .into_response();
      }
    };

  // 4. Fetch user attributes from the userinfo endpoint.
  // Authelia places email and name in the userinfo response rather than
  // embedding them in the ID token, so we always query the endpoint after
  // token verification.  ID token claims are kept as a fallback.
  let userinfo: CoreUserInfoClaims = {
    let req = match state
      .oidc_client
      .user_info(token_response.access_token().clone(), None)
    {
      Ok(r) => r,
      Err(e) => {
        warn!("Could not build userinfo request: {e}");
        return (
          StatusCode::BAD_GATEWAY,
          "Authentication failed — no userinfo endpoint.",
        )
          .into_response();
      }
    };
    match req
      .request_async(openidconnect::reqwest::async_http_client)
      .await
    {
      Ok(u) => u,
      Err(e) => {
        warn!("Userinfo request failed: {e}");
        return (
          StatusCode::BAD_GATEWAY,
          "Authentication failed — could not fetch user info.",
        )
          .into_response();
      }
    }
  };

  let email = match userinfo.email().or_else(|| claims.email()) {
    Some(e) => e.to_string(),
    None => {
      warn!("No email in userinfo or ID token");
      return (
        StatusCode::BAD_GATEWAY,
        "Authentication failed — no email found.",
      )
        .into_response();
    }
  };

  let name = userinfo
    .name()
    .or_else(|| claims.name())
    .and_then(|n| n.get(None))
    .map(|n| n.as_str().to_owned())
    .unwrap_or_else(|| email.clone());

  let user = AuthUser { name, email };
  info!(user.email, "user authenticated");

  // 5. Store user in session.
  if let Err(e) = session.insert(KEY_USER, &user).await {
    warn!("Failed to store user in session: {e}");
    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
  }

  // 6. Redirect to where the user was going (or home).
  // Validate to a relative path so a stuffed session can't open-redirect.
  let return_to: String = session
    .remove(KEY_RETURN_TO)
    .await
    .unwrap_or(None)
    .filter(|u: &String| u.starts_with('/') && !u.starts_with("//"))
    .unwrap_or_else(|| "/".to_owned());

  Redirect::to(&return_to).into_response()
}

/// `GET /auth/logout` — clear the session and return to home.
pub async fn logout_handler(session: Session) -> impl IntoResponse {
  if let Err(e) = session.flush().await {
    warn!("Failed to flush session on logout: {e}");
  }
  Redirect::to("/")
}

// ── middleware ────────────────────────────────────────────────────────────────

/// Middleware that requires an authenticated session.
///
/// Unauthenticated requests are redirected to `/auth/login`.
/// The original URI is stored in the session so the user returns
/// to their intended destination after signing in.
pub async fn require_auth(
  session: Session,
  req: Request,
  next: Next,
) -> Response {
  let user: Option<AuthUser> = session.get(KEY_USER).await.unwrap_or(None);

  if user.is_none() {
    let return_to = req.uri().to_string();
    if let Err(e) = session.insert(KEY_RETURN_TO, return_to).await {
      warn!("Failed to save return_to in session: {e}");
    }
    return Redirect::to("/auth/login").into_response();
  }

  next.run(req).await
}

/// Extract the current user from the session, if any.
///
/// Returns `None` for unauthenticated sessions rather than failing —
/// use `require_auth` on routes that must be protected.
pub async fn current_user(session: &Session) -> Option<AuthUser> {
  session.get(KEY_USER).await.unwrap_or(None)
}
