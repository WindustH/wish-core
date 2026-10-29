//! Infrastructure reused by extensions; no application or tool orchestration.
mod artifact_destinations;
pub(crate) mod artifacts;
pub mod image_pixels;
pub mod images;
pub(crate) mod jobs;
pub mod multipart;
pub mod parameters;
pub(crate) mod polling;
pub mod read;
pub(crate) mod read_budget;
pub(crate) mod urls;
pub mod video;
pub mod web_fetch;
pub mod web_search;

pub mod read_codecs;
