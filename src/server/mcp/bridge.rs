//! How a session's shell finds the MCP bridge (`server::http::mcp`): the server's address, known
//! once it listens, the session's own token, and a directory that puts this build's `wish` first on
//! the shell's `PATH`.
use crate::server::data_dir::DataDir;
use std::{
  net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
  path::PathBuf,
  sync::OnceLock,
};

pub struct Bridge {
  /// The API's address, known once the listener is bound.
  url: OnceLock<String>,
  /// A directory holding only a link to this program, put first on a session shell's `PATH` so
  /// `wish mcp` runs this build.
  bin_dir: Option<PathBuf>,
}

impl Bridge {
  pub fn new(data_dir: &DataDir) -> Self {
    Self { url: OnceLock::new(), bin_dir: prepare_bin_dir(data_dir.bin()) }
  }
  /// Records the address the server listens on; an unspecified one is reached on loopback.
  pub fn set_address(&self, mut address: SocketAddr) {
    if address.ip().is_unspecified() {
      address.set_ip(match address.ip() {
        IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
      });
    }
    let _ = self.url.set(format!("http://{address}/api"));
  }
  /// The variables a session's shell gets for reaching its MCP servers through `wish mcp`.
  pub fn shell_environment(&self, session: &str, token: &str) -> Vec<(String, String)> {
    let Some(url) = self.url.get() else { return Vec::new() };
    let mut variables = vec![
      ("WISH_URL".to_owned(), url.clone()),
      ("WISH_SESSION".to_owned(), session.to_owned()),
      ("WISH_MCP_TOKEN".to_owned(), token.to_owned()),
    ];
    if let Some(bin) = &self.bin_dir {
      let inherited = std::env::var_os("PATH").unwrap_or_default();
      let paths = std::iter::once(bin.clone()).chain(std::env::split_paths(&inherited));
      if let Ok(path) = std::env::join_paths(paths) {
        variables.push(("PATH".to_owned(), path.to_string_lossy().into_owned()));
      }
    }
    variables
  }
}

/// The directory sessions' shells find `wish mcp` in: on Unix `bin` (`data_dir/bin`), holding a
/// `wish` link to this program; on Windows, where creating a link takes a privilege ordinary users
/// lack, the program's own directory. Without one, a shell finds `wish` only if it is on `PATH`
/// already.
fn prepare_bin_dir(bin: PathBuf) -> Option<PathBuf> {
  #[cfg(unix)]
  {
    let program = std::env::current_exe().ok()?;
    let directory = std::path::absolute(bin).ok()?;
    std::fs::create_dir_all(&directory).ok()?;
    let link = directory.join("wish");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&program, &link).ok()?;
    Some(directory)
  }
  #[cfg(not(unix))]
  {
    let _ = bin;
    std::env::current_exe().ok()?.parent().map(std::path::Path::to_path_buf)
  }
}
