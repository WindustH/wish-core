//! Wish's protocol, session engine and HTTP application in one executable.

mod application;
mod client;
mod executor;
mod extension_host;
mod extensions;
mod mcp;
mod migration;
mod protocol;
mod server;
mod session;
mod storage;
mod task;
mod tool;
mod transport;
mod utils;

fn main() -> Result<(), Box<dyn std::error::Error>> {
  if std::env::args_os().nth(1).is_some_and(|arg| arg == "--blob-worker") {
    return application::blob_preview::worker::run();
  }
  tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(server::run())
}

mod capability;
