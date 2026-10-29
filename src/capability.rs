//! Stable built-in capability identities, names and command discovery.
//! Settings and executors attach policy to these identities instead of repeating name tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
  History,
  Image,
  Sessions,
  Shell,
  Task,
  AskUser,
  WebFetch,
  WebSearch,
  ImageGenerate,
  VideoGenerate,
  AudioGenerate,
  AudioTranscribe,
  ExtensionDiscovery,
}
impl Capability {
  /// Stable advertisement order for a newly built generation.
  pub const ALL: [Self; 13] = [
    Self::History,
    Self::Image,
    Self::Sessions,
    Self::Shell,
    Self::Task,
    Self::AskUser,
    Self::WebFetch,
    Self::WebSearch,
    Self::ImageGenerate,
    Self::VideoGenerate,
    Self::AudioGenerate,
    Self::AudioTranscribe,
    Self::ExtensionDiscovery,
  ];
  pub fn from_tool(name: &str) -> Option<Self> {
    Some(match name {
      "history_search" | "history_read" | "history_query" => Self::History,
      "view_image" => Self::Image,
      "session" => Self::Sessions,
      "shell" | "shell_start" | "shell_edit" | "shell_poll" | "shell_write" | "shell_kill" => {
        Self::Shell
      }
      "task" => Self::Task,
      "ask_user" => Self::AskUser,
      "extension_tools" => Self::ExtensionDiscovery,
      _ => {
        return crate::extensions::catalog::find(name).map(|registration| {
          match registration.permission {
            crate::extensions::catalog::Permission::ImageGeneration => Self::ImageGenerate,
            crate::extensions::catalog::Permission::VideoGeneration => Self::VideoGenerate,
            crate::extensions::catalog::Permission::AudioGeneration => Self::AudioGenerate,
            crate::extensions::catalog::Permission::Transcription => Self::AudioTranscribe,
            crate::extensions::catalog::Permission::WebSearch => Self::WebSearch,
            crate::extensions::catalog::Permission::WebFetch => Self::WebFetch,
          }
        });
      }
    })
  }
  pub fn from_command(name: &str) -> Option<Self> {
    match name {
      "askuser" => Some(Self::AskUser),
      _ => Self::from_tool(name),
    }
  }
  pub fn command_backed(self) -> bool {
    !matches!(self, Self::Image | Self::Shell)
  }
  pub fn command_instruction(self) -> Option<&'static str> {
    Some(match self {
      Self::Sessions => "`wish session` — sessions and groups",
      Self::Task => "`wish task` — task management",
      Self::ExtensionDiscovery => {
        "`wish extension search [TAG ...]`, `wish extension inspect TOOL`, `wish extension call TOOL --json '<arguments>'` — discover and invoke extension-owned tools"
      }
      Self::History => "`wish history` — search/read session history",
      Self::AskUser => "`wish askuser` — ask the user",
      _ => return None,
    })
  }
}
