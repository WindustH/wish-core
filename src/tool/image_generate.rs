//! Session image resource callback used by extension-owned tools.
pub use crate::extensions::shared::image_request::ImageRequest;
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};
pub type Backend = Arc<
  dyn Fn(
      ImageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Value, super::paid_operation::Failure>> + Send>>
    + Send
    + Sync,
>;
pub struct ImageService(pub Backend, pub super::providers::Listing);
