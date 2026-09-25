mod ask;
pub(crate) mod content;
mod directories;
mod manage;
mod providers;
mod sessions;
mod statistics;
use crate::server::{app::App, error::ApiError};
use axum::{
  Json, Router,
  extract::{Request, State},
  http::StatusCode,
  middleware::{self, Next},
  response::{IntoResponse, Response},
  routing::{get, post, put},
};
use serde_json::json;
use std::sync::Arc;

pub fn build_router(app: Arc<App>) -> Router {
  let api = Router::new()
    .route("/status", get(statistics::status))
    .route("/storage", get(statistics::storage))
    .route("/usage", get(statistics::global_usage))
    .route("/usage/series", get(statistics::global_series))
    .route("/usage/daily", get(statistics::global_daily))
    .route("/sessions/{id}/usage", get(statistics::session_usage))
    .route("/sessions/{id}/usage/series", get(statistics::session_series))
    .route("/sessions/{id}/usage/daily", get(statistics::session_daily))
    .route("/sessions/{id}/ask", post(ask::ask))
    .route("/sessions/{id}/input", post(content::input))
    .route("/sessions/{id}/blobs", post(content::upload))
    .route("/sessions/{id}/blobs/{blob}", get(content::download))
    .route("/sessions/{id}/blobs/{blob}/meta", get(content::metadata))
    .route("/sessions/{id}/history", get(content::timeline))
    .route("/config", get(manage::configuration).put(manage::save_configuration))
    .route("/proxy-environment", get(|| async { Json(crate::server::config::proxy_environment()) }))
    .route("/shells", get(|| async { Json(crate::server::config::shell_catalog()) }))
    .route("/defaults", get(manage::defaults))
    .route("/directories", get(directories::list))
    .route("/events", get(manage::events))
    .route(
      "/version",
      get(|| async { Json(json!({"name":"wish","version":env!("CARGO_PKG_VERSION")})) }),
    )
    .route("/sessions/{id}/context/clear", post(manage::clear_context))
    .route("/sessions/{id}/fork", post(manage::fork))
    .route(
      "/sessions/{id}/queue/{entry}",
      axum::routing::delete(manage::cancel_input).patch(manage::move_input),
    )
    .route("/provider-presets", get(|| async { Json(crate::server::presets::catalog()) }))
    .route("/providers", get(providers::list))
    .route("/providers/{id}", get(providers::get))
    .route("/providers/{id}/models", get(providers::models))
    .route("/providers/{id}/account", get(providers::account))
    .route(
      "/providers/{id}/chatgpt-login",
      post(crate::server::codex_login::start).get(crate::server::codex_login::status),
    )
    .route(
      "/providers/{id}/chatgpt-login/complete",
      post(crate::server::codex_login::complete),
    )
    .route("/providers/{id}/call", post(providers::call))
    .route("/providers/{id}/count-tokens", post(providers::count))
    .route("/providers/{id}/compact", post(providers::compact))
    .route("/sessions", get(sessions::list).post(sessions::create))
    .route(
      "/sessions/{id}",
      get(sessions::get).patch(manage::update_session).delete(manage::delete_session),
    )
    .route("/sessions/{id}/messages", post(sessions::enqueue))
    .route("/sessions/{id}/config", put(sessions::set_config))
    .route("/sessions/{id}/metadata", put(sessions::set_metadata))
    .route("/sessions/{id}/shell", put(sessions::set_shell))
    .route("/sessions/{id}/run", post(sessions::run))
    .route("/sessions/{id}/interrupt", post(sessions::interrupt))
    .route("/sessions/{id}/compact", post(sessions::compact))
    .route("/sessions/{id}/events", get(sessions::events))
    .route("/sessions/{id}/entries", get(sessions::entries))
    .route("/sessions/{id}/queue", get(sessions::queue))
    .route("/sessions/{id}/generations", get(sessions::generations))
    .route("/sessions/{id}/generations/{generation}/entries", get(sessions::generation_entries))
    .route("/sessions/{id}/calls", get(sessions::calls))
    .route("/sessions/{id}/history/query", post(sessions::query_history))
    .route("/sessions/{id}/history/search", post(sessions::search_history))
    .route("/sessions/{id}/history/{sequence}", get(sessions::read_history))
    .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
    .route_layer(middleware::from_fn_with_state(app.clone(), authorize));
  Router::new()
    .nest("/api", api)
    .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
    .route(
      "/version",
      get(|| async { Json(json!({"name":"wish","version":env!("CARGO_PKG_VERSION")})) }),
    )
    .fallback(|| async { ApiError::not_found() })
    .with_state(app)
}
async fn authorize(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
  if let Some(token) = &app.token {
    let supplied = request
      .headers()
      .get("authorization")
      .and_then(|h| h.to_str().ok())
      .and_then(|h| h.strip_prefix("Bearer "));
    if supplied != Some(token.as_str()) {
      return (StatusCode::UNAUTHORIZED, Json(json!({"error":{"message":"unauthorized"}})))
        .into_response();
    }
  }
  next.run(request).await
}
