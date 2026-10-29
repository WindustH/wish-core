//! xAI owns operation selection and construction of its asset bindings.
use super::{
  video::{Asset, Request},
  video_options::VideoOptions,
};
use crate::extensions::shared::video::{InputRole, VideoInput, VideoMode};

pub enum Operation {
  Submit(Box<Submission>),
  Status(String),
  ListJobs,
}
pub struct Submission {
  pub prompt: String,
  pub service: Option<String>,
  pub mode: VideoMode,
  pub inputs: Vec<VideoInput>,
  pub options: VideoOptions,
}
fn input(asset: Asset, role: InputRole) -> VideoInput {
  let (blob_id, url) = match asset {
    Asset::Blob(id) => (Some(id), None),
    Asset::Url(url) => (None, Some(url)),
  };
  VideoInput { role, blob_id, url, job_id: None, timestamp_s: None, voice_id: None }
}
impl Request {
  pub fn prepare(self) -> Operation {
    match self {
      Self::Generate(args) => {
        let mode = if !args.reference_images.is_empty() || !args.reference_voices.is_empty() {
          VideoMode::Reference
        } else if args.image.is_some() || args.last_frame.is_some() || !args.keyframes.is_empty() {
          VideoMode::Frames
        } else {
          VideoMode::Text
        };
        let mut inputs = Vec::new();
        if let Some(image) = args.image {
          inputs.push(input(image, InputRole::FirstFrame));
        }
        if let Some(image) = args.last_frame {
          inputs.push(input(image, InputRole::LastFrame));
        }
        for image in args.reference_images {
          inputs.push(input(image, InputRole::ReferenceImage));
        }
        for frame in args.keyframes {
          let mut input = input(frame.image, InputRole::Keyframe);
          input.timestamp_s = Some(frame.timestamp_s);
          inputs.push(input);
        }
        for voice in args.reference_voices {
          inputs.push(VideoInput {
            role: InputRole::ReferenceVoice,
            voice_id: Some(voice),
            blob_id: None,
            url: None,
            job_id: None,
            timestamp_s: None,
          });
        }
        Operation::Submit(Box::new(Submission {
          prompt: args.prompt,
          service: args.service,
          mode,
          inputs,
          options: VideoOptions {
            duration: args.duration.map(f64::from),
            resolution: args.resolution,
            aspect_ratio: args.aspect_ratio,
            generate_audio: args.generate_audio,
          },
        }))
      }
      Self::Edit { prompt, video, service } => Operation::Submit(Box::new(Submission {
        prompt,
        service,
        mode: VideoMode::Edit,
        inputs: vec![input(video, InputRole::SourceVideo)],
        options: Default::default(),
      })),
      Self::Extend { prompt, video, duration, service } => {
        Operation::Submit(Box::new(Submission {
          prompt,
          service,
          mode: VideoMode::Extend,
          inputs: vec![input(video, InputRole::SourceVideo)],
          options: VideoOptions { duration: duration.map(f64::from), ..Default::default() },
        }))
      }
      Self::Status { job_id } => Operation::Status(job_id),
      Self::ListJobs {} => Operation::ListJobs,
    }
  }
}
