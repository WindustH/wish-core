use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
  Submitting,
  Running,
  Downloading,
  Succeeded,
  Failed,
  OutcomeUnknown,
}
#[derive(Serialize, Deserialize)]
struct Job {
  id: String,
  kind: MediaKind,
  provider: String,
  service: MediaProvider,
  phase: Phase,
  task_id: Option<String>,
  asset_url: Option<String>,
  artifacts: Vec<blobs::Blob>,
  created_at: u64,
  updated_at: u64,
  error: Option<String>,
}
impl Job {
  fn view(&self) -> Value {
    json!({"job_id":self.id,"kind":self.kind,"provider":self.provider,"model":self.service.model,"status":self.phase,"artifacts":self.artifacts,"created_at":self.created_at,"updated_at":self.updated_at,"error":self.error,"next_action":if matches!(self.phase,Phase::Running|Phase::Downloading) { Some("status") } else { None },"poll_after_seconds":if self.phase==Phase::Running {Some(5)} else {None}})
  }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
  #[serde(default)]
  pub action: Option<String>,
  #[serde(default)]
  pub provider: Option<String>,
  #[serde(default)]
  pub prompt: Option<String>,
  #[serde(default)]
  pub kind: Option<MediaKind>,
  #[serde(default)]
  pub job_id: Option<String>,
  #[serde(default)]
  pub options: MediaOptions,
}
impl MediaHub {
  pub async fn execute(
    &self,
    request: Request,
    call_id: &str,
    directory: &Path,
    accounts: &AccountRegistry,
    video: bool,
  ) -> Result<Value, String> {
    let action = request.action.as_deref().unwrap_or("run");
    let jobs = directory.join(".media-jobs");
    if action == "list_jobs" {
      let mut rows = Vec::new();
      match tokio::fs::read_dir(&jobs).await {
        Ok(mut entries) => {
          while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            if entry.path().extension().is_some_and(|s| s == "json") {
              let job: Job = read_job(&entry.path()).await?;
              if (job.kind == MediaKind::Video) == video {
                rows.push(job.view());
              }
            }
          }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
      }
      rows.sort_by_key(|v| std::cmp::Reverse(v["created_at"].as_u64().unwrap_or_default()));
      return Ok(json!({"jobs":rows}));
    }
    let id = if action == "run" {
      blobs::blob_id(format!("{video}:{call_id}").as_bytes())
    } else {
      request.job_id.clone().ok_or("status requires job_id")?
    };
    if !blobs::is_blob_id(&id) {
      return Err("invalid job_id".into());
    }
    let _lock = self.locks[id.as_bytes()[0] as usize % self.locks.len()].lock().await;
    let _permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
    let path = jobs.join(format!("{id}.json"));
    let state = self.state.read().unwrap().clone();
    if action == "status" || tokio::fs::try_exists(&path).await.map_err(|e| e.to_string())? {
      let mut job: Job = read_job(&path).await?;
      if (job.kind == MediaKind::Video) != video {
        return Err("job belongs to a different tool".into());
      }
      self.advance(&mut job, &path, directory, accounts, &state.proxy).await?;
      return Ok(job.view());
    }
    if action != "run" {
      return Err("action must be run, status, list_jobs or list_providers".into());
    }
    if request.job_id.is_some() {
      return Err("run does not accept job_id; use status to resume a job".into());
    }
    if video && request.kind.is_some_and(|kind| kind != MediaKind::Video) {
      return Err("video_generate requires video kind".into());
    }
    let kind = if video {
      MediaKind::Video
    } else {
      request.kind.ok_or("audio generation requires kind: speech, music or sound_effect")?
    };
    if (kind == MediaKind::Video) != video || kind == MediaKind::Transcription {
      return Err("kind does not match this tool".into());
    }
    let selected = request
      .provider
      .as_deref()
      .or_else(|| {
        state
          .config
          .order
          .iter()
          .find(|id| {
            state
              .config
              .providers
              .get(*id)
              .is_some_and(|p| p.enabled && preset(&p.preset).is_ok_and(|s| s.kind == kind))
          })
          .map(String::as_str)
      })
      .ok_or("no enabled provider supports this media kind")?;
    let service = state
      .config
      .providers
      .get(selected)
      .filter(|p| p.enabled)
      .ok_or("media provider is unknown or disabled")?
      .clone();
    let spec = preset(&service.preset).map_err(|e| e.to_string())?;
    if spec.kind != kind {
      return Err("selected provider does not support this media kind".into());
    }
    let options = request.options.overlay(&service.parameters);
    let (route, draft) = spec
      .protocol
      .submission(&service.model, request.prompt.as_deref().unwrap_or(""), &options)
      .map_err(|e| e.to_string())?;
    let account = accounts
      .get(&format!("account:{}", service.credential_id))
      .ok_or("media account unavailable")?;
    let credentials = account.client.get_credentials();
    if credentials.api_key.is_empty() {
      return Err("media account has no API key".into());
    }
    let base = service.base_url.as_deref().filter(|v| !v.is_empty()).unwrap_or(&spec.base_url);
    let call = spec.protocol.call(base, &route, credentials, draft).map_err(|e| e.to_string())?;
    let transport = transport(&service, &state.proxy)?;
    let now = crate::utils::time::unix_seconds();
    let mut job = Job {
      id,
      kind,
      provider: selected.into(),
      service,
      phase: Phase::Submitting,
      task_id: None,
      asset_url: None,
      artifacts: Vec::new(),
      created_at: now,
      updated_at: now,
      error: None,
    };
    tokio::fs::create_dir_all(&jobs).await.map_err(|e| e.to_string())?;
    let mut entries = tokio::fs::read_dir(&jobs).await.map_err(|e| e.to_string())?;
    let mut count = 0;
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
      if entry.path().extension().is_some_and(|s| s == "json") {
        count += 1;
      }
    }
    if count >= 256 {
      return Err("session media job limit (256) reached".into());
    }
    save_job(&path, &mut job).await?;
    // Once recorded as submitting, cancellation/restart cannot cause automatic resubmission.
    let result = async {
      if video {
        let reply = json_transport(&job.service, &state.proxy)?
          .execute(&call)
          .await
          .map_err(|_| "submission connection failed; outcome unknown".to_owned())?;
        if !reply.is_success() {
          return Err(format!(
            "media submission HTTP {}; outcome unknown, do not automatically resubmit",
            reply.status
          ));
        }
        let value: Value = serde_json::from_slice(&reply.body)
          .map_err(|_| "invalid media submission reply".to_owned())?;
        job.task_id = Some(spec.protocol.task_id(&value)?);
        job.phase = Phase::Running;
      } else {
        let stream = transport
          .execute_stream(&call)
          .await
          .map_err(|_| "audio submission connection failed; outcome unknown".to_owned())?;
        job.artifacts = vec![save_stream(stream, directory, kind).await?];
        job.phase = Phase::Succeeded;
      }
      Ok::<_, String>(())
    }
    .await;
    if let Err(error) = result {
      job.phase = Phase::OutcomeUnknown;
      job.error = Some(error);
    }
    save_job(&path, &mut job).await?;
    Ok(job.view())
  }
  async fn advance(
    &self,
    job: &mut Job,
    path: &Path,
    directory: &Path,
    accounts: &AccountRegistry,
    proxy: &ProxyConfig,
  ) -> Result<(), String> {
    if job.phase == Phase::Submitting {
      job.phase = Phase::OutcomeUnknown;
      job.error=Some("submission was interrupted; upstream may have completed it. Do not automatically resubmit.".into());
      save_job(path, job).await?;
      return Ok(());
    }
    if matches!(job.phase, Phase::Succeeded | Phase::Failed | Phase::OutcomeUnknown) {
      return Ok(());
    }
    let transport = transport(&job.service, proxy)?;
    if job.phase == Phase::Running {
      let spec = preset(&job.service.preset).map_err(|e| e.to_string())?;
      let credentials = accounts
        .get(&format!("account:{}", job.service.credential_id))
        .ok_or("media account unavailable; restore it to continue the existing job")?
        .client
        .get_credentials();
      if credentials.api_key.is_empty() {
        return Err("media account has no API key".into());
      }
      let base =
        job.service.base_url.as_deref().filter(|v| !v.is_empty()).unwrap_or(&spec.base_url);
      let task = job.task_id.as_deref().ok_or("missing upstream task ID")?;
      let call = spec
        .protocol
        .call(
          base,
          &format!("/v2/query/video_generation/{task}"),
          credentials,
          Draft::get(Vec::new()),
        )
        .map_err(|e| e.to_string())?;
      let reply = json_transport(&job.service, proxy)?
        .execute(&call)
        .await
        .map_err(|_| "task query connection failed; retry status, not generation".to_owned())?;
      if !reply.is_success() {
        return Err(format!("task query HTTP {}; retry status, not generation", reply.status));
      }
      let value: Value = serde_json::from_slice(&reply.body).map_err(|e| e.to_string())?;
      match value["task"]["status"].as_str() {
        Some("succeeded") => {
          let url =
            value["task"]["content"]["url"].as_str().ok_or("completed task has no artifact URL")?;
          crate::protocol::image_generation::validate_url(url).map_err(|e| e.to_string())?;
          job.asset_url = Some(url.into());
          job.phase = Phase::Downloading;
        }
        Some("failed" | "cancelled" | "canceled") => {
          job.phase = Phase::Failed;
          job.error = Some("upstream generation failed or was cancelled".into());
        }
        Some("queued" | "pending" | "running" | "processing") => {}
        _ => return Err("unrecognized upstream task status; existing task was retained".into()),
      }
      save_job(path, job).await?;
    }
    if job.phase == Phase::Downloading {
      let url = job.asset_url.as_deref().ok_or("missing artifact URL")?;
      crate::protocol::image_generation::validate_url(url).map_err(|e| e.to_string())?;
      // CDN downloads carry no provider credentials; redirects are not followed by transport.
      let call =
        Call { method: Method::Get, url: url.into(), headers: Vec::new(), body: Vec::new() };
      let result = async {
        let stream = transport
          .execute_stream(&call)
          .await
          .map_err(|_| "artifact download connection failed".to_owned())?;
        save_stream(stream, directory, job.kind).await
      }
      .await;
      match result {
        Ok(blob) => {
          job.artifacts = vec![blob];
          job.phase = Phase::Succeeded;
          job.error = None;
        }
        Err(error) => {
          job.error = Some(format!("{error}; call status to retry download without regenerating"));
        }
      }
      save_job(path, job).await?;
    }
    Ok(())
  }
}
fn transport(service: &MediaProvider, proxy: &ProxyConfig) -> Result<ReqwestTransport, String> {
  ReqwestTransport::new(
    Limits {
      total: Duration::from_secs(600),
      first_byte: Duration::from_secs(300),
      max_response_bytes: 512 * 1024 * 1024,
      ..Default::default()
    },
    proxy.policy(service.proxy_enabled),
  )
  .map_err(|e| e.to_string())
}
async fn save_stream<S: ReplyStream>(
  stream: S,
  directory: &Path,
  kind: MediaKind,
) -> Result<blobs::Blob, String> {
  if !stream.is_success() {
    return Err(format!("media transfer HTTP {}", stream.get_status()));
  }
  let chunks = Box::pin(futures_util::stream::try_unfold(stream, |mut stream| async move {
    Ok::<_, crate::protocol::error::Error>(stream.next().await?.map(|chunk| (chunk, stream)))
  }));
  media::save(
    directory,
    chunks,
    kind.file_kind(),
    if kind == MediaKind::Video { 512 * 1024 * 1024 } else { 128 * 1024 * 1024 },
  )
  .await
  .map_err(|e| e.to_string())
}
async fn save_job(path: &Path, job: &mut Job) -> Result<(), String> {
  job.updated_at = crate::utils::time::unix_seconds();
  let bytes = serde_json::to_vec(job).map_err(|e| e.to_string())?;
  let temporary = path.with_extension("tmp");
  use tokio::io::AsyncWriteExt;
  let mut file = tokio::fs::File::create(&temporary).await.map_err(|e| e.to_string())?;
  file.write_all(&bytes).await.map_err(|e| e.to_string())?;
  file.sync_all().await.map_err(|e| e.to_string())?;
  drop(file);
  tokio::fs::rename(temporary, path).await.map_err(|e| e.to_string())
}
async fn read_job<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
  let bytes = tokio::fs::read(path).await.map_err(|_| "media job not found".to_owned())?;
  serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn json_transport(
  service: &MediaProvider,
  proxy: &ProxyConfig,
) -> Result<ReqwestTransport, String> {
  ReqwestTransport::new(
    Limits {
      total: Duration::from_secs(60),
      max_response_bytes: 2 * 1024 * 1024,
      ..Default::default()
    },
    proxy.policy(service.proxy_enabled),
  )
  .map_err(|e| e.to_string())
}
