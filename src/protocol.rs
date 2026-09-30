//! The contract layer: what the wires say, where a call goes, and how failures are named.
//!
//! Every request is the same product: what a wire says, rendered by one of the payload trees and
//! handed over as a draft, and how it is reached - [`endpoint`], the target, the auth scheme and
//! the account's credentials that join a draft into one call. The payload trees are [`model_use`],
//! a call as our caller describes it - the conversation, the tools, the request, the response and
//! the stream - beside the per-protocol shapes that translate each of them; [`token_count`], a
//! provider's count of a request's input, made without generating anything;
//! [`upstream_compaction`], the call that asks a service to stand in for a conversation that grew
//! too long; [`account_state`], the bodies a service reports about the key that reached it;
//! [`model_list`], the pages it lists its models in; and [`web_search`], the searches a service
//! runs on the model's behalf. [`attempt`] is the contract a transport implements, [`error`]
//! with [`http_error`] is the failure vocabulary every layer speaks, and [`json_read`] is how every
//! reader takes what a service wrote.
//!
//! The protocol enums a configuration names by text share one shape - each variant has an id, and
//! the id reads back into the variant - so `text_id_enum!` below writes it once for all of them.

/// Declares a protocol enum whose variants are known by a text id.
///
/// The ids are what configuration and the presets write, so they are the one spelling that must
/// never drift: `ALL` lists the variants, `get_id` and `Display` spell them, and `FromStr` reads an
/// id back, refusing an unknown one with [`Error::Build`](error::Error::Build) worded by the
/// `unknown` format.
macro_rules! text_id_enum {
  (
    $(#[$attribute:meta])*
    $visibility:vis enum $name:ident (unknown: $unknown:literal) {
      $($(#[$variant_attribute:meta])* $variant:ident => $id:literal,)+
    }
  ) => {
    $(#[$attribute])*
    $visibility enum $name {
      $($(#[$variant_attribute])* $variant,)+
    }

    impl $name {
      /// Every variant, in declaration order.
      pub const ALL: &[$name] = &[$($name::$variant,)+];

      /// The id this variant is known by in text: what configuration, the presets and the
      /// documentation write.
      pub const fn get_id(self) -> &'static str {
        match self {
          $($name::$variant => $id,)+
        }
      }
    }

    impl std::fmt::Display for $name {
      fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.get_id())
      }
    }

    impl std::str::FromStr for $name {
      type Err = $crate::protocol::error::Error;

      /// Reads back what `get_id` wrote, for a boundary that carries text.
      fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::ALL
          .iter()
          .copied()
          .find(|variant| variant.get_id() == id)
          .ok_or_else(|| $crate::protocol::error::Error::Build(format!($unknown, id)))
      }
    }
  };
}

pub mod account_state;
pub mod attempt;
pub mod endpoint;
pub mod error;
pub mod http_error;
pub mod json_read;
pub mod model_list;
pub mod model_use;
pub mod token_count;
pub mod upstream_compaction;
pub mod web_search;

pub use model_use::message::{ContentBlock, Message, ReasoningOpaqueKind};
pub use model_use::request::{ReasoningConfig, ReasoningSummary, Request, ToolChoice};
pub use model_use::response::{Response, StopReason, Usage};
pub use model_use::stream::{BlockKind, StreamAccumulator, StreamEvent};
pub use model_use::tool::Tool;
pub use token_count::{TokenCount, TokenCountProtocol};
pub use upstream_compaction::{UpstreamCompaction, UpstreamCompactionRequest};
