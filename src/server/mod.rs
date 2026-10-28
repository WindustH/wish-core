//! HTTP serving, authentication callbacks, web assets and process startup.
//! Application use cases live in `application`; persistence lives in `storage`.

use crate::application::{app, config};
mod cli;
mod codex_login;
#[cfg(windows)]
mod console;
mod copilot_login;
mod error;
mod http;
mod user_dirs;
mod web;

pub(crate) async fn run() -> Result<(), Box<dyn std::error::Error>> {
  let mut args = std::env::args().skip(1);
  match std::env::args().nth(1).as_deref() {
    Some("--help" | "-h" | "help") => {
      println!(
        "Wish: agent service and session commands\n\nServer: wish [--config PATH]\nCommands: history, task, webfetch, websearch, imagegen, videogen, audiogen, asr, askuser, mcp, skill, session\nRun wish <command> --help for its manual. Commands run inside a Wish session shell."
      );
      return Ok(());
    }
    Some(
      name @ ("webfetch" | "websearch" | "askuser" | "imagegen" | "videogen" | "audiogen" | "asr"),
    ) => std::process::exit(cli::tool_commands::run(name, args.skip(1).collect()).await),
    Some("history") => std::process::exit(cli::history::run(args.skip(1).collect()).await),
    Some("task") => std::process::exit(cli::tasks::run(args.skip(1).collect()).await),
    Some("mcp") => std::process::exit(cli::mcp::run(args.skip(1).collect()).await),
    Some("skill") => std::process::exit(cli::skills::run(args.skip(1).collect()).await),
    Some("session") => std::process::exit(cli::sessions::run(args.skip(1).collect()).await),
    _ => {}
  }
  // Named as it was started, for a link under another name.
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
  let raw: serde_json::Value = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
  let data_root = std::path::Path::new(raw["data_dir"].as_str().unwrap_or("data"));
  tokio::fs::create_dir_all(data_root).await?;
  let _data_lock = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .open(data_root.join(".wish.lock"))?;
  _data_lock.try_lock().map_err(|error| {
    format!("cannot exclusively lock data directory {}: {error}", data_root.display())
  })?;
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
  app.bridge.set_address(address);
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
