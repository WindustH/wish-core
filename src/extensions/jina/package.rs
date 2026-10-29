//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "jina",
  tools: &[&super::search_tool::REGISTRATION, &super::fetch_tool::REGISTRATION],
  images: &[],
  media: &[],
  search: &[&super::search::CODEC],
  fetch: &[&super::fetch::CODEC],
  presets: include_str!("presets.json"),
};
