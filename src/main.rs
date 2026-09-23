//! Wish's protocol, session engine and HTTP application in one executable.

pub mod executor;
pub mod protocol;
mod server;
pub mod session;
pub mod storage;
pub mod tool;
pub mod transport;
pub mod utils;

pub use protocol::error::Error;
pub use utils::retry::RetryPolicy;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  server::run().await
}
