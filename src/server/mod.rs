//! The Wish server: one process serving the HTTP API and the web app, driving sessions on the
//! engine and calling providers.
//!
//! - `app` holds everything a request reaches, and the lifecycle from startup to shutdown;
//! - `http` is the API; its handlers parse, check and answer, and leave the work to the rest;
//! - `session` holds open sessions and runs them; `session_model` is the model a session calls;
//! - `config` is the configuration file's schema, `config_file` the file as the server holds it;
//! - `provider`, `search` and `mcp` are what a configuration builds: model providers, search
//!   providers and MCP servers;
//! - `management` is the index of sessions, model calls and stream samples (`sampling`), and
//!   `data_dir` and `blobs` are the files beside it;
//! - `web` hands out the web app beside the API and guards a server without a token;
//!   `user_dirs` is where a start without `--config` finds its configuration.

mod app;
mod blobs;
mod codex_login;
mod compaction_item;
mod config;
mod config_file;
#[cfg(windows)]
mod console;
mod data_dir;
mod error;
mod http;
mod management;
mod mcp;
mod model_catalog;
mod presets;
mod provider;
mod sampling;
mod search;
mod session;
mod session_model;
mod user_dirs;
mod web;

pub(crate) async fn run() -> Result<(), Box<dyn std::error::Error>> {
  let mut args = std::env::args().skip(1);
  if std::env::args().nth(1).as_deref() == Some("mcp") {
    std::process::exit(mcp::cli::run(args.skip(1).collect()).await);
  }
  // Named as it was started: `wish-agent` where a package installs it so.
  let name = std::env::args()
    .next()
    .and_then(|program| {
      std::path::Path::new(&program).file_stem().map(|stem| stem.to_string_lossy().into_owned())
    })
    .unwrap_or_else(|| "wish".into());
  let path = match (args.next().as_deref(), args.next(), args.next()) {
    (Some("--config"), Some(path), None) => std::path::PathBuf::from(path),
    (None, None, None) => user_dirs::ensure_config()?,
    (Some("--help"), None, None) => {
      println!(
        "{name} [--config <config.json>]\nRuns Wish: its HTTP API and, when installed beside it, \
         its web app.\nWithout --config it reads the user's configuration, written on first start."
      );
      return Ok(());
    }
    (Some("--version"), None, None) => {
      println!("{name} {}", env!("CARGO_PKG_VERSION"));
      return Ok(());
    }
    _ => return Err(format!("usage: {name} [--config <config.json>]").into()),
  };
  crate::migration::run(&path)?;
  #[cfg(windows)]
  console::install();
  let config: config::Config = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
  let web_app = web::find_app_dir(config.web_dir.as_deref())?;
  let app = app::App::open(&config, path).await?;
  let listener = tokio::net::TcpListener::bind(config.listen)
    .await
    .map_err(|error| format!("listen on {}: {error}", config.listen))?;
  let address = listener.local_addr()?;
  eprintln!("wish listening on {address}");
  app.mcp.bridge.set_address(address);
  let front = std::sync::Arc::new(web::WebFront::new(
    web_app,
    address,
    &config.allowed_hosts,
    app.bearer_token.is_some(),
  ));
  if front.app_dir().is_some() {
    let browsable = if address.ip().is_unspecified() {
      std::net::SocketAddr::from(([127, 0, 0, 1], address.port()))
    } else {
      address
    };
    eprintln!("open http://{browsable} in a browser");
  }
  let shutdown = app.clone();
  let result = axum::serve(listener, http::build_router(app.clone(), front))
    .with_graceful_shutdown(async move {
      wait_for_signal().await;
      shutdown.begin_shutdown().await;
    })
    .await;
  app.begin_shutdown().await;
  let finished = app.finish_shutdown().await;
  #[cfg(windows)]
  console::release();
  finished?;
  result?;
  Ok(())
}

async fn wait_for_signal() {
  #[cfg(unix)]
  {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
      .expect("install SIGTERM handler");
    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
  }
  #[cfg(windows)]
  {
    let mut ctrl_break = tokio::signal::windows::ctrl_break().expect("install Ctrl-Break handler");
    tokio::select! {
      _ = tokio::signal::ctrl_c() => {},
      _ = ctrl_break.recv() => {},
      _ = console::wait_for_close() => {},
    }
  }
}
