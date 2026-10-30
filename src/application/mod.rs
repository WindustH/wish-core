//! Application configuration and use cases. Server adapters call this layer.
pub(crate) mod accounts;
pub(crate) mod app;
pub(crate) mod attachments;
pub(crate) mod background_tools;
pub(crate) mod bridge;
pub(crate) mod bridge_access;
pub(crate) mod browser_login;
pub(crate) mod compaction_item;
pub(crate) mod config;
pub(crate) mod config_file;
pub(crate) mod copilot_login;
pub(crate) mod directory_queries;
mod draft_uploads;
pub(crate) mod error;
pub(crate) mod events;
pub(crate) mod fetch;
pub(crate) mod folder_access;
pub(crate) mod group_access;
pub(crate) mod groups;
pub(crate) mod history_queries;
pub(crate) mod input;
pub(crate) mod instructions;
pub(crate) mod mcp;
pub(crate) mod mcp_access;
pub(crate) mod model_observation;
pub(crate) mod naming;
pub(crate) mod peer_commands;
pub(crate) mod presets;
pub(crate) mod provider;
pub(crate) mod provider_operations;
pub(crate) mod runtime_status;
pub(crate) mod sampling;
pub(crate) mod search;
pub(crate) mod session;
pub(crate) mod session_access;
pub(crate) mod session_commands;
pub(crate) mod session_edit;
pub(crate) mod session_model;
pub(crate) mod session_queries;
pub(crate) mod session_questions;
pub(crate) mod settings;
pub(crate) mod skill_access;
pub(crate) mod skills;
pub(crate) mod storage_cleanup;
pub(crate) mod storage_usage;
pub(crate) mod task_access;
pub(crate) mod usage_queries;
pub(crate) mod web_services;

pub mod tool_commands;

pub mod command_trace;

mod pagination;

mod session_creation;

pub(crate) mod credentials;

mod model_images;

mod capabilities;

mod identity;

pub mod image_generation;

pub mod media_generation;

pub mod media_status;

pub mod audio_transcription;

pub(crate) mod google_login;
