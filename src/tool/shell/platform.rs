use std::{
  io,
  path::{Path, PathBuf},
};
use tokio::process::Command;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub(super) use unix::ProcessTree;
#[cfg(windows)]
pub(super) use windows::ProcessTree;

pub(super) fn default_program() -> PathBuf {
  #[cfg(windows)]
  {
    std::env::var_os("COMSPEC").map(PathBuf::from).unwrap_or_else(|| "cmd.exe".into())
  }
  #[cfg(not(windows))]
  {
    "/bin/sh".into()
  }
}
pub(super) fn default_args() -> Vec<String> {
  #[cfg(windows)]
  {
    vec!["/D".into(), "/S".into(), "/C".into()]
  }
  #[cfg(not(windows))]
  {
    vec!["-c".into()]
  }
}
/// Arguments a shell takes before command text, chosen by the family its file name names. Login
/// flags load the profile a user's `PATH` usually lives in; an unknown shell gets POSIX `-c`.
pub(super) fn family_args(program: &Path) -> Vec<String> {
  let name =
    program.file_stem().map(|name| name.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
  let args: &[&str] = match name.as_str() {
    "zsh" | "bash" => &["-lc"],
    "fish" => &["-l", "-c"],
    "pwsh" | "powershell" => &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"],
    "cmd" => &["/D", "/S", "/C"],
    _ => &["-c"],
  };
  args.iter().map(|arg| (*arg).to_owned()).collect()
}
/// The first match on `PATH` for each shell name this platform commonly has.
pub(super) fn installed_shells() -> Vec<PathBuf> {
  #[cfg(windows)]
  let names = ["pwsh.exe", "powershell.exe", "cmd.exe"].as_slice();
  #[cfg(not(windows))]
  let names = ["zsh", "bash", "fish", "sh", "dash", "ksh", "nu", "pwsh"].as_slice();
  let Some(path) = std::env::var_os("PATH") else {
    return Vec::new();
  };
  let directories: Vec<_> =
    std::env::split_paths(&path).filter(|directory| !directory.as_os_str().is_empty()).collect();
  names
    .iter()
    .filter_map(|name| {
      directories.iter().map(|directory| directory.join(name)).find(|path| is_executable_file(path))
    })
    .collect()
}
pub(super) fn is_executable_file(path: &Path) -> bool {
  let Ok(metadata) = std::fs::metadata(path) else {
    return false;
  };
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
  }
  #[cfg(not(unix))]
  {
    metadata.is_file()
  }
}
pub(super) fn configure_command(builder: &mut Command, program: &Path, command: &str) {
  #[cfg(unix)]
  {
    let _ = program;
    builder.arg(command).process_group(0);
  }
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED};
    // cmd parses /C itself rather than using CRT argument decoding. /S strips the outer quotes;
    // interior quotes and metacharacters are the caller's shell script and must remain untouched.
    if program
      .file_name()
      .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("cmd.exe"))
    {
      builder.as_std_mut().raw_arg(format!("\"{command}\""));
    } else {
      builder.arg(command);
    }
    builder.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED);
  }
  #[cfg(not(any(unix, windows)))]
  {
    let _ = program;
    builder.arg(command);
  }
}

#[cfg(not(any(unix, windows)))]
pub(super) struct ProcessTree;
#[cfg(not(any(unix, windows)))]
impl ProcessTree {
  pub fn attach(_: &tokio::process::Child) -> io::Result<Self> {
    Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "shell process supervision requires Unix or Windows",
    ))
  }
  pub fn signal(&self, _: bool) -> io::Result<()> {
    Ok(())
  }
  pub fn disarm(&mut self) {}
}

// Kept here so all platform constructors report a missing child handle consistently.
pub(super) fn missing_process() -> io::Error {
  io::Error::other("spawned child has no process handle")
}
