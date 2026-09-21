use std::{
  io,
  mem::size_of,
  os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
  Foundation::{HANDLE, INVALID_HANDLE_VALUE},
  System::{
    Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent},
    Diagnostics::ToolHelp::{
      CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    },
    JobObjects::{
      AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
      JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
      SetInformationJobObject, TerminateJobObject,
    },
    Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
  },
};

/// No breakaway permission: ordinary descendants stay in the same owned job.
pub(in crate::tool::shell) struct ProcessTree {
  job: OwnedHandle,
  pid: u32,
}
impl ProcessTree {
  pub fn attach(child: &tokio::process::Child) -> io::Result<Self> {
    let pid = child.id().ok_or_else(super::missing_process)?;
    let process = child.raw_handle().ok_or_else(super::missing_process)?;
    // SAFETY: child is alive and CREATE_SUSPENDED prevents executing the script or spawning
    // descendants before job assignment. Every acquired handle is immediately RAII-owned.
    unsafe {
      let job = own_handle(CreateJobObjectW(std::ptr::null(), std::ptr::null()))?;
      let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
      limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
      check(SetInformationJobObject(
        job.as_raw_handle(),
        JobObjectExtendedLimitInformation,
        (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
        size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
      ))?;
      check(AssignProcessToJobObject(job.as_raw_handle(), process))?;
      let tree = Self { job, pid };
      resume_main_thread(pid)?;
      Ok(tree)
    }
  }
  pub fn signal(&self, force: bool) -> io::Result<()> {
    // SAFETY: this owns a live job handle. CTRL_BREAK addresses only the new child process group;
    // services/GUI parents without a shared console fall back to terminating the owned job.
    unsafe {
      if !force && GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.pid) != 0 {
        return Ok(());
      }
      check(TerminateJobObject(self.job.as_raw_handle(), 1))
    }
  }
  // Closing the job still enforces KILL_ON_JOB_CLOSE; no late PID-based signaling is necessary.
  pub fn disarm(&mut self) {}
}
fn check(result: i32) -> io::Result<()> {
  if result == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}
unsafe fn own_handle(handle: HANDLE) -> io::Result<OwnedHandle> {
  if handle.is_null() || handle == INVALID_HANDLE_VALUE {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: the caller supplies a newly acquired, uniquely owned Win32 handle.
  Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
unsafe fn resume_main_thread(pid: u32) -> io::Result<()> {
  // std::process retains the process handle but closes CreateProcess's primary-thread handle.
  // The process is suspended, so its initial thread can be located without a launch race.
  unsafe {
    let snapshot = own_handle(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0))?;
    let mut entry =
      THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
    let mut found = Thread32First(snapshot.as_raw_handle(), &mut entry);
    while found != 0 {
      if entry.th32OwnerProcessID == pid {
        let thread = own_handle(OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID))?;
        if ResumeThread(thread.as_raw_handle()) == u32::MAX {
          return Err(io::Error::last_os_error());
        }
        return Ok(());
      }
      found = Thread32Next(snapshot.as_raw_handle(), &mut entry);
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "suspended child main thread not found"))
  }
}
