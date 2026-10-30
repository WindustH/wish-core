//! HTTP login adapters and the local browser callback listener.

use crate::application::app::App;
use crate::application::browser_login::{LOGIN_LIFETIME, LoginGrant, LoginKind};
use crate::application::credentials::LoginStatus;
use crate::server::error::ApiError;
use axum::{
  Json, Router,
  extract::{Path, Query, State},
  http::StatusCode,
  response::{Html, IntoResponse, Response},
  routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

struct Callback {
  app: Arc<App>,
  attempt_id: String,
  grant: LoginGrant,
  provider: String,
}

#[derive(Deserialize)]
pub struct CallbackUrl {
  callback_url: String,
}

pub async fn start_chatgpt(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  start(app, id, LoginKind::Chatgpt).await
}
pub async fn start_google(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  start(app, id, LoginKind::Google).await
}
async fn start(
  app: Arc<App>,
  provider_id: String,
  kind: LoginKind,
) -> Result<Json<Value>, ApiError> {
  let crate::application::browser_login::PendingLogin {
    callback_endpoint: listener,
    attempt_id,
    grant,
    authorization_url: url,
  } = crate::application::browser_login::begin(&app, &provider_id, kind, async {
    let listener = bind_callback(kind).await?;
    let port = listener.local_addr().map_err(ApiError::internal)?.port();
    let path = if kind == LoginKind::Google { "/oauth-callback" } else { "/auth/callback" };
    Ok((listener, format!("http://localhost:{port}{path}")))
  })
  .await?;
  let end = grant.end.clone();
  let context = Arc::new(Callback {
    app: Arc::clone(&app),
    attempt_id: attempt_id.clone(),
    grant,
    provider: provider_id,
  });
  let path = if kind == LoginKind::Google { "/oauth-callback" } else { "/auth/callback" };
  let router = Router::new().route(path, get(callback)).with_state(context);
  let shutdown = app.lifecycle.stop.clone();
  let worker_app = Arc::clone(&app);
  app.lifecycle.tasks.spawn(async move {
    let _ = axum::serve(listener, router)
      .with_graceful_shutdown(async move {
        tokio::select! {
          _ = end.cancelled() => {},
          _ = shutdown.cancelled() => {},
          _ = tokio::time::sleep(LOGIN_LIFETIME) => {},
        }
      })
      .await;
    worker_app.browser_login.finish(&attempt_id, LoginStatus::Expired, None).await;
  });
  Ok(Json(json!({"authorization_url": url.as_str(), "expires_in": LOGIN_LIFETIME.as_secs()})))
}

pub async fn status_chatgpt(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(crate::application::browser_login::status(app, id, LoginKind::Chatgpt).await?))
}
pub async fn status_google(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(crate::application::browser_login::status(app, id, LoginKind::Google).await?))
}
pub async fn complete_chatgpt(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<CallbackUrl>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    crate::application::browser_login::complete(app, id, input.callback_url, LoginKind::Chatgpt)
      .await?,
  ))
}
pub async fn complete_google(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<CallbackUrl>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    crate::application::browser_login::complete(app, id, input.callback_url, LoginKind::Google)
      .await?,
  ))
}

async fn bind_callback(kind: LoginKind) -> Result<tokio::net::TcpListener, ApiError> {
  #[cfg(debug_assertions)]
  if let Ok(port) = std::env::var(if kind == LoginKind::Google {
    "WISH_TEST_GOOGLE_CALLBACK_PORT"
  } else {
    "WISH_TEST_CODEX_CALLBACK_PORT"
  }) {
    let port: u16 = port
      .parse()
      .map_err(|error: std::num::ParseIntError| ApiError::invalid(error.to_string()))?;
    return tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(ApiError::internal);
  }
  for _ in 0..20 {
    let ports: &[u16] =
      if kind == LoginKind::Google { &[51121, 51122, 51123] } else { &[1455, 1457] };
    for &port in ports {
      if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        return Ok(listener);
      }
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
  Err(ApiError::conflict("Browser login callback ports are unavailable"))
}

async fn callback(
  State(context): State<Arc<Callback>>,
  Query(query): Query<HashMap<String, String>>,
) -> Response {
  use crate::application::browser_login::{CallbackFailure, receive_callback};
  match receive_callback(&context.app,&context.attempt_id,&context.grant,&context.provider,
    query.get("state").map(String::as_str),query.get("code").map(String::as_str),query.get("error").map(String::as_str)).await {
    Ok(()) => Html("<meta charset=\"utf-8\"><h1>登录成功</h1><p>可以返回 Wish。</p><script>window.close()</script>").into_response(),
    Err(failure)=>{
      let (status,message)=match failure {
       CallbackFailure::StateMismatch=>(StatusCode::BAD_REQUEST,"Authorization state did not match"),
       CallbackFailure::Declined=>(StatusCode::BAD_REQUEST,"Browser authorization was declined"),
       CallbackFailure::MissingCode=>(StatusCode::BAD_REQUEST,"Authorization code is missing"),
       CallbackFailure::Inactive=>(StatusCode::CONFLICT,"Login attempt is no longer active"),
       CallbackFailure::RedemptionFailed=>(StatusCode::BAD_GATEWAY,"Browser login failed; return to Wish for details"),
      };
      (status,Html(message)).into_response()
    }
  }
}
