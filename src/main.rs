//! Wish's protocol, session engine and HTTP application in one executable.

mod client;
mod executor;
mod mcp;
mod migration;
mod protocol;
mod server;
mod session;
mod storage;
mod tool;
mod transport;
mod utils;

use protocol::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  server::run().await
}
