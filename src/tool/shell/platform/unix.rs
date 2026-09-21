use std::io;

/// The group remains owned until the supervisor finishes or is dropped.
pub(in crate::tool::shell) struct ProcessTree {
  pid: u32,
  armed: bool,
}
impl ProcessTree {
  pub fn attach(child: &tokio::process::Child) -> io::Result<Self> {
    Ok(Self { pid: child.id().ok_or_else(super::missing_process)?, armed: true })
  }
  pub fn signal(&self, force: bool) -> io::Result<()> {
    // SAFETY: the child was launched with process_group(0); a negative PID addresses that group.
    let result =
      unsafe { libc::kill(-(self.pid as i32), if force { libc::SIGKILL } else { libc::SIGTERM }) };
    if result == 0 {
      return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) }
  }
  pub fn disarm(&mut self) {
    self.armed = false;
  }
}
impl Drop for ProcessTree {
  fn drop(&mut self) {
    if self.armed {
      let _ = self.signal(true);
    }
  }
}
