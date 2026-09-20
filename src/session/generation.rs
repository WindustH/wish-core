use super::SessionConfig;
use crate::storage::ListId;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GenerationId(pub usize);

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationStatus {
  Active,
  Standby,
  Sealed,
}

/// Ordered direct entry references; never references another generation's content.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Generation {
  pub id: GenerationId,
  pub status: GenerationStatus,
  pub entries: ListId,
  pub config: SessionConfig,
  /// The stable source prefix captured when standby was prepared.
  pub(crate) source: Option<(GenerationId, u64)>,
}
