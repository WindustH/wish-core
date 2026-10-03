//! The HTTP API: the routes, and the layers every request passes - the checks a server without a
//! token makes (see [`web`]), cross-origin rules and the bearer token. Everything lives under
//! `/api` except the unauthenticated `/health` and `/version`; other paths are the web app's.
mod ask;
mod bridge;
mod config;
mod content;
mod directories;
mod history;
mod manage;
mod mcp;
mod providers;
mod search;
mod sessions;
mod skills;
mod sse;
mod status;
mod storage;
mod usage;
use crate::server::{
  app::App,
  codex_login, presets,
  web::{self, WebFront},
};
use axum::{
  Json, Router,
  extract::{Request, State},
  http::{HeaderValue, Method, StatusCode, header},
  middleware::{self, Next},
  response::{IntoResponse, Response},
  routing::{delete, get, post, put},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn build_router(app: Arc<App>, front: Arc<WebFront>) -> Router {
  let api = Router::new()
    .route("/status", get(status::status))
    .route("/events", get(status::events))
    .route("/version", get(version))
    .route("/storage", get(storage::storage))
    .route("/storage/sessions", get(storage::session_storage))
    .route("/storage/prune", post(storage::prune))
    .route("/usage", get(usage::global_usage))
    .route("/usage/series", get(usage::global_series))
    .route("/usage/daily", get(usage::global_daily))
    .route("/sessions/{id}/usage", get(usage::session_usage))
    .route("/sessions/{id}/usage/series", get(usage::session_series))
    .route("/sessions/{id}/usage/daily", get(usage::session_daily))
    .route("/config", get(config::get).put(config::save))
    .route("/proxy-environment", get(config::proxy_environment))
    .route("/shells", get(config::shells))
    .route("/defaults", get(config::defaults))
    .route("/directories", get(directories::list))
    .route("/provider-presets", get(|| async { Json(presets::provider_presets()) }))
    .route("/search-presets", get(|| async { Json(presets::search_presets()) }))
    .route("/search/providers", get(search::list))
    .route("/search/providers/{id}/check", post(search::check))
    .route("/providers", get(providers::list))
    .route("/provider-draft/models", post(providers::draft_models))
    .route("/providers/{id}", get(providers::get))
    .route("/providers/{id}/models", get(providers::models))
    .route("/providers/{id}/account", get(providers::account))
    .route("/providers/{id}/chatgpt-login", post(codex_login::start).get(codex_login::status))
    .route("/providers/{id}/chatgpt-login/complete", post(codex_login::complete))
    .route("/providers/{id}/call", post(providers::call))
    .route("/providers/{id}/count-tokens", post(providers::count))
    .route("/providers/{id}/compact", post(providers::compact))
    .route("/sessions", get(sessions::list).post(sessions::create))
    .route(
      "/sessions/{id}",
      get(sessions::get).patch(manage::update_session).delete(manage::delete_session),
    )
    .route("/sessions/{id}/fork", post(manage::fork))
    .route("/sessions/{id}/context/clear", post(manage::clear_context))
    .route("/sessions/{id}/messages", post(sessions::enqueue))
    .route("/sessions/{id}/input", post(content::input))
    .route("/sessions/{id}/queue/{entry}", delete(manage::cancel_input).patch(manage::move_input))
    .route("/sessions/{id}/blobs", post(content::upload))
    .route("/sessions/{id}/blobs/{blob}", get(content::download))
    .route("/sessions/{id}/blobs/{blob}/meta", get(content::metadata))
    .route("/sessions/{id}/config", put(sessions::set_config))
    .route("/sessions/{id}/metadata", put(sessions::set_metadata))
    .route("/sessions/{id}/shell", put(sessions::set_shell))
    .route("/sessions/{id}/tools", put(sessions::set_tools))
    .route("/sessions/{id}/answer", post(sessions::answer))
    .route("/sessions/{id}/ask", post(ask::ask))
    .route("/sessions/{id}/run", post(sessions::run))
    .route("/sessions/{id}/interrupt", post(sessions::interrupt))
    .route("/sessions/{id}/compact", post(sessions::compact))
    .route("/sessions/{id}/events", get(sessions::events))
    .route("/sessions/{id}/entries", get(sessions::entries))
    .route("/sessions/{id}/queue", get(sessions::queue))
    .route("/sessions/{id}/generations", get(sessions::generations))
    .route("/sessions/{id}/generations/{generation}/entries", get(sessions::generation_entries))
    .route("/sessions/{id}/calls", get(sessions::calls))
    .route("/sessions/{id}/history", get(history::timeline))
    .route("/sessions/{id}/history/query", post(history::query))
    .route("/sessions/{id}/history/search", post(history::search))
    .route("/sessions/{id}/history/{sequence}", get(history::read))
    .route("/mcp/servers", get(mcp::list))
    .route("/skills", get(skills::list))
    .route("/skills/{name}", get(skills::show))
    .route("/mcp/servers/{id}/check", post(mcp::check))
    .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
    .route_layer(middleware::from_fn_with_state(app.clone(), authorize))
    // The bridge a session's shell reaches its MCP servers through checks that session's own token
    // instead of the application's.
    .route("/sessions/{id}/mcp/servers", get(mcp::servers))
    .route("/sessions/{id}/mcp/tool", get(mcp::tool))
    .route("/sessions/{id}/mcp/call", post(mcp::call))
    .route("/sessions/{id}/skills", get(skills::session_list))
    .route("/sessions/{id}/skills/{name}", get(skills::session_show));
  Router::new()
    .nest("/api", api)
    .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
    .route("/version", get(version))
    .fallback({
      let front = front.clone();
      move |request: Request| async move { web::serve(&front, request).await }
    })
    .layer(middleware::from_fn_with_state(app.clone(), cross_origin))
    .layer(middleware::from_fn_with_state(front, web::guard))
    .with_state(app)
}
async fn version() -> Json<Value> {
  Json(json!({"name":"wish","version":env!("CARGO_PKG_VERSION")}))
}
/// Pages from other origins may call a server that requires a token: they cannot
/// act without knowing it. A server without one answers its own origin only, so
/// an arbitrary web page cannot drive a local agent through the visitor's browser.
async fn cross_origin(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
  if app.bearer_token.is_none() || !request.headers().contains_key(header::ORIGIN) {
    return next.run(request).await;
  }
  let preflight = request.method() == Method::OPTIONS
    && request.headers().contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
  let private_network = request.headers().contains_key("access-control-request-private-network");
  let mut response =
    if preflight { StatusCode::NO_CONTENT.into_response() } else { next.run(request).await };
  let headers = response.headers_mut();
  headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
  if preflight {
    headers.insert(
      header::ACCESS_CONTROL_ALLOW_METHODS,
      HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE"),
    );
    headers.insert(
      header::ACCESS_CONTROL_ALLOW_HEADERS,
      HeaderValue::from_static("authorization, content-type, if-match, last-event-id"),
    );
    headers.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    // Chrome asks before a public page reaches a server on a private address.
    if private_network {
      headers.insert("access-control-allow-private-network", HeaderValue::from_static("true"));
    }
  }
  response
}
async fn authorize(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
  if let Some(token) = &app.bearer_token {
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
