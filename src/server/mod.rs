//! HTTP application, provider configuration and operational session management.

mod app;
mod catalog;
mod codex_login;
mod compaction_item;
mod config;
mod configuration;
#[cfg(windows)]
mod console;
mod error;
mod http;
mod management;
mod mcp;
mod media;
mod presets;
mod provider;
mod sampling;
mod session;

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
  app.set_bridge_address(address);
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
