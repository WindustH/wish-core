use super::FetchProtocol;
use crate::protocol::endpoint::Source;
pub(super) fn find_source(protocol: FetchProtocol, has_key: bool) -> Source {
  (protocol.0.source)(protocol.get_id(), has_key)
}
