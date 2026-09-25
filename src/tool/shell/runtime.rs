use super::{
  DataEncoding, KillMode, ShellConfig, ShellError, ShellOperation,
  edit::{EditCapture, EditResult},
  operation,
  platform::{self, ProcessTree},
  process::{self, Execution, Snapshot, Status},
};
use crate::executor::{
  ExecutionControl,
  tool::{ToolCall, ToolExecutor, ToolOutcome},
};
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
/// lock only protects handle insertion/lookup; no process or asynchronous I/O runs under that lock.
#[derive(Clone)]
pub struct ShellTool {
  inner: Arc<Runtime>,
}
struct Runtime {
  config: ShellConfig,
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
    if !self.returned {
      let _ = self.execution.kills.send(KillMode::Force);
    }
  }
}

impl ShellTool {
  pub async fn new(mut config: ShellConfig) -> Result<Self, ShellError> {
    if !cfg!(any(unix, windows)) {
      return Err(
        io::Error::new(io::ErrorKind::Unsupported, "shell supervision requires Unix or Windows")
          .into(),
      );
    }
    config.cwd = tokio::fs::canonicalize(&config.cwd).await?;
    if !tokio::fs::metadata(&config.cwd).await?.is_dir() {
      return Err(ShellError::InvalidArguments("cwd must be a directory".into()));
    }
    for duration in [config.soft_timeout, config.kill_grace, config.stdin_write_timeout] {
      resolve_deadline(duration)?;
    }
    tokio::fs::create_dir_all(&config.capture_dir).await?;
    config.capture_dir = tokio::fs::canonicalize(&config.capture_dir).await?;
    Ok(Self { inner: Arc::new(Runtime { config, registry: Mutex::new(Registry::default()) }) })
  }
  pub fn get_specifications(&self) -> Vec<crate::protocol::Tool> {
    operation::build_specifications()
  }
  fn lock_registry(&self) -> Result<std::sync::MutexGuard<'_, Registry>, ShellError> {
    self
      .inner
      .registry
      .lock()
      .map_err(|_| ShellError::Unknown("shell registry lock poisoned".into()))
  }
  fn get_execution(&self, id: &str) -> Result<Arc<Execution>, ShellError> {
    self
      .lock_registry()?
      .executions
      .get(id)
      .cloned()
      .ok_or_else(|| ShellError::ExecutionNotFound(id.into()))
  }
  /// Stop all owned process groups and await reaping. Captures and completed handles are retained.
  pub async fn shutdown(&self) -> Result<(), ShellError> {
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
  /// Wait for an execution returned by start and read its final status without inline output.
  /// Applications can enqueue this result as a background completion notification.
  pub async fn wait_for_completion(&self, id: &str) -> Result<Value, ShellError> {
    let execution = self.get_execution(id)?;
    // Unknown is a terminal outcome too; retain the diagnostic in the returned snapshot.
    let _ = execution.wait_for_exit().await;
    read_output(&execution, 0, 0, DataEncoding::Utf8).await
  }
  pub async fn run(&self, operation: ShellOperation, control: &ExecutionControl) -> ToolOutcome {
    if control.is_cancelled() {
      return ToolOutcome::Cancelled;
    }
    let result = match operation {
      ShellOperation::Start { command, timeout, data, encoding, interactive } => {
        let timeout = match timeout {
          None | Some(-1.0) => Ok(self.inner.config.soft_timeout),
          Some(seconds) => Duration::try_from_secs_f64(seconds)
            .map_err(|error| ShellError::InvalidArguments(error.to_string())),
        };
        match (timeout, data.map(|data| encoding.decode(data)).transpose()) {
          (Ok(timeout), Ok(data)) => self.start(command, timeout, data, interactive, control).await,
          (Err(error), _) | (_, Err(error)) => Err(error),
        }
      }
      ShellOperation::Edit { command, diff, check_diff } => {
        return self.edit(command, diff, check_diff, control).await.unwrap_or_else(failure);
      }
      ShellOperation::Poll { execution_id, offset, max_bytes, wait_ms, encoding } => {
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
      ShellOperation::Write { execution_id, data, encoding, close } => {
        match (self.get_execution(&execution_id), encoding.decode(data)) {
          (Ok(execution), Ok(bytes)) => {
            let result = execution.write_bytes(&bytes, close, control, true).await;
            Ok(json!({"execution_id": execution_id, "write": result}))
          }
          (Err(error), _) | (_, Err(error)) => Err(error),
        }
      }
      ShellOperation::Kill { execution_id, mode } => match self.get_execution(&execution_id) {
        Ok(execution) => execution
          .terminate(mode)
          .await
          .map(|snapshot| json!({"execution_id": execution_id, "process": snapshot})),
        Err(error) => Err(error),
      },
    };
    result.map(ToolOutcome::Success).unwrap_or_else(failure)
  }
  async fn spawn(
    &self,
    command: String,
    data: Option<Vec<u8>>,
    interactive: bool,
    edits: Vec<EditCapture>,
    control: &ExecutionControl,
  ) -> Result<Arc<Execution>, ShellError> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let id =
      format!("{}-{timestamp}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let directory = self.inner.config.capture_dir.join(&id);
    tokio::fs::create_dir(&directory).await?;
    let output_path = directory.join("output.log");
    let file = tokio::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&output_path)
      .await?
      .into_std()
      .await;
    let shell =
      self.inner.config.command.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let mut builder = Command::new(&shell.program);
    builder
      .args(&shell.args)
      .current_dir(&self.inner.config.cwd)
      .envs(&self.inner.config.env)
      .stdout(Stdio::from(file.try_clone()?))
      .stderr(Stdio::from(file))
      .stdin(if interactive || data.is_some() { Stdio::piped() } else { Stdio::null() })
      .kill_on_drop(true);
    platform::configure_command(&mut builder, &shell.program, &command);
    // Serialize spawn/registration with shutdown so a successfully spawned process is never missed.
    let mut registry = self.lock_registry()?;
    if registry.closed {
      return Err(ShellError::Closed);
    }
    if control.is_cancelled() {
      return Err(ShellError::Interrupted);
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
      initial_input,
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
  ) -> Result<Value, ShellError> {
    let deadline = resolve_deadline(timeout)?;
    let execution = self.spawn(command, data, interactive, Vec::new(), control).await?;
    let mut foreground = Foreground { execution: execution.clone(), returned: false };
    let reason = loop {
      if control.is_cancelled() {
        execution.terminate(KillMode::Graceful).await?;
        break "interrupted";
      }
      let snapshot = execution.get_snapshot();
      let output_bytes = match tokio::fs::metadata(&execution.output_path).await {
        Ok(metadata) => metadata.len(),
        Err(error) => {
          execution.terminate(KillMode::Force).await?;
          return Err(ShellError::Unknown(format!(
            "execution {} output unavailable: {error}",
            execution.id
          )));
        }
      };
      if snapshot.status == Status::Unknown {
        return Err(ShellError::Unknown(snapshot.error.unwrap_or_default()));
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
      tokio::select! {
        _ = control.wait_for_cancellation() => {},
        _ = tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + Duration::from_millis(20))) => {},
      }
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
    foreground.returned = true;
    Ok(output)
  }
  /// Run an editing command to exit, so the diffs it returns are final. Without `check_diff` the
  /// model sees only whether each file changed; the diffs stay on the result for the application.
  async fn edit(
    &self,
    command: String,
    paths: Vec<std::path::PathBuf>,
    check_diff: bool,
    control: &ExecutionControl,
  ) -> Result<ToolOutcome, ShellError> {
    if paths.is_empty() {
      return Err(ShellError::InvalidArguments("diff needs at least one file path".into()));
    }
    let mut edits = Vec::with_capacity(paths.len());
    for path in paths {
      edits.push(EditCapture::capture(path).await?);
    }
    let execution = self.spawn(command, None, false, edits, control).await?;
    let mut foreground = Foreground { execution: execution.clone(), returned: false };
    let (snapshot, reason) = tokio::select! {
      snapshot = execution.wait_for_exit() => (snapshot?, "exited"),
      _ = control.wait_for_cancellation() => {
        (execution.terminate(KillMode::Graceful).await?, "interrupted")
      }
    };
    foreground.returned = true;
    let mut output =
      read_output(&execution, 0, self.inner.config.inline_bytes, DataEncoding::Utf8).await?;
    output["return_reason"] = reason.into();
    let edits = json!(*snapshot.edits);
    if check_diff {
      output["edits"] = edits;
      return Ok(ToolOutcome::Success(output));
    }
    let mut summaries = edits.clone();
    for summary in summaries.as_array_mut().into_iter().flatten() {
      if let Some(summary) = summary.as_object_mut() {
        summary.remove("diff");
      }
    }
    output["edits"] = summaries;
    Ok(ToolOutcome::SuccessWithMetadata { output, metadata: json!({"edits": edits}) })
  }
  async fn poll(
    &self,
    id: &str,
    offset: u64,
    max_bytes: usize,
    wait: Duration,
    encoding: DataEncoding,
    control: &ExecutionControl,
  ) -> Result<Value, ShellError> {
    let execution = self.get_execution(id)?;
    let deadline = resolve_deadline(wait)?;
    loop {
      if control.is_cancelled() {
        return Err(ShellError::Interrupted);
      }
      if execution.get_snapshot().status != Status::Running
        || tokio::fs::metadata(&execution.output_path).await?.len() > offset
        || tokio::time::Instant::now() >= deadline
      {
        return read_output(&execution, offset, max_bytes, encoding).await;
      }
      tokio::select! {
        _ = control.wait_for_cancellation() => {},
        _ = tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + Duration::from_millis(20))) => {},
      }
    }
  }
}
impl ToolExecutor for ShellTool {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    match ShellOperation::parse_call(&call.name, call.arguments.clone()) {
      Ok(operation) => self.run(operation, control).await,
      Err(error) => ToolOutcome::Failed(error.to_string()),
    }
  }
}
fn resolve_deadline(duration: Duration) -> Result<tokio::time::Instant, ShellError> {
  tokio::time::Instant::now()
    .checked_add(duration)
    .ok_or_else(|| ShellError::InvalidArguments("wait duration overflows the clock".into()))
}
async fn read_output(
  execution: &Execution,
  offset: u64,
  max_bytes: usize,
  encoding: DataEncoding,
) -> Result<Value, ShellError> {
  let mut file = tokio::fs::File::open(&execution.output_path).await?;
  file.seek(io::SeekFrom::Start(offset)).await?;
  let mut bytes = Vec::new();
  (&mut file).take(max_bytes as u64).read_to_end(&mut bytes).await?;
  let snapshot = execution.get_snapshot();
  let total = file.metadata().await?.len();
  let next_offset = offset + bytes.len() as u64;
  let (text, lossy) = match encoding {
    DataEncoding::Utf8 => {
      let text = String::from_utf8_lossy(&bytes);
      let lossy = matches!(text, std::borrow::Cow::Owned(_));
      (text.into_owned(), lossy)
    }
    DataEncoding::Base64 => (base64::engine::general_purpose::STANDARD.encode(&bytes), false),
  };
  let mut output = json!({"execution_id":execution.id, "process":snapshot, "output_path":execution.output_path,
    "output_bytes":total, "next_offset":next_offset, "eof":snapshot.status != Status::Running && next_offset >= total,
    "encoding":encoding, "text":text, "lossy":lossy});
  if next_offset < total {
    output["output_notice"] = "Output limited. Read only the needed range or search output_path. Use poll with next_offset to continue; avoid reading the entire file or repeating broad commands.".into();
  }
  Ok(output)
}

fn failure(error: ShellError) -> ToolOutcome {
  match error {
    ShellError::Interrupted => ToolOutcome::Cancelled,
    ShellError::Unknown(message) => ToolOutcome::Unknown(message),
    error => ToolOutcome::Failed(error.to_string()),
  }
}
