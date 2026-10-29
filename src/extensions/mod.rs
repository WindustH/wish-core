//! Independent capabilities beyond the conversational LLM protocol.
//! Each extension owns its provider formats and feature-specific input/output contracts.
//! Application composition supplies accounts and policy; shared infrastructure does not
//! impose a universal generation request on unrelated capabilities.
pub mod catalog;
pub mod novelai;
pub(crate) mod shared;

pub mod bfl;

pub mod minimax;

pub mod xai;

pub mod alibaba;
pub mod bocha;
pub mod brave;
pub mod deepseek;
pub mod elevenlabs;
pub mod exa;
pub mod fal;
pub mod google;
pub mod jimeng;
pub mod jina;
pub mod kimi;
pub mod luma;
pub mod metaso;
pub mod openai;
pub mod perplexity;
pub mod replicate;
pub mod runninghub;
pub mod runway;
pub mod searxng;
pub mod seedance;
pub mod serper;
pub mod siliconflow;
pub mod stability;
pub mod tavily;
pub mod together;
pub mod xiaoyunque;
pub mod zai;
