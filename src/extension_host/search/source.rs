//! Host routing only; each extension owns its endpoint and authentication declaration.
use super::SearchProtocol;
use crate::protocol::endpoint::Source;
pub(super) fn find_source(protocol: SearchProtocol) -> Source {
  match protocol {
    SearchProtocol::ZaiSearch => crate::extensions::zai::sources::search(protocol.get_id()),
    SearchProtocol::CodexAlphaSearch => {
      crate::extensions::openai::sources::search(protocol.get_id())
    }
    SearchProtocol::TavilySearch => crate::extensions::tavily::sources::search(protocol.get_id()),
    SearchProtocol::TavilyKeylessSearch => {
      crate::extensions::tavily::sources::keyless_search(protocol.get_id())
    }
    SearchProtocol::ExaSearch => crate::extensions::exa::sources::search(protocol.get_id()),
    SearchProtocol::PerplexitySearch => {
      crate::extensions::perplexity::sources::search(protocol.get_id())
    }
    SearchProtocol::BraveSearch => crate::extensions::brave::sources::search(protocol.get_id()),
    SearchProtocol::SerperSearch => crate::extensions::serper::sources::search(protocol.get_id()),
    SearchProtocol::JinaSearch => crate::extensions::jina::sources::search(protocol.get_id()),
    SearchProtocol::BochaSearch => crate::extensions::bocha::sources::search(protocol.get_id()),
    SearchProtocol::MetasoSearch => crate::extensions::metaso::sources::search(protocol.get_id()),
    SearchProtocol::KimiCodeSearch => crate::extensions::kimi::sources::search(protocol.get_id()),
    SearchProtocol::MinimaxCodingPlanSearch => {
      crate::extensions::minimax::sources::search(protocol.get_id())
    }
    SearchProtocol::DeepseekSearch => {
      crate::extensions::deepseek::sources::search(protocol.get_id())
    }
    SearchProtocol::SearxngSearch => crate::extensions::searxng::sources::search(protocol.get_id()),
  }
}
