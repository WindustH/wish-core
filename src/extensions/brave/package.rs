//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "brave",
  tools: &[&super::search_tool::REGISTRATION],
  images: &[],
  media: &[],
  search: &[&super::search::CODEC],
  fetch: &[],
  presets: include_str!("presets.json"),
};
