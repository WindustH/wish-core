//! Where Wish keeps its configuration and data when started without `--config`: the platform's
//! usual places for a user's settings and data, under `wish-agent`. The first such start writes a
//! configuration that listens on loopback and keeps its data beside it.
use serde_json::json;
use std::path::PathBuf;

const NAME: &str = "wish-agent";
/// The address a new configuration listens on: loopback, on the port the web app has always used.
const LISTEN: &str = "127.0.0.1:8790";

/// The configuration file, written first if there is none yet.
pub fn ensure_config() -> Result<PathBuf, String> {
  let (config_dir, data_dir) = locations()?;
  let path = config_dir.join("config.json");
  if path.exists() {
    return Ok(path);
  }
  std::fs::create_dir_all(&config_dir)
    .map_err(|error| format!("create {}: {error}", config_dir.display()))?;
  let text = serde_json::to_string_pretty(&json!({"listen": LISTEN, "data_dir": data_dir}))
    .map_err(|error| error.to_string())?;
  std::fs::write(&path, format!("{text}\n"))
    .map_err(|error| format!("write {}: {error}", path.display()))?;
  eprintln!("created {}", path.display());
  Ok(path)
}

/// The configuration's directory and the data directory a new configuration names.
fn locations() -> Result<(PathBuf, PathBuf), String> {
  let variable =
    |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
  if cfg!(windows) {
    let profile = variable("USERPROFILE");
    let roaming =
      variable("APPDATA").or_else(|| profile.as_ref().map(|home| home.join("AppData/Roaming")));
    let local = variable("LOCALAPPDATA").or_else(|| profile.map(|home| home.join("AppData/Local")));
    return match (roaming, local) {
      (Some(roaming), Some(local)) => Ok((roaming.join(NAME), local.join(NAME).join("data"))),
      _ => Err("neither APPDATA nor USERPROFILE is set; pass --config".into()),
    };
  }
  let home = variable("HOME").ok_or("HOME is not set; pass --config")?;
  if cfg!(target_os = "macos") {
    let dir = home.join("Library/Application Support").join(NAME);
    return Ok((dir.clone(), dir.join("data")));
  }
  let config = variable("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
  let data = variable("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
  Ok((config.join(NAME), data.join(NAME)))
}
