//! The configuration file as the server holds it: its path, the parsed configuration and the
//! revision a save must name, the redacted form the settings page reads, and writing a new one.
//!
//! Secrets never leave the server. The settings page reads each as `<redacted>` and sends that back
//! for the ones it leaves alone; a save puts the stored value in its place.
use crate::server::{config::Config, error::ApiError, provider::ProviderConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const REDACTED: &str = "<redacted>";

pub struct ConfigFile {
  path: PathBuf,
  pub config: Config,
  /// The SHA-256 of the file's bytes. A save names the revision it was read at.
  pub revision: String,
}
impl ConfigFile {
  pub fn open(path: PathBuf, config: Config) -> Result<Self, ApiError> {
    let bytes = std::fs::read(&path).map_err(ApiError::internal)?;
    Ok(Self { path, config, revision: hash(&bytes) })
  }
  /// The configuration with its secrets redacted, and its revision.
  pub fn redacted(&self) -> Value {
    let mut config = serde_json::to_value(&self.config).unwrap();
    redact(&mut config);
    json!({"revision":self.revision,"config":config})
  }
  /// The configuration a save sends, with its redacted secrets put back from this one. The save
  /// must name the current revision, which the file on disk must still have.
  pub async fn read_save(&self, revision: &str, mut value: Value) -> Result<Config, ApiError> {
    let source = tokio::fs::read(&self.path).await.map_err(ApiError::internal)?;
    if revision != self.revision || hash(&source) != revision {
      return Err(ApiError::conflict("configuration changed; reload before saving"));
    }
    restore(&mut value, &serde_json::to_value(&self.config).unwrap())?;
    serde_json::from_value(value).map_err(|e| ApiError::bad_request(e.to_string()))
  }
  /// A provider a page holds before saving it, with the secrets it sends back redacted put back
  /// from the provider of that id here.
  pub fn read_draft_provider(&self, id: &str, provider: Value) -> Result<ProviderConfig, ApiError> {
    let mut value = json!({ "providers": { id: provider } });
    restore(&mut value, &serde_json::to_value(&self.config).unwrap())?;
    serde_json::from_value(value["providers"][id].take())
      .map_err(|e| ApiError::bad_request(e.to_string()))
  }
  /// Writes a configuration over the file and takes it as the current one.
  pub async fn write(&mut self, config: Config) -> Result<(), ApiError> {
    let bytes = serde_json::to_vec_pretty(&config).map_err(ApiError::internal)?;
    write_atomic(&self.path, &bytes).await?;
    self.config = config;
    self.revision = hash(&bytes);
    Ok(())
  }
}

fn hash(bytes: &[u8]) -> String {
  format!("{:x}", Sha256::digest(bytes))
}

/// Replaces a file whole or not at all, keeping its permissions.
async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ApiError> {
  let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
  let permissions = tokio::fs::metadata(path).await.map_err(ApiError::internal)?.permissions();
  tokio::fs::write(&temp, bytes).await.map_err(ApiError::internal)?;
  tokio::fs::set_permissions(&temp, permissions).await.map_err(ApiError::internal)?;
  if let Err(error) = tokio::fs::rename(&temp, path).await {
    let _ = tokio::fs::remove_file(&temp).await;
    return Err(ApiError::internal(error));
  }
  Ok(())
}

/// Where the configuration keeps secrets: items under a path, every entry of the map there or the
/// configuration itself, and the fields of an item that hold one.
struct Secrets {
  items: &'static [&'static str],
  each: bool,
  fields: &'static [Secret],
}
/// A field holding a secret, or with a final `*` every entry of the map it names.
struct Secret {
  path: &'static [&'static str],
  /// Left as it is when empty: nothing is secret then.
  shown_empty: bool,
  /// What a save that sends it back redacted is told when none is stored, by item and entry.
  missing: fn(&str, &str) -> String,
}
/// In the order a save is checked: item by item, as the save lists them, each field in turn.
const SECRETS: &[Secrets] = &[
  Secrets {
    items: &["providers"],
    each: true,
    fields: &[
      Secret {
        path: &["api_key"],
        shown_empty: false,
        missing: |_, _| "redacted API key has no stored value".into(),
      },
      Secret {
        path: &["refresh_token"],
        shown_empty: false,
        missing: |_, _| "redacted refresh token has no stored value".into(),
      },
      Secret {
        path: &["credentials", "*"],
        shown_empty: false,
        missing: |_, _| "redacted credential has no stored value".into(),
      },
      Secret {
        path: &["headers", "*"],
        shown_empty: false,
        missing: |_, _| "redacted header has no stored value".into(),
      },
    ],
  },
  Secrets {
    items: &[],
    each: false,
    fields: &[Secret {
      path: &["proxy", "password"],
      shown_empty: true,
      missing: |_, _| "redacted proxy password has no stored value".into(),
    }],
  },
  // An MCP server's environment and headers carry its keys.
  Secrets {
    items: &["mcp", "servers"],
    each: true,
    fields: &[
      Secret {
        path: &["env", "*"],
        shown_empty: false,
        missing: |_, name| format!("redacted MCP env value `{name}` has no stored value"),
      },
      Secret {
        path: &["headers", "*"],
        shown_empty: false,
        missing: |_, name| format!("redacted MCP headers value `{name}` has no stored value"),
      },
    ],
  },
  // A search provider's key and headers are secrets like a model provider's.
  Secrets {
    items: &["search", "providers"],
    each: true,
    fields: &[
      Secret {
        path: &["api_key"],
        shown_empty: false,
        missing: |id, _| format!("redacted search provider key `{id}` has no stored value"),
      },
      Secret {
        path: &["headers", "*"],
        shown_empty: false,
        missing: |_, name| format!("redacted search header `{name}` has no stored value"),
      },
    ],
  },
];

fn redact(config: &mut Value) {
  for secrets in SECRETS {
    for (_, item) in items(config, secrets) {
      for secret in secrets.fields {
        for (_, value) in fields(item, secret.path) {
          let empty = value.as_str().is_some_and(str::is_empty);
          if !value.is_null() && !(empty && secret.shown_empty) {
            *value = json!(REDACTED);
          }
        }
      }
    }
  }
}

fn restore(config: &mut Value, stored: &Value) -> Result<(), ApiError> {
  for secrets in SECRETS {
    let stored_items = secrets.items.iter().fold(stored, |value, name| &value[name]);
    for (id, item) in items(config, secrets) {
      let stored_item = if secrets.each { &stored_items[id.as_str()] } else { stored_items };
      for secret in secrets.fields {
        for (name, value) in fields(item, secret.path) {
          if value != REDACTED {
            continue;
          }
          let kept = secret
            .path
            .iter()
            .try_fold(stored_item, |value, part| {
              value.get(if *part == "*" { name.as_str() } else { *part })
            })
            .filter(|kept| !kept.is_null())
            .ok_or_else(|| ApiError::bad_request((secret.missing)(&id, &name)))?;
          *value = kept.clone();
        }
      }
    }
  }
  Ok(())
}

/// The items a group of secrets sits in, by key.
fn items<'a>(config: &'a mut Value, secrets: &Secrets) -> Vec<(String, &'a mut Value)> {
  let mut value = config;
  for name in secrets.items {
    value = &mut value[name];
  }
  if !secrets.each {
    return vec![(String::new(), value)];
  }
  value.as_object_mut().into_iter().flatten().map(|(id, item)| (id.clone(), item)).collect()
}

/// The values at a field's path in an item, each by the key its `*` stands for.
fn fields<'a>(item: &'a mut Value, path: &[&str]) -> Vec<(String, &'a mut Value)> {
  let Some((last, parents)) = path.split_last() else { return Vec::new() };
  let parent = parents.iter().try_fold(item, |value, name| value.get_mut(name));
  let Some(parent) = parent else { return Vec::new() };
  if *last != "*" {
    return parent.get_mut(last).map(|value| (String::new(), value)).into_iter().collect();
  }
  parent.as_object_mut().into_iter().flatten().map(|(name, value)| (name.clone(), value)).collect()
}
