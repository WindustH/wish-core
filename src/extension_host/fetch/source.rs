//! Host routing only; suppliers own read endpoint declarations.
use super::FetchProtocol;
use crate::protocol::endpoint::Source;
pub(super) fn find_source(protocol: FetchProtocol, has_key: bool) -> Source {
  match protocol {
    FetchProtocol::DeepseekFetch => {
      crate::extensions::deepseek::sources::fetch(protocol.get_id(), has_key)
    }
    FetchProtocol::ZaiReader => crate::extensions::zai::sources::fetch(protocol.get_id(), has_key),
    FetchProtocol::JinaReader => {
      crate::extensions::jina::sources::fetch(protocol.get_id(), has_key)
    }
    FetchProtocol::CodexFetch => {
      crate::extensions::openai::sources::fetch(protocol.get_id(), has_key)
    }
  }
}
