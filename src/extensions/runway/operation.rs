//! Runway owns conversion from its public tool request to an executable operation.
use super::{
  Asset, Request,
  edit::{Plan, Timing},
  video_options::VideoOptions,
};
use crate::extensions::shared::video::{InputRole, VideoInput, VideoMode};

pub enum Operation {
  Submit(Box<Submission>),
  Status(String),
  ListJobs,
}
pub struct Submission {
  pub service: Option<String>,
  pub prompt: Option<String>,
  pub mode: VideoMode,
  pub inputs: Vec<VideoInput>,
  pub options: VideoOptions,
  pub plan: Plan,
}
fn input(asset: Asset, role: InputRole) -> VideoInput {
  let (blob_id, url) = match asset {
    Asset::Blob(id) => (Some(id), None),
    Asset::Url(url) => (None, Some(url)),
  };
  VideoInput { role, blob_id, url, job_id: None, timestamp_s: None, voice_id: None }
}
impl Request {
  pub fn prepare(self) -> Result<Operation, String> {
    Ok(match self {
      Self::Generate { prompt, service, image, duration, aspect_ratio, seed } => {
        let mode = if image.is_some() { VideoMode::Frames } else { VideoMode::Text };
        Operation::Submit(Box::new(Submission {
          service,
          prompt: Some(prompt),
          mode,
          inputs: image.into_iter().map(|asset| input(asset, InputRole::FirstFrame)).collect(),
          options: VideoOptions {
            duration: duration.map(f64::from),
            aspect_ratio,
            ..Default::default()
          },
          plan: Plan::Generate { seed },
        }))
      }
      Self::Edit {
        prompt,
        service,
        video,
        first_frame,
        last_frame,
        keyframes,
        seed,
        target_aspect_ratio,
        output_format,
        prores_profile,
      } => {
        if prompt.as_ref().is_some_and(|p| p.trim().is_empty()) {
          return Err("omit the edit prompt instead of supplying empty text".into());
        }
        if !keyframes.is_empty() && (first_frame.is_some() || last_frame.is_some()) {
          return Err("keyframes cannot be combined with first_frame/last_frame".into());
        }
        let mut inputs = vec![input(video, InputRole::SourceVideo)];
        let mut frames = Vec::new();
        for (image, at) in [(first_frame, 0.0), (last_frame, 1.0)] {
          if let Some(image) = image {
            inputs.push(input(image, InputRole::ReferenceImage));
            frames.push(Timing { at: Some(at), seconds: None, range: None });
          }
        }
        for frame in keyframes {
          inputs.push(input(frame.image, InputRole::ReferenceImage));
          frames.push(Timing { at: frame.at, seconds: frame.seconds, range: frame.range });
        }
        Operation::Submit(Box::new(Submission {
          service,
          prompt,
          mode: VideoMode::Edit,
          inputs,
          options: Default::default(),
          plan: Plan::Edit { seed, target_aspect_ratio, frames, output_format, prores_profile },
        }))
      }
      Self::Status { job_id } => Operation::Status(job_id),
      Self::ListJobs {} => Operation::ListJobs,
    })
  }
}
