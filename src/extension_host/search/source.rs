use super::SearchProtocol;
use crate::protocol::endpoint::Source;
pub(super) fn find_source(protocol: SearchProtocol) -> Source {
  (protocol.0.source)(protocol.get_id())
}
