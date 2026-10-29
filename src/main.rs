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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  server::run().await
}

mod capability;
