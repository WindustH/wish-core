use super::*;
use crate::extension_host::media::video::VideoMode;

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
#[derive(Clone, Serialize, Deserialize)]
struct Job {
  id: String,
  kind: MediaKind,
  #[serde(default)]
  mode: VideoMode,
  provider: String,
  service: MediaProvider,
  phase: Phase,
  task_id: Option<String>,
  asset_url: Option<String>,
  #[serde(default)]
  subtitle_url: Option<String>,
  #[serde(default)]
  artifact_mime: Option<String>,
  artifacts: Vec<blobs::Blob>,
  #[serde(default)]
  timestamps: Option<blobs::Blob>,
  created_at: u64,
  updated_at: u64,
  error: Option<String>,
  #[serde(default)]
  next_poll_at: u64,
}
impl Job {
  fn view(&self) -> Value {
    json!({"job_id":self.id,"kind":self.kind,"mode":self.mode,"provider":self.provider,"model":self.service.model,"status":self.phase,"artifacts":self.artifacts,"timestamps":self.timestamps,"created_at":self.created_at,"updated_at":self.updated_at,"error":self.error,"next_action":if matches!(self.phase,Phase::Running|Phase::Downloading) { Some("status") } else { None },"poll_after_seconds":if self.phase==Phase::Running {Some(5)} else {None}})
  }
}
pub use crate::extensions::media_runtime::MediaRequest as Request;
/// Host-enforced extension scope. Empty presets is reserved for user-facing job recovery.
pub struct Scope {
  pub video: bool,
  pub presets: &'static [&'static str],
}
impl Scope {
  fn accepts(&self, preset: &str) -> bool {
    self.presets.is_empty() || self.presets.contains(&preset)
  }
}
impl MediaHub {
  pub async fn recover(
    &self,
    job_id: String,
    directory: &Path,
    accounts: &AccountRegistry,
    video: bool,
  ) -> Result<Value, String> {
    let request =
      Request { action: Some("status".into()), job_id: Some(job_id), ..Default::default() };
    self.execute_scoped(request, "", directory, accounts, Scope { video, presets: &[] }).await
  }
  pub async fn execute_scoped(
    &self,
    request: Request,
    call_id: &str,
    directory: &Path,
    accounts: &AccountRegistry,
    scope: Scope,
  ) -> Result<Value, String> {
    let video = scope.video;
    let action = request.action.as_deref().unwrap_or("run");
    if action == "run" && scope.presets.is_empty() {
      return Err("generation requires an extension service scope".into());
    }
    if !matches!(action, "run" | "status" | "list_jobs") {
      return Err("invalid media action".into());
    }
    if action == "run" && request.job_id.is_some() {
      return Err("run does not accept job_id; use status to resume a job".into());
    }
    let jobs = directory.join(".media-jobs");
    if action == "list_jobs" {
      let mut rows = Vec::new();
      let mut unreadable_jobs = Vec::new();
      match tokio::fs::read_dir(&jobs).await {
        Ok(mut entries) => {
          while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            if entry.path().extension().is_some_and(|s| s == "json") {
              let job: Job = match read_job(&entry.path()).await {
                Ok(job) => job,
                Err(_) => {
                  unreadable_jobs.push(entry.file_name().to_string_lossy().into_owned());
                  continue;
                }
              };
              if (job.kind == MediaKind::Video) == video && scope.accepts(&job.service.preset) {
                rows.push(job.view());
              }
            }
          }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
      }
      rows.sort_by_key(|v| std::cmp::Reverse(v["created_at"].as_u64().unwrap_or_default()));
      return Ok(json!({"jobs":rows,"unreadable_jobs":unreadable_jobs}));
    }
    let id = if action == "run" {
      blobs::blob_id(format!("{video}:{call_id}").as_bytes())
    } else {
      request.job_id.clone().ok_or("status requires job_id")?
    };
    if !blobs::is_blob_id(&id) {
      return Err("invalid job_id".into());
    }
    let path = jobs.join(format!("{id}.json"));
    let _lock = self.locks.lock(&path).await;
    let state = self.state.read().unwrap().clone();
    if action == "status" || tokio::fs::try_exists(&path).await.map_err(|e| e.to_string())? {
      let mut job: Job = read_job(&path).await?;
      if (job.kind == MediaKind::Video) != video || !scope.accepts(&job.service.preset) {
        return Err("job belongs to a different tool".into());
      }
      self.advance(&mut job, &path, directory, accounts, &state).await?;
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
    if !video
      && (!request.inputs.is_empty() || request.mode.is_some() || request.modify_mode.is_some())
    {
      return Err("video inputs/mode are not supported by audio_generate".into());
    }
    let mode = request.mode.unwrap_or_else(|| {
      use crate::extension_host::media::video::InputRole::*;
      if request.inputs.is_empty() {
        VideoMode::Text
      } else if request.inputs.iter().any(|i| i.role == SourceVideo) {
        VideoMode::Edit
      } else if request.inputs.iter().any(|i| {
        matches!(i.role, ReferenceImage | ReferenceVideo | ReferenceAudio | ReferenceVoice)
      }) {
        VideoMode::Reference
      } else if request.inputs.iter().any(|i| matches!(i.role, FirstFrame | LastFrame | Keyframe)) {
        VideoMode::Frames
      } else {
        VideoMode::Reference
      }
    });
    let mut reusable_blobs = std::collections::HashSet::new();
    if video && request.provider.is_none() {
      for (service_id, service) in &state.config.providers {
        if !service.enabled || !scope.accepts(&service.preset) {
          continue;
        }
        for input in &request.inputs {
          if let Some(id) = &input.blob_id
            && blobs::is_blob_id(id)
            && source_url(directory, id, service)
              .await?
              .is_some_and(|url| url.starts_with("https://"))
          {
            reusable_blobs.insert((service_id.clone(), id.clone()));
          }
        }
      }
    }
    let selected = request
      .provider
      .as_deref()
      .or_else(|| {
        state
          .config
          .order
          .iter()
          .find(|service_id| {
            state.config.providers.get(*service_id).is_some_and(|p| {
              p.enabled
                && scope.accepts(&p.preset)
                && preset(&p.preset).is_ok_and(|s| {
                  s.kind == kind
                    && super::preflight::prepare(&request, p, s.protocol, kind, mode).is_ok()
                    && accounts
                      .get(&format!("account:{}", p.credential_id))
                      .is_some_and(|a| s.protocol.credentials_ready(a.client.get_credentials()))
                    && (!video
                      || (s.protocol.video_capabilities(&p.model).is_some_and(|caps| {
                        request.inputs.iter().all(|input| {
                          input.blob_id.as_ref().is_none_or(|id| {
                            caps.blob_inputs.iter().any(|kind| kind == input.role.media_type())
                              || reusable_blobs.contains(&((*service_id).clone(), id.clone()))
                          })
                        })
                      })))
                })
            })
          })
          .map(String::as_str)
      })
      .ok_or("no available provider supports this media kind, mode, inputs and options")?;
    let service = state
      .config
      .providers
      .get(selected)
      .filter(|p| p.enabled && scope.accepts(&p.preset))
      .ok_or("media provider is unknown or disabled")?
      .clone();
    let spec = preset(&service.preset).map_err(|e| e.to_string())?;
    if spec.kind != kind {
      return Err("selected provider does not support this media kind".into());
    }
    let (options, mut route, mut draft) =
      super::preflight::prepare(&request, &service, spec.protocol, kind, mode)?;
    let prompt = request.prompt.as_deref().unwrap_or("");
    let account = accounts
      .get(&format!("account:{}", service.credential_id))
      .ok_or("media account unavailable")?;
    let credentials = account.client.get_credentials();
    if !spec.protocol.credentials_ready(credentials) {
      return Err("media account lacks required credentials".into());
    }
    let _permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
    let timed_audio = spec.protocol.0.buffered_audio(&options);
    let buffered_audio =
      timed_audio || request.plan.as_ref().is_some_and(|plan| plan.buffered_audio());
    if video && !request.inputs.is_empty() {
      let inputs = super::inputs::resolve(
        &request.inputs,
        directory,
        spec.protocol,
        &service,
        spec,
        credentials,
        &state,
      )
      .await?;
      let protocol = spec.protocol;
      let model = service.model.clone();
      let prompt = prompt.to_owned();
      let modify_mode = request.modify_mode.clone();
      let plan = request.plan.clone();
      let defaults = service.parameters.clone();
      (route, draft) = crate::utils::blocking::cpu(move || {
        if let Some(plan) = plan {
          plan.build(&crate::extensions::media_runtime::PlanContext {
            model: &model,
            prompt: &prompt,
            defaults: &defaults,
            parameters: &options,
            inputs: &inputs,
          })
        } else {
          protocol.video_submission(
            &model,
            &prompt,
            mode,
            &options,
            &inputs,
            modify_mode.as_deref(),
          )
        }
      })
      .await
      .map_err(|e| e.to_string())?
      .map_err(|e| e.to_string())?;
    }
    let base = service.base_url.as_deref().filter(|v| !v.is_empty()).unwrap_or(&spec.base_url);
    let call = spec.protocol.call(base, &route, credentials, draft).map_err(|e| e.to_string())?;
    let transport = state.transport(service.proxy_enabled, TransferProfile::Artifact)?;
    let now = crate::utils::time::unix_seconds();
    let mut job = Job {
      id,
      kind,
      mode,
      provider: selected.into(),
      service,
      phase: Phase::Submitting,
      task_id: None,
      asset_url: None,
      subtitle_url: None,
      artifact_mime: request.plan.as_ref().and_then(|plan| plan.artifact_mime()).map(str::to_owned),
      artifacts: Vec::new(),
      timestamps: None,
      created_at: now,
      updated_at: now,
      error: None,
      next_poll_at: 0,
    };
    tokio::fs::create_dir_all(&jobs).await.map_err(|e| e.to_string())?;
    let mut entries = tokio::fs::read_dir(&jobs).await.map_err(|e| e.to_string())?;
    let mut count = 0;
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
      if entry.path().extension().is_some_and(|s| s == "json") {
        let Ok(old) = read_job::<Job>(&entry.path()).await else {
          continue;
        };
        if matches!(old.phase, Phase::Submitting | Phase::Running | Phase::Downloading) {
          count += 1;
        }
      }
    }
    if count >= 256 {
      return Err("session active media job limit (256) reached".into());
    }
    save_job(&path, &mut job).await?;
    // Once recorded as submitting, cancellation/restart cannot cause automatic resubmission.
    let result = async {
      if video {
        let reply = state
          .transport(job.service.proxy_enabled, TransferProfile::VideoSubmission)?
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
      } else if buffered_audio {
        let profile =
          if timed_audio { TransferProfile::TimedSpeech } else { TransferProfile::AudioSubmission };
        let reply = state
          .transport(job.service.proxy_enabled, profile)?
          .execute(&call)
          .await
          .map_err(|_| "audio submission connection failed; outcome unknown".to_owned())?;
        if !reply.is_success() {
          return Err(format!("audio submission HTTP {}; outcome unknown", reply.status));
        }
        let plan = request.plan.clone();
        let protocol = spec.protocol;
        let completion = crate::utils::blocking::cpu(move || {
          if let Some(plan) = plan {
            plan.decode_audio(&reply.body)
          } else {
            protocol.0.decode_audio(&reply.body)
          }
        })
        .await
        .map_err(|e| e.to_string())??;
        match completion {
          crate::extensions::media_runtime::AudioCompletion::Inline { audio, metadata } => {
            job.artifacts =
              vec![blobs::save_upload(directory, &audio).await.map_err(|e| e.to_string())?];
            job.timestamps =
              Some(blobs::save_upload(directory, &metadata).await.map_err(|e| e.to_string())?);
            job.phase = Phase::Succeeded;
          }
          crate::extensions::media_runtime::AudioCompletion::Download { audio, subtitles } => {
            job.asset_url = Some(audio);
            job.subtitle_url = subtitles;
            job.phase = Phase::Downloading;
          }
        }
      } else {
        let stream = transport
          .execute_stream(&call)
          .await
          .map_err(|_| "audio submission connection failed; outcome unknown".to_owned())?;
        job.artifacts = vec![
          save_stream(stream, directory, kind, None, spec.protocol.0.artifact_validator(None))
            .await?,
        ];
        job.phase = Phase::Succeeded;
      }
      Ok::<_, String>(())
    }
    .await;
    if let Err(error) = result {
      job.phase = Phase::OutcomeUnknown;
      job.error = Some(error);
    }
    if let Err(error) = save_job(&path, &mut job).await {
      job.phase = Phase::OutcomeUnknown;
      job.error = Some(format!(
        "Submission may have completed, but recording its result failed: {error}. Do not resubmit; reconcile job {}.",
        job.id
      ));
    }
    Ok(job.view())
  }
  async fn advance(
    &self,
    job: &mut Job,
    path: &Path,
    directory: &Path,
    accounts: &AccountRegistry,
    state: &State,
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
    let now = crate::utils::time::unix_seconds();
    if now < job.next_poll_at {
      return Ok(());
    }
    job.next_poll_at = now + 5;
    save_job(path, job).await?;
    let transport = state.transport(job.service.proxy_enabled, TransferProfile::Artifact)?;
    if job.phase == Phase::Running
      || (job.phase == Phase::Downloading && job.error.is_some() && job.task_id.is_some())
    {
      let _status = self.status_permits.acquire().await.map_err(|e| e.to_string())?;
      let spec = preset(&job.service.preset).map_err(|e| e.to_string())?;
      let credentials = accounts
        .get(&format!("account:{}", job.service.credential_id))
        .ok_or("media account unavailable; restore it to continue the existing job")?
        .client
        .get_credentials();
      if !spec.protocol.credentials_ready(credentials) {
        return Err("media account lacks required credentials".into());
      }
      let base =
        job.service.base_url.as_deref().filter(|v| !v.is_empty()).unwrap_or(&spec.base_url);
      let task = job.task_id.as_deref().ok_or("missing upstream task ID")?;
      let (route, draft) = spec.protocol.status_request(task).map_err(|e| e.to_string())?;
      let call = spec.protocol.call(base, &route, credentials, draft).map_err(|e| e.to_string())?;
      let reply = state
        .transport(job.service.proxy_enabled, TransferProfile::Status)?
        .execute(&call)
        .await
        .map_err(|_| "task query connection failed; retry status, not generation".to_owned())?;
      if !reply.is_success() {
        return Err(format!("task query HTTP {}; retry status, not generation", reply.status));
      }
      let value: Value = serde_json::from_slice(&reply.body).map_err(|e| e.to_string())?;
      use crate::extension_host::media::VideoStatus;
      match spec.protocol.decode_status(&value).map_err(|e| e.to_string())? {
        VideoStatus::Complete(url) => {
          job.asset_url = Some(url);
          job.phase = Phase::Downloading;
        }
        VideoStatus::Failed => {
          job.phase = Phase::Failed;
          job.error = Some("upstream generation failed or was cancelled".into());
        }
        VideoStatus::Pending => {}
      }
      save_job(path, job).await?;
    }
    if job.phase == Phase::Downloading {
      let _download = self.download_permits.acquire().await.map_err(|e| e.to_string())?;
      let spec = preset(&job.service.preset).map_err(|e| e.to_string())?;
      let trusted_base = job
        .service
        .base_url
        .as_deref()
        .filter(|base| !base.is_empty())
        .unwrap_or(&spec.base_url)
        .to_owned();
      // Streaming is shared infrastructure; the extension retains artifact validation and state.
      let result = async {
        if job.artifacts.is_empty() {
          let url = job.asset_url.as_deref().ok_or("missing artifact URL")?;
          let stream =
            crate::extensions::shared::artifacts::open_stream(&transport, url, &trusted_base)
              .await?;
          job.artifacts = vec![
            save_stream(
              stream,
              directory,
              job.kind,
              job.artifact_mime.as_deref(),
              spec.protocol.0.artifact_validator(job.artifact_mime.as_deref()),
            )
            .await?,
          ];
          // Persist audio independently so a subtitle failure/restart never repeats its download.
          save_job(path, job).await?;
        }
        if let Some(url) = job.subtitle_url.as_deref().filter(|_| job.timestamps.is_none()) {
          let client = state.transport(job.service.proxy_enabled, TransferProfile::Status)?;
          let bytes =
            crate::extensions::shared::artifacts::download(&client, url, "", &trusted_base).await?;
          let protocol = spec.protocol;
          let bytes = crate::utils::blocking::cpu(move || {
            protocol.0.validate_subtitles(&bytes)?;
            Ok::<_, String>(bytes)
          })
          .await
          .map_err(|e| e.to_string())??;
          job.timestamps =
            Some(blobs::save_upload(directory, &bytes).await.map_err(|e| e.to_string())?);
        }
        Ok::<_, String>(())
      }
      .await;
      match result {
        Ok(()) => {
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
async fn save_stream<S: ReplyStream>(
  stream: S,
  directory: &Path,
  kind: MediaKind,
  expected_mime: Option<&str>,
  validator: Option<media::Validator>,
) -> Result<blobs::Blob, String> {
  if !stream.is_success() {
    return Err(format!("media transfer HTTP {}", stream.get_status()));
  }
  let chunks = Box::pin(futures_util::stream::try_unfold(stream, |mut stream| async move {
    Ok::<_, crate::protocol::error::Error>(stream.next().await?.map(|chunk| (chunk, stream)))
  }));
  media::save_validated(
    directory,
    chunks,
    expected_mime.unwrap_or(kind.file_kind()),
    if kind == MediaKind::Video { 512 * 1024 * 1024 } else { 128 * 1024 * 1024 },
    validator,
  )
  .await
  .map_err(|e| e.to_string())
}
async fn save_job(path: &Path, job: &mut Job) -> Result<(), String> {
  job.updated_at = crate::utils::time::unix_seconds();
  crate::extensions::shared::jobs::write(path, job.clone()).await
}
async fn read_job<T: serde::de::DeserializeOwned + Send + 'static>(
  path: &Path,
) -> Result<T, String> {
  crate::extensions::shared::jobs::read(path).await
}

/// Use only successful jobs owned by this session; never accept arbitrary upstream task IDs.
pub(super) async fn source_task(
  directory: &Path,
  id: &str,
  service: &MediaProvider,
) -> Result<String, String> {
  if !blobs::is_blob_id(id) {
    return Err("invalid source job_id".into());
  }
  let job: Job = read_job(&directory.join(".media-jobs").join(format!("{id}.json"))).await?;
  if job.phase != Phase::Succeeded
    || job.kind != MediaKind::Video
    || job.service.preset != service.preset
    || job.service.credential_id != service.credential_id
    || job.service.base_url != service.base_url
    || job.service.model != service.model
  {
    return Err("extension requires a successful video job from the same service/account".into());
  }
  job.task_id.ok_or("source job has no upstream task ID".into())
}
pub(super) async fn source_url(
  directory: &Path,
  blob: &str,
  service: &MediaProvider,
) -> Result<Option<String>, String> {
  let mut entries = match tokio::fs::read_dir(directory.join(".media-jobs")).await {
    Ok(v) => v,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(e) => return Err(e.to_string()),
  };
  while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
    if entry.path().extension().is_some_and(|s| s == "json") {
      let Ok(job) = read_job::<Job>(&entry.path()).await else { continue };
      if job.phase == Phase::Succeeded
        && job.service.preset == service.preset
        && job.service.credential_id == service.credential_id
        && job.service.base_url == service.base_url
        && job.service.model == service.model
        && job.artifacts.iter().any(|b| b.id == blob)
      {
        return Ok(job.asset_url);
      }
    }
  }
  Ok(None)
}
