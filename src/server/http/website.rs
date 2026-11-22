//! A preview host has one upstream origin and can never fall through to the Wish router.
//! Keep its routing label separate from the credential: DNS and TLS expose host names.
use super::*;
use crate::utils::token;
use crate::{application::websites, server::error::ApiError};
use axum::{body::Body, extract::Path, http::HeaderMap};
use futures_util::{StreamExt, TryStreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone)]
pub(super) struct Proxy {
  app: Arc<App>,
  http: reqwest::Client,
}
impl Proxy {
  pub fn new(app: Arc<App>) -> Self {
    Self {
      app,
      http: reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .read_timeout(Duration::from_secs(90))
        .build()
        .expect("website preview client"),
    }
  }
}
pub async fn create(
  State(app): State<Arc<App>>,
  Path(owner): Path<String>,
  Json(input): Json<websites::Website>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(json!(websites::create(&app, &owner, input).await?)))
}
pub async fn open(
  State(app): State<Arc<App>>,
  Path((owner, id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
  let (label, secret) = websites::Previews::issue(&app, owner, id).await?;
  Ok(
    (
      [("cache-control", "no-store"), ("x-wish-site", label.as_str())],
      Json(json!({"label":label,"secret":secret})),
    )
      .into_response(),
  )
}
pub async fn close(
  State(app): State<Arc<App>>,
  Path((owner, id, label)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
  app.previews.close(&owner, &id, &label);
  Ok(StatusCode::NO_CONTENT)
}
fn cookie<'a>(cookies: &'a str, name: &str) -> Option<&'a str> {
  cookies
    .split(';')
    .filter_map(|part| part.trim().split_once('='))
    .find_map(|(k, v)| (k == name).then_some(v))
}
// Keep the site's embedding policy. Return a readable local diagnostic when it excludes the
// cross-origin Wish parent, even though the immediate frame wrapper shares the preview origin.
fn denies_embedding(headers: &HeaderMap) -> bool {
  let mut has_ancestors = false;
  for policy in headers.get_all("content-security-policy") {
    let Ok(policy) = policy.to_str() else { continue };
    for policy in policy.split(',') {
      for directive in policy.split(';') {
        let mut words = directive.split_ascii_whitespace();
        if words.next().is_some_and(|word| word.eq_ignore_ascii_case("frame-ancestors")) {
          has_ancestors = true;
          if words
            .all(|word| word.eq_ignore_ascii_case("'self'") || word.eq_ignore_ascii_case("'none'"))
          {
            return true;
          }
          break;
        }
      }
    }
  }
  !has_ancestors
    && headers
      .get_all("x-frame-options")
      .iter()
      .filter_map(|v| v.to_str().ok())
      .flat_map(|value| value.split(','))
      .any(|value| {
        ["deny", "sameorigin"].iter().any(|rule| rule.eq_ignore_ascii_case(value.trim()))
      })
}
pub async fn dispatch(State(proxy): State<Proxy>, mut req: Request, next: Next) -> Response {
  if req.headers().contains_key("x-wish-site-hop") {
    return StatusCode::FORBIDDEN.into_response();
  }
  let host = req
    .headers()
    .get(header::HOST)
    .and_then(|v| v.to_str().ok())
    .unwrap_or_default()
    .to_ascii_lowercase();
  let Some((label, _)) = host.split_once('.') else { return next.run(req).await };
  if !label.starts_with("ws-")
    || label.len() != 35
    || !label[3..].bytes().all(|c| c.is_ascii_hexdigit())
  {
    return next.run(req).await;
  }
  let Some(grant) = proxy.app.previews.get(label) else { return StatusCode::GONE.into_response() };
  if !grant.source.available().await {
    proxy.app.previews.discard(label);
    return StatusCode::GONE.into_response();
  }
  let cookies = req.headers().get(header::COOKIE).and_then(|v| v.to_str().ok()).unwrap_or_default();
  let loopback = host.split(':').next().is_some_and(|h| h.ends_with(".localhost"));
  let partitioned = cookie(cookies, "wish_view_partitioned")
    .is_some_and(|value| token::matches(&grant.secret, value));
  let secure = grant.secure_cookies.load(Ordering::Relaxed) || partitioned;
  if req
    .uri()
    .path()
    .strip_prefix("/.wish-open/")
    .is_some_and(|supplied| token::matches(&grant.secret, supplied))
    && req.method() == Method::GET
  {
    let secure = match req.uri().query() {
      Some("secure=1") => true,
      Some("secure=0") => false,
      _ => req.headers().get("x-forwarded-proto").is_some_and(|v| v == "https"),
    };
    if secure {
      grant.secure_cookies.store(true, Ordering::Relaxed);
    }
    let suffix = if secure { "; SameSite=None; Secure; Partitioned" } else { "; SameSite=Lax" };
    let path = serde_json::to_string(&grant.source.url[url::Position::BeforePath..])
      .map(|s| s.replace('<', "\\u003c"))
      .unwrap_or_default();
    let html = include_str!("../../../assets/website/frame.html").replace("SITE_PATH", &path);
    let mut response = (
      [
        ("content-type", "text/html;charset=utf-8"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
        ("set-cookie", &format!("wish_view={}; Path=/; HttpOnly{suffix}", grant.secret)),
      ],
      html,
    )
      .into_response();
    // Cross-site Chromium embeddings can use a partitioned loopback cookie.
    // Same-site HTTP clients can use the ordinary host-only cookie above.
    if !secure
      && (loopback || req.headers().get("x-wish-preview-loopback").is_some_and(|h| h == "1"))
    {
      if let Ok(cookie) = HeaderValue::from_str(&format!(
        "wish_view_partitioned={}; Path=/; HttpOnly; SameSite=None; Secure; Partitioned",
        grant.secret
      )) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
      }
    }
    return response;
  }
  if !partitioned
    && !cookie(cookies, "wish_view").is_some_and(|supplied| token::matches(&grant.secret, supplied))
  {
    return (StatusCode::FORBIDDEN, [("content-type","text/html;charset=utf-8"), ("cache-control","no-store")],
      "<!doctype html><meta name=wish-preview-error content=cookies>Website preview expired or preview cookies are blocked.")
      .into_response();
  }
  if req.uri().path() == "/.wish-view/html2canvas.js" {
    return (
      [("content-type", "text/javascript;charset=utf-8")],
      include_str!("../../../assets/website/html2canvas-1.4.1.min.js"),
    )
      .into_response();
  }
  if req.headers().contains_key("service-worker") || req.method() == Method::CONNECT {
    return StatusCode::FORBIDDEN.into_response();
  }
  if let Some(origin) = req.headers().get(header::ORIGIN)
    && ![format!("https://{host}"), format!("http://{host}")]
      .iter()
      .any(|expected| origin.as_bytes().eq_ignore_ascii_case(expected.as_bytes()))
  {
    return StatusCode::FORBIDDEN.into_response();
  }
  let mut target = grant.source.url.clone();
  target.set_path(req.uri().path());
  target.set_query(req.uri().query());
  target.set_fragment(None);
  let ws = req
    .headers()
    .get(header::UPGRADE)
    .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));

  let mut headers = HeaderMap::new();
  let connection =
    req.headers().get(header::CONNECTION).and_then(|v| v.to_str().ok()).unwrap_or("");
  for (key, value) in req.headers() {
    let name = key.as_str();
    if matches!(
      name,
      "host"
        | "origin"
        | "referer"
        | "cookie"
        | "authorization"
        | "forwarded"
        | "connection"
        | "upgrade"
        | "content-length"
        | "transfer-encoding"
        | "te"
        | "trailer"
        | "keep-alive"
    ) || ["x-wish-", "x-forwarded-", "sec-fetch-", "proxy-"].iter().any(|p| name.starts_with(p))
      || connection.split(',').any(|n| n.trim().eq_ignore_ascii_case(name))
    {
      continue;
    }
    headers.append(key, value.clone());
  }
  // Only cookies minted by the development site are returned to it.
  let site_cookies = cookies
    .split(';')
    .filter_map(|part| part.trim().strip_prefix("wish_up_"))
    .collect::<Vec<_>>()
    .join("; ");
  if let Ok(value) = HeaderValue::from_str(&site_cookies)
    && !site_cookies.is_empty()
  {
    headers.insert(header::COOKIE, value);
  }
  if req.headers().contains_key(header::ORIGIN) {
    let Ok(origin) = HeaderValue::from_str(&grant.source.url.origin().ascii_serialization()) else {
      return StatusCode::BAD_GATEWAY.into_response();
    };
    headers.insert(header::ORIGIN, origin);
  }
  if ws {
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
  }
  headers.insert("x-wish-site-hop", HeaderValue::from_static("1"));
  let downstream = ws.then(|| hyper::upgrade::on(&mut req));
  let embedded =
    req.headers().get("sec-fetch-dest").is_some_and(|dest| dest == "iframe" || dest == "frame");
  let method = req.method().clone();
  let mut upstream = proxy.http.request(method.clone(), target.clone()).headers(headers);
  const MAX_UPLOAD: usize = 32 * 1024 * 1024;
  let exceeded = Arc::new(AtomicBool::new(false));
  if !matches!(method, Method::GET | Method::HEAD) {
    if req
      .headers()
      .get(header::CONTENT_LENGTH)
      .and_then(|v| v.to_str().ok())
      .and_then(|v| v.parse::<u64>().ok())
      .is_some_and(|n| n > MAX_UPLOAD as u64)
    {
      return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let limit = exceeded.clone();
    let mut remaining = MAX_UPLOAD;
    let upload =
      req.into_body().into_data_stream().map_err(std::io::Error::other).and_then(move |chunk| {
        let result = if chunk.len() <= remaining {
          remaining -= chunk.len();
          Ok(chunk)
        } else {
          limit.store(true, Ordering::Release);
          Err(std::io::Error::other("preview upload exceeds limit"))
        };
        std::future::ready(result)
      });
    upstream = upstream.body(reqwest::Body::wrap_stream(upload));
  }
  let response = tokio::select! {
    biased;
    _=grant.closed()=>return StatusCode::GONE.into_response(),
    response=tokio::time::timeout(Duration::from_secs(30), upstream.send())=>response
  };
  if exceeded.load(Ordering::Acquire) {
    return StatusCode::PAYLOAD_TOO_LARGE.into_response();
  }
  let res = match response {
    Ok(Ok(res)) => res,
    _ => {
      return (
        StatusCode::BAD_GATEWAY,
        [("cache-control", "no-store"), ("content-type", "text/html;charset=utf-8")],
        "<!doctype html><meta name=wish-preview-error content=unavailable>Website unavailable",
      )
        .into_response();
    }
  };
  let status = res.status();
  if embedded && !status.is_redirection() && denies_embedding(res.headers()) {
    return (StatusCode::FORBIDDEN, [("content-type","text/html;charset=utf-8"), ("cache-control","no-store")],
      "<!doctype html><meta name=wish-preview-error content=embedding>Website policy blocks embedded previews").into_response();
  }
  let mut out = Response::builder()
    .status(status)
    .header("referrer-policy", "no-referrer")
    .header("cache-control", "no-store");
  for (key, value) in res.headers() {
    match key.as_str() {
      "location" => {
        if let Ok(raw) = value.to_str()
          && let Ok(url) = target.join(raw)
        {
          if url.origin() != grant.source.url.origin() {
            return (StatusCode::BAD_GATEWAY, "Redirect leaves this local website").into_response();
          }
          out = out.header(key, format!("/.{}", &url[url::Position::BeforePath..]));
        }
      }
      "set-cookie" => {
        if let Ok(raw) = value.to_str() {
          let mut parts = raw.split(';');
          let mut scoped = format!("wish_up_{}", parts.next().unwrap_or_default());
          for part in parts {
            let attr = part.trim().split('=').next().unwrap_or_default().trim();
            if attr.eq_ignore_ascii_case("domain")
              || (secure
                && ["samesite", "secure", "partitioned"]
                  .iter()
                  .any(|a| attr.eq_ignore_ascii_case(a)))
            {
              continue;
            }
            scoped.push(';');
            scoped.push_str(part);
          }
          if secure {
            scoped.push_str("; SameSite=None; Secure; Partitioned");
          }
          out = out.header(key, scoped);
        }
      }
      "content-type"
      | "x-frame-options"
      | "content-length"
      | "content-encoding"
      | "content-range"
      | "accept-ranges"
      | "content-security-policy"
      | "etag"
      | "last-modified"
      | "sec-websocket-accept"
      | "sec-websocket-protocol"
      | "sec-websocket-extensions" => out = out.header(key, value),
      _ => {}
    }
  }
  if status == StatusCode::SWITCHING_PROTOCOLS {
    let Some(local) = downstream else { return StatusCode::BAD_GATEWAY.into_response() };
    if proxy.app.lifecycle.while_open(|| proxy.app.lifecycle.tasks.spawn(async move {
      tokio::select! {
        _=grant.closed()=>{},
        _=async {
          if let (Ok(local), Ok(mut remote)) = tokio::join!(local, res.upgrade()) {
            let _ = tokio::io::copy_bidirectional(&mut hyper_util::rt::TokioIo::new(local), &mut remote).await;
          }
        }=>{}
      }
    })).is_none() { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
    out = out.header("connection", "upgrade").header("upgrade", "websocket");
    return out.body(Body::empty()).unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
  }
  out
    .body(Body::from_stream(res.bytes_stream().take_until(async move { grant.closed().await })))
    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}
