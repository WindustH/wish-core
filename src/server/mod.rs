//! The Wish server: one process serving the HTTP API, driving sessions on the engine and calling
//! providers.
//!
//! - `app` holds everything a request reaches, and the lifecycle from startup to shutdown;
//! - `http` is the API; its handlers parse, check and answer, and leave the work to the rest;
//! - `session` holds open sessions and runs them; `session_model` is the model a session calls;
//! - `config` is the configuration file's schema, `config_file` the file as the server holds it;
//! - `provider`, `search` and `mcp` are what a configuration builds: model providers, search
//!   providers and MCP servers;
//! - `management` is the index of sessions, model calls and stream samples (`sampling`), and
//!   `data_dir` and `blobs` are the files beside it.

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

pub(crate) async fn run() -> Result<(), Box<dyn std::error::Error>> {
  let mut args = std::env::args().skip(1);
  if std::env::args().nth(1).as_deref() == Some("mcp") {
    std::process::exit(mcp::cli::run(args.skip(1).collect()).await);
  }
  let path = match (args.next().as_deref(), args.next(), args.next()) {
    (Some("--config"), Some(path), None) => path,
    (Some("--help"), None, None) | (None, None, None) => {
      println!("wish --config <config.json>\nOne HTTP service for providers and sessions.");
      return Ok(());
    }
    (Some("--version"), None, None) => {
      println!("wish {}", env!("CARGO_PKG_VERSION"));
      return Ok(());
    }
    _ => return Err("usage: wish --config <config.json>".into()),
  };
  crate::migration::run(std::path::Path::new(&path))?;
  #[cfg(windows)]
  console::install();
  let config: config::Config = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
  let app = app::App::open(&config, path.into()).await?;
  let listener = tokio::net::TcpListener::bind(config.listen).await?;
  let address = listener.local_addr()?;
  eprintln!("wish listening on {address}");
  app.mcp.bridge.set_address(address);
  let shutdown = app.clone();
  let result = axum::serve(listener, http::build_router(app.clone()))
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
