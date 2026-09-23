//! HTTP application, provider configuration and operational session management.

mod app;
mod config;
mod configuration;
mod error;
mod http;
mod management;
mod media;
mod presets;
mod provider;
mod sampling;
mod session;

pub(crate) async fn run() -> Result<(), Box<dyn std::error::Error>> {
  let mut args = std::env::args().skip(1);
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
  let config: config::Config = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
  let app = app::App::open(&config, path.into()).await?;
  let listener = tokio::net::TcpListener::bind(config.listen).await?;
  eprintln!("wish listening on {}", listener.local_addr()?);
  let shutdown = app.clone();
  let result = axum::serve(listener, http::build_router(app.clone()))
    .with_graceful_shutdown(async move {
      wait_for_signal().await;
      shutdown.begin_shutdown().await;
    })
    .await;
  app.begin_shutdown().await;
  app.finish_shutdown().await?;
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
  #[cfg(not(unix))]
  {
    tokio::signal::ctrl_c().await.expect("install Ctrl-C handler");
  }
}
