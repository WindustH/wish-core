//! Session-scoped process supervision, task records and foreground/background ownership.
use super::{
  DataEncoding, KillMode, TaskConfig, TaskError, TaskOperation,
  edit::{EditCapture, EditResult},
  platform::{self, ProcessTree},
  process::{self, Execution, Snapshot, Status},
};
use super::{ForegroundResult, InputArrival};
use crate::utils::control::ExecutionControl;
use base64::Engine;
use serde_json::{Value, json};
use std::{
  collections::HashMap,
  io,
  process::Stdio,
  sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
  },
  time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
  io::{AsyncReadExt, AsyncSeekExt},
  process::Command,
  sync::{mpsc, watch},
};

/// Share one instance within a session, including across executor::run invocations. Its registry
/// lock covers lookup, and spawn plus registration so shutdown never misses a started process; no
/// asynchronous I/O runs under that lock.
#[derive(Clone)]
pub struct TaskManager {
  inner: Arc<Runtime>,
}
struct Runtime {
  config: TaskConfig,
  registry: Mutex<Registry>,
}
#[derive(Default)]
struct Registry {
  closed: bool,
  executions: HashMap<String, Arc<Execution>>,
}
impl Drop for Runtime {
  fn drop(&mut self) {
    let registry = self.registry.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner());
    for execution in registry.executions.values() {
      let _ = execution.kills.send(KillMode::Force);
    }
  }
}
struct Foreground {
  execution: Arc<Execution>,
  returned: bool,
}
impl Drop for Foreground {
  fn drop(&mut self) {
    if !self.returned && !*self.execution.background.borrow() {
      let _ = self.execution.kills.send(KillMode::Force);
    }
  }
}

impl TaskManager {
  pub async fn new(mut config: TaskConfig) -> Result<Self, TaskError> {
    if !cfg!(any(unix, windows)) {
      return Err(
        io::Error::new(io::ErrorKind::Unsupported, "shell supervision requires Unix or Windows")
          .into(),
      );
    }
    // Validate the working directory when starting a command, not when opening its history.
    // Otherwise a deleted checkout makes the session impossible to repair through the API.
    for duration in [config.soft_timeout, config.kill_grace, config.stdin_write_timeout] {
      resolve_deadline(duration)?;
    }
    tokio::fs::create_dir_all(&config.capture_dir).await?;
    config.capture_dir = tokio::fs::canonicalize(&config.capture_dir).await?;
    let mut registry = Registry::default();
    for record in crate::storage::task_records::load(&config.capture_dir).await? {
      let Some(id) = record["task_id"].as_str() else { continue };
      if !super::is_execution_id(id) {
        continue;
      }
      let Ok(mut snapshot) = serde_json::from_value::<Snapshot>(record["process"].clone()) else {
        eprintln!("ignoring invalid task process record: {id}");
        continue;
      };
      snapshot.edits =
        Arc::new(serde_json::from_value(record["edits"].clone()).unwrap_or_default());
      let recovered = snapshot.status == Status::Running;
      if recovered {
        snapshot.status = Status::Interrupted;
        snapshot.error = Some("Wish restarted before task completion was recorded".into());
      }
      let (state, _) = watch::channel(snapshot);
      let (kills, _) = mpsc::unbounded_channel();
      let (background, _) = watch::channel(record["background"].as_bool().unwrap_or(true));
      let (initial_input_done, _) = watch::channel(true);
      let execution = Arc::new(Execution {
        id: id.to_owned(),
        command: record["command"].as_str().unwrap_or_default().into(),
        started_at: record["started_at"].as_u64().unwrap_or_default(),
        background,
        notification_claimed: false.into(),
        record_lock: tokio::sync::Mutex::new(()),
        output_path: config.capture_dir.join(id).join("output.log"),
        stdin: tokio::sync::Mutex::new(None),
        state,
        initial_input_done,
        kills,
        write_timeout: config.stdin_write_timeout,
      });
      if recovered {
        execution.persist().await?;
      }
      registry.executions.insert(id.to_owned(), execution);
    }
    Ok(Self { inner: Arc::new(Runtime { config, registry: Mutex::new(registry) }) })
  }

  fn lock_registry(&self) -> Result<std::sync::MutexGuard<'_, Registry>, TaskError> {
    self
      .inner
      .registry
      .lock()
      .map_err(|_| TaskError::Unknown("shell registry lock poisoned".into()))
  }
  pub(super) fn get_execution(&self, id: &str) -> Result<Arc<Execution>, TaskError> {
    self
      .lock_registry()?
      .executions
      .get(id)
      .cloned()
      .ok_or_else(|| TaskError::ExecutionNotFound(id.into()))
  }
  /// Whether the execution `id` is still running, so its capture is still being written.
  pub fn is_running(&self, id: &str) -> bool {
    self.get_execution(id).is_ok_and(|execution| execution.get_snapshot().status == Status::Running)
  }
  pub fn forget_removed(&self) {
    if let Ok(mut registry) = self.lock_registry() {
      registry.executions.retain(|_, task| {
        task.get_snapshot().status == Status::Running || task.output_path.exists()
      });
    }
  }
  /// Stop all owned process groups and await reaping. Captures and completed handles are retained.
  pub async fn shutdown(&self) -> Result<(), TaskError> {
    let executions: Vec<_> = {
      let mut registry = self.lock_registry()?;
      registry.closed = true;
      registry.executions.values().cloned().collect()
    };
    let results = futures_util::future::join_all(
      executions.iter().map(|execution| execution.terminate(KillMode::Force)),
    )
    .await;
    for result in results {
      result?;
    }
    Ok(())
  }
  /// Wait for completion and return final status with a bounded output tail and edit summary.
  /// Applications can enqueue this result as a background completion notification.
  pub async fn wait_for_completion(&self, id: &str) -> Result<Value, TaskError> {
    let execution = self.get_execution(id)?;
    // Unknown is a terminal outcome too; retain the diagnostic in the returned snapshot.
    let _ = execution.wait_for_exit().await;
    let total = tokio::fs::metadata(&execution.output_path).await?.len();
    let mut output =
      read_output(&execution, total.saturating_sub(8192), 8192, DataEncoding::Utf8).await?;
    output["truncated"] = json!(total > 8192);
    output["edits"] = json!(*execution.get_snapshot().edits);
    output["edits_path"] = json!(execution.output_path.with_file_name("task.json"));
    Ok(output)
  }
  pub async fn run(
    &self,
    operation: TaskOperation,
    control: &ExecutionControl,
  ) -> Result<Value, TaskError> {
    if control.is_cancelled() {
      return Err(TaskError::Interrupted);
    }

    match operation {
      TaskOperation::Start { command, timeout, data, encoding, interactive } => {
        let timeout = match timeout {
          None | Some(-1.0) => Ok(self.inner.config.soft_timeout),
          Some(seconds) => Duration::try_from_secs_f64(seconds)
            .map_err(|error| TaskError::InvalidArguments(error.to_string())),
        };
        match (timeout, data.map(|data| encoding.decode(data)).transpose()) {
          (Ok(timeout), Ok(data)) => self.start(command, timeout, data, interactive, control).await,
          (Err(error), _) | (_, Err(error)) => Err(error),
        }
      }
      TaskOperation::Poll { execution_id, offset, max_bytes, wait_ms, encoding } => {
        self
          .poll(
            &execution_id,
            offset,
            max_bytes.unwrap_or(self.inner.config.poll_bytes),
            Duration::from_millis(wait_ms),
            encoding,
            control,
          )
          .await
      }
      TaskOperation::Write { execution_id, data, encoding, close } => {
        match (self.get_execution(&execution_id), encoding.decode(data)) {
          (Ok(execution), Ok(bytes)) => {
            let result = execution.write_bytes(&bytes, close, control, true).await;
            Ok(json!({"execution_id": execution_id, "write": result}))
          }
          (Err(error), _) | (_, Err(error)) => Err(error),
        }
      }
      TaskOperation::Kill { execution_id, mode } => match self.get_execution(&execution_id) {
        Ok(execution) => execution
          .terminate(mode)
          .await
          .map(|snapshot| json!({"execution_id": execution_id, "process": snapshot})),
        Err(error) => Err(error),
      },
    }
  }
  pub fn list(&self) -> Result<Value, TaskError> {
    let mut tasks: Vec<_> =
      self.lock_registry()?.executions.values().map(|task| task.describe()).collect();
    tasks.sort_by_key(|task| std::cmp::Reverse(task["started_at"].as_u64().unwrap_or(0)));
    Ok(json!({"tasks":tasks}))
  }
  pub async fn remove(&self, id: &str) -> Result<Value, TaskError> {
    let task = {
      let mut registry = self.lock_registry()?;
      let task =
        registry.executions.get(id).ok_or_else(|| TaskError::ExecutionNotFound(id.into()))?;
      if task.get_snapshot().status == Status::Running {
        return Err(TaskError::InvalidArguments("stop the task before removing it".into()));
      }
      registry.executions.remove(id).unwrap()
    };
    let _record = task.record_lock.lock().await;
    if let Err(error) =
      crate::storage::task_records::remove(task.output_path.parent().unwrap()).await
    {
      self.lock_registry()?.executions.insert(id.into(), task.clone());
      return Err(error.into());
    }
    Ok(json!({"removed":id}))
  }
  pub fn show(&self, id: &str) -> Result<Value, TaskError> {
    Ok(self.get_execution(id)?.describe())
  }
  pub fn claim_notification(&self, id: &str) -> bool {
    self.get_execution(id).is_ok_and(|task| !task.notification_claimed.swap(true, Ordering::AcqRel))
  }
  pub async fn background(&self, id: &str) -> Result<Value, TaskError> {
    let task = self.get_execution(id)?;
    if task.get_snapshot().status != Status::Running {
      return Err(TaskError::InvalidArguments("only a running task can be backgrounded".into()));
    }
    task.background.send_replace(true);
    task.persist().await?;
    Ok(task.describe())
  }
  #[allow(dead_code)] // standalone engine/tool fixtures
  pub async fn foreground(
    &self,
    command: String,
    timeout: Option<f64>,
    paths: Vec<std::path::PathBuf>,
    check_diff: bool,
    control: &ExecutionControl,
  ) -> Result<ForegroundResult, TaskError> {
    self.foreground_responsive(command, timeout, paths, check_diff, control, None).await
  }
  pub async fn foreground_responsive(
    &self,
    command: String,
    timeout: Option<f64>,
    paths: Vec<std::path::PathBuf>,
    check_diff: bool,
    control: &ExecutionControl,
    mut input: Option<InputArrival>,
  ) -> Result<ForegroundResult, TaskError> {
    let deadline = match timeout {
      Some(seconds) if seconds.is_finite() && seconds > 0.0 => Some(resolve_deadline(
        Duration::try_from_secs_f64(seconds)
          .map_err(|e| TaskError::InvalidArguments(e.to_string()))?,
      )?),
      Some(_) => {
        return Err(TaskError::InvalidArguments(
          "timeout must be a positive finite number of seconds".into(),
        ));
      }
      None => None,
    };
    if paths.len() > 16 {
      return Err(TaskError::InvalidArguments("at most 16 edit paths are allowed".into()));
    }
    let mut edits = Vec::with_capacity(paths.len());
    for path in paths {
      edits.push(EditCapture::capture(path).await?);
    }
    let task = self.spawn(command, None, false, edits, control).await?;
    let mut guard = Foreground { execution: task.clone(), returned: false };
    let mut detached = task.background.subscribe();
    // The hard deadline belongs to the process, not to its foreground waiter. Detaching
    // must not remove it. This is one deadline timer raced against exit, not a poll loop.
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(deadline) = deadline {
      let (execution, expired) = (task.clone(), timed_out.clone());
      tokio::spawn(async move {
        tokio::select! {
          biased;
          _ = execution.wait_for_exit() => {}
          _ = tokio::time::sleep_until(deadline) => {
            expired.store(true, Ordering::Release);
            execution.state.send_modify(|state| state.error = Some("Command exceeded its hard timeout".into()));
            let _ = execution.terminate(KillMode::Graceful).await;
          }
        }
      });
    }
    let mut yielded_for_input = false;
    let reason = loop {
      tokio::select! {
        biased;
        _ = control.wait_for_cancellation() => { task.terminate(KillMode::Graceful).await?; break "interrupted"; }
        result = task.wait_for_exit() => {
          result?;
          break if timed_out.load(Ordering::Acquire) { "timeout" } else { "exited" };
        }
        _ = async { let _ = detached.wait_for(|value| *value).await; } => break "backgrounded",
        arrived = async {
          match input.as_mut() { Some(input) => input.await, None => std::future::pending().await }
        } => {
          match arrived {
            Ok(()) => { self.background(&task.id).await?; yielded_for_input = true; break "backgrounded"; }
            Err(error) => { eprintln!("input notification: {error}"); input = None; }
          }
        }
      }
    };
    guard.returned = true;
    let total = tokio::fs::metadata(&task.output_path).await?.len();
    let mut output =
      read_output(&task, total.saturating_sub(50 * 1024), 50 * 1024, DataEncoding::Utf8).await?;
    let text = output["text"].as_str().unwrap_or_default().to_owned();
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    if lines.len() > 2000 {
      let removed: usize = lines[..lines.len() - 2000].iter().map(|line| line.len()).sum();
      output["start_offset"] = json!(output["start_offset"].as_u64().unwrap_or(0) + removed as u64);
      output["text"] = lines[lines.len() - 2000..].concat().into();
    }
    output["truncated"] = json!(total > 50 * 1024 || lines.len() > 2000);
    output["return_reason"] = reason.into();
    output["task_id"] = task.id.clone().into();
    output["background"] = json!(*task.background.borrow());
    output["yielded_for_input"] = json!(yielded_for_input);
    let edits = json!(*task.get_snapshot().edits);
    let mut summaries = edits.clone();
    if !check_diff {
      for item in summaries.as_array_mut().into_iter().flatten() {
        if let Some(item) = item.as_object_mut() {
          item.remove("diff");
        }
      }
    }
    output["edits"] = summaries;
    // Preserve captured diagnostics and the task identifier even when the command fails.
    output["success"] = json!(
      reason == "backgrounded"
        || (reason == "exited"
          && task.get_snapshot().exit_code == Some(0)
          && task.get_snapshot().error.is_none())
    );
    if !edits.as_array().is_none_or(Vec::is_empty) {
      // The supervisor persists final edits here before publishing completion.
      // Refer to that record, including for a foreground command detached mid-edit.
      output["edits_path"] = json!(task.output_path.with_file_name("task.json"));
    }
    let failed = output["success"] != true;
    Ok(ForegroundResult { output, edits, failed })
  }
  async fn spawn(
    &self,
    command: String,
    data: Option<Vec<u8>>,
    interactive: bool,
    edits: Vec<EditCapture>,
    control: &ExecutionControl,
  ) -> Result<Arc<Execution>, TaskError> {
    if command.trim().is_empty() {
      return Err(TaskError::InvalidArguments(
        "`command` must be non-empty; for example, wish task start -- 'pwd'".into(),
      ));
    }
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let id =
      format!("{}-{timestamp}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let directory = self.inner.config.capture_dir.join(&id);
    tokio::fs::create_dir(&directory).await?;
    let output_path = directory.join("output.log");
    let file =
      tokio::fs::OpenOptions::new().write(true).create_new(true).open(&output_path).await?;
    let shell =
      self.inner.config.command.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let mut builder = Command::new(&shell.program);
    builder
      .args(&shell.args)
      .current_dir(self.inner.config.cwd.read().unwrap_or_else(|e| e.into_inner()).clone())
      .envs(&self.inner.config.env)
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .stdin(if interactive || data.is_some() { Stdio::piped() } else { Stdio::null() })
      .kill_on_drop(true);
    builder.env("WISH_TASK_ID", &id);
    platform::configure_command(&mut builder, &shell.program, &command);
    // Serialize spawn/registration with shutdown so a successfully spawned process is never missed.
    let mut registry = self.lock_registry()?;
    if registry.closed {
      return Err(TaskError::Closed);
    }
    if control.is_cancelled() {
      return Err(TaskError::Interrupted);
    }
    if registry
      .executions
      .values()
      .filter(|task| task.get_snapshot().status == Status::Running)
      .count()
      >= 16
    {
      return Err(TaskError::InvalidArguments("session running task limit (16) reached".into()));
    }
    let mut child = builder.spawn()?;
    let group = ProcessTree::attach(&child)?;
    let (kills, receiver) = mpsc::unbounded_channel();
    let (state, _) = watch::channel(Snapshot {
      edits: Arc::new(
        edits.iter().map(|edit| EditResult::Pending { path: edit.path.clone() }).collect(),
      ),
      ..Snapshot::default()
    });
    let (initial_input_done, _) = watch::channel(data.is_none());
    let execution = Arc::new(Execution {
      id: id.clone(),
      command: command.clone(),
      started_at: (timestamp / 1_000_000) as u64,
      background: watch::channel(false).0,
      notification_claimed: false.into(),
      record_lock: tokio::sync::Mutex::new(()),
      output_path,
      stdin: tokio::sync::Mutex::new(child.stdin.take()),
      state,
      initial_input_done,
      kills,
      write_timeout: self.inner.config.stdin_write_timeout,
    });
    registry.executions.insert(id, execution.clone());
    let initial_input = data.map(|bytes| {
      let execution = execution.clone();
      tokio::spawn(async move {
        let result =
          execution.write_bytes(&bytes, !interactive, &ExecutionControl::new(), false).await;
        if !interactive {
          execution.stdin.lock().await.take();
        }
        execution.state.send_modify(|snapshot| {
          snapshot.initial_input_bytes = result.accepted_bytes;
          snapshot.initial_input_error = result
            .error
            .or_else(|| result.timed_out.then(|| "initial stdin write timed out".into()));
        });
        execution.initial_input_done.send_replace(true);
      })
    });
    tokio::spawn(process::supervise(
      child,
      group,
      execution.clone(),
      receiver,
      self.inner.config.kill_grace,
      (initial_input, file),
      edits,
    ));
    Ok(execution)
  }
  async fn start(
    &self,
    command: String,
    timeout: Duration,
    data: Option<Vec<u8>>,
    interactive: bool,
    control: &ExecutionControl,
  ) -> Result<Value, TaskError> {
    let deadline = resolve_deadline(timeout)?;
    let execution = self.spawn(command, data, interactive, Vec::new(), control).await?;
    let mut foreground = Foreground { execution: execution.clone(), returned: false };
    let reason = loop {
      if control.is_cancelled() {
        execution.terminate(KillMode::Graceful).await?;
        break "interrupted";
      }
      if *execution.background.borrow() {
        break "backgrounded";
      }
      let snapshot = execution.get_snapshot();
      let output_bytes = match tokio::fs::metadata(&execution.output_path).await {
        Ok(metadata) => metadata.len(),
        Err(error) => {
          execution.terminate(KillMode::Force).await?;
          return Err(TaskError::Unknown(format!(
            "execution {} output unavailable: {error}",
            execution.id
          )));
        }
      };
      if snapshot.status == Status::Unknown {
        return Err(TaskError::Unknown(snapshot.error.unwrap_or_default()));
      }
      if snapshot.status != Status::Running {
        break if output_bytes > self.inner.config.inline_bytes as u64 {
          "output_threshold"
        } else {
          "exited"
        };
      }
      if output_bytes > self.inner.config.inline_bytes as u64 {
        break "output_exceeded_inline";
      }
      if tokio::time::Instant::now() >= deadline {
        break "soft_timeout";
      }
      wait_tick(deadline, control).await;
    };
    let mut output =
      read_output(&execution, 0, self.inner.config.inline_bytes, DataEncoding::Utf8).await?;
    // Cancellation during the final file read still belongs to the foreground invocation.
    if control.is_cancelled() && execution.get_snapshot().status == Status::Running {
      execution.terminate(KillMode::Graceful).await?;
      output =
        read_output(&execution, 0, self.inner.config.inline_bytes, DataEncoding::Utf8).await?;
    }
    output["return_reason"] = if control.is_cancelled() { "interrupted" } else { reason }.into();
    if execution.get_snapshot().status == Status::Running {
      execution.background.send_replace(true);
    }
    execution.persist().await?;
    foreground.returned = true;
    Ok(output)
  }
  /// Run an editing command to exit, so the diffs it returns are final. Without `check_diff` the
  async fn poll(
    &self,
    id: &str,
    offset: u64,
    max_bytes: usize,
    wait: Duration,
    encoding: DataEncoding,
    control: &ExecutionControl,
  ) -> Result<Value, TaskError> {
    if max_bytes > 1024 * 1024 || wait > Duration::from_secs(60) {
      return Err(TaskError::InvalidArguments("poll permits at most 1 MiB and 60 seconds".into()));
    }
    let execution = self.get_execution(id)?;
    let deadline = resolve_deadline(wait)?;
    loop {
      if control.is_cancelled() {
        return Err(TaskError::Interrupted);
      }
      if execution.get_snapshot().status != Status::Running
        || tokio::fs::metadata(&execution.output_path).await?.len() > offset
        || tokio::time::Instant::now() >= deadline
      {
        return read_output(&execution, offset, max_bytes, encoding).await;
      }
      wait_tick(deadline, control).await;
    }
  }
}
/// Waits one polling tick, ended early by `deadline` or by cancellation.
async fn wait_tick(deadline: tokio::time::Instant, control: &ExecutionControl) {
  let tick = deadline.min(tokio::time::Instant::now() + Duration::from_millis(100));
  control.run_until_cancelled(tokio::time::sleep_until(tick)).await;
}
fn resolve_deadline(duration: Duration) -> Result<tokio::time::Instant, TaskError> {
  tokio::time::Instant::now()
    .checked_add(duration)
    .ok_or_else(|| TaskError::InvalidArguments("wait duration overflows the clock".into()))
}
async fn read_output(
  execution: &Execution,
  offset: u64,
  max_bytes: usize,
  encoding: DataEncoding,
) -> Result<Value, TaskError> {
  if max_bytes > 1024 * 1024 {
    return Err(TaskError::InvalidArguments("output read exceeds 1 MiB".into()));
  }
  let mut file = tokio::fs::File::open(&execution.output_path).await?;
  file.seek(io::SeekFrom::Start(offset)).await?;
  let mut bytes = Vec::new();
  (&mut file).take(max_bytes as u64).read_to_end(&mut bytes).await?;
  let snapshot = execution.get_snapshot();
  let total = file.metadata().await?.len();
  let next_offset = offset + bytes.len() as u64;
  let (text, lossy) = match encoding {
    DataEncoding::Utf8 => platform::decode_output(&bytes),
    DataEncoding::Base64 => (base64::engine::general_purpose::STANDARD.encode(&bytes), false),
  };
  let mut output = json!({"execution_id":execution.id, "process":snapshot, "output_path":execution.output_path,
    "output_bytes":total, "start_offset":offset, "next_offset":next_offset, "eof":snapshot.status != Status::Running && next_offset >= total,
    "encoding":encoding, "text":text, "lossy":lossy});
  if next_offset < total {
    output["output_notice"] = "Output limited. Read only the needed range or search output_path. Use poll with next_offset to continue; avoid reading the entire file or repeating broad commands.".into();
  }
  Ok(output)
}
