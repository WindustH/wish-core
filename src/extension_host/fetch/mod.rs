//! Registry-backed read execution. Supplier logic is defined by its extension.
mod client;
mod source;
use crate::extensions::shared::read_codecs::FetchCodec;
pub use crate::extensions::shared::web_fetch::{FetchRequest, FetchResult};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::Value;
pub mod execution;
pub use execution::fetch;
#[derive(Clone, Copy)]
pub struct FetchProtocol(pub &'static FetchCodec);
impl FetchProtocol {
  pub fn get_id(self) -> &'static str {
    self.0.id
  }

  pub fn build_draft(self, request: &FetchRequest) -> Result<Draft, Error> {
    (self.0.build)(request)
  }
  pub fn parse_body(self, body: &Value) -> Result<FetchResult, Error> {
    (self.0.parse)(body)
  }
}
impl std::fmt::Debug for FetchProtocol {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.0.id)
  }
}
impl PartialEq for FetchProtocol {
  fn eq(&self, other: &Self) -> bool {
    self.0.id == other.0.id
  }
}
impl Eq for FetchProtocol {}
impl std::hash::Hash for FetchProtocol {
  fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
    self.0.id.hash(state)
  }
}
impl std::str::FromStr for FetchProtocol {
  type Err = Error;
  fn from_str(id: &str) -> Result<Self, Error> {
    crate::extensions::read_registry::fetch()
      .iter()
      .find(|codec| codec.id == id)
      .map(|codec| Self(codec))
      .ok_or_else(|| Error::Build(format!("unknown web fetch protocol: {id}")))
  }
}
impl serde::Serialize for FetchProtocol {
  fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(self.0.id)
  }
}
impl<'de> serde::Deserialize<'de> for FetchProtocol {
  fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
    let id = <String as serde::Deserialize>::deserialize(d)?;
    id.parse().map_err(serde::de::Error::custom)
  }
}
