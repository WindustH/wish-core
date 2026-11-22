//! Persistent, session-local names for opaque identifiers. Never recycle an assigned number.
//! Only these public variables can be expanded by tools and Markdown; process secrets are excluded.
use super::error::{ApplicationError, blocking};
use crate::{session::ToolOutcome, storage::blobs};
use serde::{Deserialize, Serialize};
use std::{
  collections::{BTreeMap, HashMap, HashSet},
  io::Write,
  path::{Path, PathBuf},
  sync::{Arc, LazyLock, Mutex, RwLock, Weak},
};

fn ordinal(mut index: usize) -> String {
  const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
  let mut digits = Vec::new();
  loop {
    digits.push(DIGITS[index % 36] as char);
    index /= 36;
    if index == 0 {
      break;
    }
  }
  digits.into_iter().rev().collect()
}

const FILE: &str = ".env-aliases.json";
const MAX_BYTES: usize = 4 * 1024 * 1024;
static OPENING: LazyLock<crate::utils::keyed_lock::KeyedLocks<PathBuf>> =
  LazyLock::new(Default::default);
static OPEN: LazyLock<Mutex<BTreeMap<PathBuf, Weak<Aliases>>>> = LazyLock::new(Default::default);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Names {
  version: u8,
  blobs: Vec<String>,
  sessions: Vec<String>,
  #[serde(default)]
  announced: bool,
}
pub struct Aliases {
  directory: PathBuf,
  names: Mutex<Names>,
  /// Task startup takes a snapshot; changing this never mutates a running process.
  pub environment: Arc<RwLock<BTreeMap<String, String>>>,
}
impl Aliases {
  /// Call on the blocking pool. A live session and input admission share the same registry.
  pub fn open(directory: &Path) -> Result<Arc<Self>, ApplicationError> {
    let directory = std::path::absolute(directory).map_err(ApplicationError::internal)?;
    let opening = OPENING.get(directory.clone());
    let _opening = opening.lock().unwrap();
    if let Some(found) = OPEN.lock().unwrap().get(&directory).and_then(Weak::upgrade) {
      return Ok(found);
    }
    let owner = directory
      .file_name()
      .and_then(|v| v.to_str())
      .filter(|v| uuid::Uuid::parse_str(v).is_ok())
      .ok_or_else(|| ApplicationError::invalid("invalid alias owner"))?
      .to_owned();
    let names: Names = match crate::utils::bounded_file::read_sync(&directory.join(FILE), MAX_BYTES)
    {
      Ok(bytes) => serde_json::from_slice(&bytes).map_err(ApplicationError::internal)?,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
        Names { version: 1, blobs: vec![], sessions: vec![owner.clone()], announced: false }
      }
      Err(error) => return Err(ApplicationError::internal(error)),
    };
    if names.version != 1
      || names.sessions.first() != Some(&owner)
      || names.blobs.iter().any(|id| !blobs::is_blob_id(id))
      || names.sessions.iter().any(|id| uuid::Uuid::parse_str(id).is_err())
      || names.blobs.iter().collect::<std::collections::BTreeSet<_>>().len() != names.blobs.len()
      || names.sessions.iter().collect::<std::collections::BTreeSet<_>>().len()
        != names.sessions.len()
    {
      return Err(ApplicationError::invalid("invalid session alias registry"));
    }
    let aliases =
      Arc::new(Self { directory, names: Mutex::new(names), environment: Default::default() });
    let names = aliases.names.lock().unwrap();
    if !aliases.directory.join(FILE).exists() {
      aliases.save(&names)?;
    }
    aliases.publish(&names);
    drop(names);
    let mut open = OPEN.lock().unwrap();
    open.retain(|_, value| value.strong_count() > 0);
    open.insert(aliases.directory.clone(), Arc::downgrade(&aliases));
    Ok(aliases)
  }
  fn save(&self, names: &Names) -> Result<(), ApplicationError> {
    let bytes = serde_json::to_vec(names).map_err(ApplicationError::internal)?;
    if bytes.len() > MAX_BYTES {
      return Err(ApplicationError::invalid("session alias limit exceeded"));
    }
    std::fs::create_dir_all(&self.directory).map_err(ApplicationError::internal)?;
    let temporary = self.directory.join(format!(".env-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
      let mut file = std::fs::File::create(&temporary)?;
      file.write_all(&bytes)?;
      file.sync_all()?;
      std::fs::rename(&temporary, self.directory.join(FILE))
    })();
    if result.is_err() {
      let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(ApplicationError::internal)
  }
  fn publish(&self, names: &Names) {
    let mut env = self.environment.write().unwrap();
    env.insert("WISH_BLOB_ROOT".into(), self.directory.to_string_lossy().into_owned());
    for (i, id) in names.blobs.iter().enumerate() {
      env.insert(format!("WISH_BLOB_ID_{}", ordinal(i)), id.clone());
    }
    for (i, id) in names.sessions.iter().enumerate() {
      env.insert(format!("WISH_SESSION_ID_{}", ordinal(i)), id.clone());
    }
  }
  /// Forks preserve the numbering used in copied history, but self is always slot zero.
  pub fn inherit(&self, source: &Aliases) -> Result<(), ApplicationError> {
    // Legacy contexts may reference blobs before aliases or Library listing ever touched them.
    let mut ids = std::collections::BTreeSet::new();
    for file in std::fs::read_dir(&source.directory).map_err(ApplicationError::internal)? {
      let file = file.map_err(ApplicationError::internal)?;
      let name = file.file_name().to_string_lossy().into_owned();
      let id = name.strip_suffix(".json").unwrap_or(&name);
      if blobs::is_blob_id(id) {
        ids.insert(id.to_owned());
      }
    }
    source.register(&ids.iter().cloned().collect::<Vec<_>>(), &[])?;
    let mut inherited = source.names.lock().unwrap().clone();
    let mut names = self.names.lock().unwrap();
    inherited.sessions[0] = names.sessions[0].clone();
    inherited.announced = names.announced;
    for id in ids {
      self.copy_blob(source, &id)?;
    }
    blobs::library::inherit(&source.directory, &self.directory)?;
    self.save(&inherited)?;
    self.publish(&inherited);
    *names = inherited;
    Ok(())
  }
  /// Share immutable stored bytes (hard-link when possible) and preserve linked-file descriptors.
  /// The recipient's blob variables must resolve to files in its own namespace.
  fn copy_blob(&self, source: &Aliases, id: &str) -> Result<bool, ApplicationError> {
    let _catalog = blobs::catalog::Mutation(&self.directory);
    let from = source.directory.join(id);
    let to = self.directory.join(id);
    let mut found = false;
    if from.is_file() {
      if !to.exists() && std::fs::hard_link(&from, &to).is_err() {
        let temporary = self.directory.join(format!(".copy-{}.tmp", uuid::Uuid::new_v4()));
        let result =
          std::fs::copy(&from, &temporary).and_then(|_| std::fs::rename(&temporary, &to));
        let _ = std::fs::remove_file(&temporary);
        result.map_err(ApplicationError::internal)?;
      }
      found = true;
    }
    let sidecar = source.directory.join(format!("{id}.json"));
    if sidecar.is_file() {
      let bytes = crate::utils::bounded_file::read_sync(&sidecar, 64 * 1024)
        .map_err(ApplicationError::internal)?;
      let mut blob: blobs::Blob =
        serde_json::from_slice(&bytes).map_err(ApplicationError::internal)?;
      if !blob.linked {
        blob.path = to.to_string_lossy().into_owned();
      }
      let target = self.directory.join(format!("{id}.json"));
      if !target.exists() {
        blobs::atomic_write_sync(
          &target,
          &serde_json::to_vec(&blob).map_err(ApplicationError::internal)?,
        )
        .map_err(ApplicationError::internal)?;
      }
      found = true;
    }
    Ok(found)
  }
  /// Ordinary prose is not a tool argument: unknown variables and incomplete examples stay literal.
  pub fn receive(&self, source: &Aliases, text: &str) -> Result<String, ApplicationError> {
    let expanded = source.expand_with(text, false)?;
    let env = source.snapshot();
    let mut blobs = Vec::new();
    let mut sessions = Vec::new();
    for (key, value) in &env {
      if !expanded.contains(value) {
        continue;
      }
      if key.starts_with("WISH_BLOB_ID_") && self.copy_blob(source, value)? {
        blobs.push(value.clone());
      }
      if key.starts_with("WISH_SESSION_ID_") {
        sessions.push(value.clone());
      }
    }
    self.register(&blobs, &sessions)?;
    // Only rewrite paths whose resource was actually copied. A root-only replacement
    // would silently redirect missing/unregistered sender files into our namespace.
    let mut expanded = expanded;
    for id in &blobs {
      expanded = replace_path(
        &expanded,
        &source.directory.join(id).to_string_lossy(),
        &self.directory.join(id).to_string_lossy(),
      );
    }
    Ok(self.shorten(&expanded))
  }
  pub fn root(&self) -> &Path {
    &self.directory
  }
  pub fn snapshot_existing(directory: &Path) -> Result<BTreeMap<String, String>, ApplicationError> {
    let names: Names = match crate::utils::bounded_file::read_sync(&directory.join(FILE), MAX_BYTES)
    {
      Ok(bytes) => serde_json::from_slice(&bytes).map_err(ApplicationError::internal)?,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
      Err(e) => return Err(ApplicationError::internal(e)),
    };
    Ok(
      names
        .blobs
        .into_iter()
        .enumerate()
        .map(|(i, id)| (format!("WISH_BLOB_ID_{}", ordinal(i)), id))
        .collect(),
    )
  }
  pub fn snapshot(&self) -> BTreeMap<String, String> {
    self.environment.read().unwrap().clone()
  }
  pub fn needs_notice(&self) -> bool {
    !self.names.lock().unwrap().announced
  }
  pub fn mark_announced(&self) -> Result<(), ApplicationError> {
    let mut names = self.names.lock().unwrap();
    let mut next = names.clone();
    next.announced = true;
    self.save(&next)?;
    *names = next;
    Ok(())
  }
  pub fn blob(&self, id: &str) -> Result<String, ApplicationError> {
    if !blobs::is_blob_id(id) {
      return Err(ApplicationError::invalid("invalid blob id"));
    }
    self.register(&[id.to_owned()], &[])?;
    let names = self.names.lock().unwrap();
    Ok(format!("$WISH_BLOB_ID_{}", ordinal(names.blobs.iter().position(|v| v == id).unwrap())))
  }
  pub fn session(&self, id: &str) -> Result<String, ApplicationError> {
    if uuid::Uuid::parse_str(id).is_err() {
      return Err(ApplicationError::invalid("invalid session id"));
    }
    self.register(&[], &[id.to_owned()])?;
    let names = self.names.lock().unwrap();
    Ok(format!(
      "$WISH_SESSION_ID_{}",
      ordinal(names.sessions.iter().position(|v| v == id).unwrap())
    ))
  }
  pub(super) fn register(
    &self,
    blobs: &[String],
    sessions: &[String],
  ) -> Result<(), ApplicationError> {
    let mut names = self.names.lock().unwrap();
    let mut next = names.clone();
    for id in blobs {
      if !next.blobs.contains(id) {
        next.blobs.push(id.clone());
      }
    }
    for id in sessions {
      if !next.sessions.contains(id) {
        next.sessions.push(id.clone());
      }
    }
    if next.blobs.len() == names.blobs.len() && next.sessions.len() == names.sessions.len() {
      return Ok(());
    }
    self.save(&next)?;
    self.publish(&next);
    *names = next;
    Ok(())
  }
  /// Single-pass expansion: no shell evaluation, recursive expansion, or inherited env lookup.
  pub fn expand(&self, text: &str) -> Result<String, ApplicationError> {
    self.expand_with(text, true)
  }
  fn expand_with(&self, text: &str, strict: bool) -> Result<String, ApplicationError> {
    let env = self.snapshot();
    if strict && let Some(value) = env.get(text) {
      return Ok(value.clone());
    }
    let mut output = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
      output.push_str(&rest[..at]);
      rest = &rest[at + 1..];
      let (name, length) = if let Some(braced) = rest.strip_prefix('{') {
        let Some(end) = braced.find('}') else {
          if strict && braced.starts_with("WISH_") {
            return Err(ApplicationError::invalid("unclosed environment variable"));
          }
          output.push('$');
          output.push_str(rest);
          return Ok(output);
        };
        (&braced[..end], end + 2)
      } else {
        let end =
          rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        (&rest[..end], end)
      };
      if !name.starts_with("WISH_") {
        output.push('$');
        continue;
      }
      match env.get(name) {
        Some(value) => output.push_str(value),
        None if strict => {
          return Err(ApplicationError::invalid(format!("unknown session variable ${name}")));
        }
        None => {
          output.push('$');
          output.push_str(&rest[..length]);
        }
      }
      rest = &rest[length..];
    }
    output.push_str(rest);
    Ok(output)
  }
  pub fn shorten(&self, text: &str) -> String {
    Shortener::new(self.snapshot()).text(text)
  }
  pub fn shorten_value(&self, value: &mut serde_json::Value) {
    Shortener::new(self.snapshot()).value(value);
  }
  /// Deduplicate tokens and query session IDs under one database lock; never scan JSON escapes.
  pub(super) fn discover(
    &self,
    text: &str,
    management: &crate::storage::management::ManagementStore,
  ) -> Result<(), ApplicationError> {
    self.discover_texts([text], management)
  }
  pub(super) fn discover_value(
    &self,
    value: &serde_json::Value,
    management: &crate::storage::management::ManagementStore,
  ) -> Result<(), ApplicationError> {
    let mut texts = Vec::new();
    strings(value, &mut texts);
    self.discover_texts(texts, management)
  }
  fn discover_texts<'a>(
    &self,
    texts: impl IntoIterator<Item = &'a str>,
    management: &crate::storage::management::ManagementStore,
  ) -> Result<(), ApplicationError> {
    let env = self.snapshot();
    let known: HashSet<_> = env.values().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut blobs = Vec::new();
    let mut sessions = Vec::new();
    for text in texts {
      for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        if known.contains(token) || !seen.insert(token) {
          continue;
        }
        if crate::storage::blobs::is_blob_id(token)
          && (self.directory.join(token).is_file()
            || self.directory.join(format!("{token}.json")).is_file())
        {
          blobs.push(token.to_owned());
        } else if uuid::Uuid::parse_str(token).is_ok() {
          sessions.push(token.to_owned());
        }
      }
    }
    let sessions =
      if sessions.is_empty() { sessions } else { management.existing_sessions(&sessions)? };
    self.register(&blobs, &sessions)
  }
}

/// Match a complete path component, not a prefix of another filename or directory.
fn replace_path(text: &str, path: &str, replacement: &str) -> String {
  if path.is_empty() {
    return text.to_owned();
  }
  let mut output = String::with_capacity(text.len());
  let mut cursor = 0;
  for (start, _) in text.match_indices(path) {
    let end = start + path.len();
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    let begins = before.is_none_or(|c| c.is_whitespace() || "\"'`=([{,:>".contains(c));
    let ends = after.is_none_or(|c| c.is_whitespace() || "/\\\"'`)]},;:!?<>#".contains(c));
    if begins && ends {
      output.push_str(&text[cursor..start]);
      output.push_str(replacement);
      cursor = end;
    }
  }
  output.push_str(&text[cursor..]);
  output
}
struct Shortener {
  env: BTreeMap<String, String>,
  reverse: HashMap<String, String>,
}
impl Shortener {
  fn new(env: BTreeMap<String, String>) -> Self {
    let reverse = env
      .iter()
      .filter(|(name, _)| name.starts_with("WISH_BLOB_ID_") || name.starts_with("WISH_SESSION_ID_"))
      .map(|(name, value)| (value.clone(), name.clone()))
      .collect();
    Self { env, reverse }
  }
  fn text(&self, text: &str) -> String {
    let mut text = text.to_owned();
    for name in ["WISH_BLOB_ROOT", "WISH_TMPDIR"] {
      if let Some(value) = self.env.get(name) {
        text = replace_path(&text, value, &format!("${name}"));
      }
    }
    text
      .split_inclusive(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
      .map(|part| {
        let end = part
          .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
          .unwrap_or(part.len());
        match self.reverse.get(&part[..end]) {
          Some(name) => format!("${name}{}", &part[end..]),
          None => part.to_owned(),
        }
      })
      .collect()
  }
  fn value(&self, value: &mut serde_json::Value) {
    match value {
      serde_json::Value::String(text) => *text = self.text(text),
      serde_json::Value::Array(items) => {
        for item in items {
          self.value(item);
        }
      }
      serde_json::Value::Object(items) => {
        for item in items.values_mut() {
          self.value(item);
        }
      }
      _ => {}
    }
  }
}
fn strings<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a str>) {
  match value {
    serde_json::Value::String(text) => out.push(text),
    serde_json::Value::Array(items) => {
      for item in items {
        strings(item, out);
      }
    }
    serde_json::Value::Object(items) => {
      for item in items.values() {
        strings(item, out);
      }
    }
    _ => {}
  }
}

pub async fn shorten_outcome(
  slot: &Arc<super::session::SessionSlot>,
  mut outcome: ToolOutcome,
) -> ToolOutcome {
  if !matches!(
    outcome,
    ToolOutcome::Report { .. }
      | ToolOutcome::Success(_)
      | ToolOutcome::SuccessWithMetadata { .. }
      | ToolOutcome::SuccessWithInput { .. }
      | ToolOutcome::Failed(_)
      | ToolOutcome::Unknown(_)
  ) {
    return outcome;
  }
  let aliases = slot.aliases.clone();
  let management = slot.management.clone();
  let result = blocking(move || {
    match &mut outcome {
      ToolOutcome::Report { text, output, .. } => {
        let mut texts = vec![text.as_str()];
        strings(output, &mut texts);
        aliases.discover_texts(texts, &management)?;
        *text = aliases.shorten(text);
      }
      ToolOutcome::Success(output) | ToolOutcome::SuccessWithMetadata { output, .. } => {
        let mut texts = Vec::new();
        strings(output, &mut texts);
        aliases.discover_texts(texts, &management)?;
        let mut model_output = output.clone();
        aliases.shorten_value(&mut model_output);
        if model_output != *output {
          let output = output.clone();
          let metadata = outcome.metadata();
          outcome = ToolOutcome::SuccessWithProjection { output, model_output, metadata };
        }
      }
      ToolOutcome::SuccessWithInput { output, model_text, .. } if model_text.is_none() => {
        let mut projected = output.clone();
        aliases.shorten_value(&mut projected);
        if projected != *output {
          *model_text = Some(projected.to_string());
        }
      }
      ToolOutcome::Failed(text) | ToolOutcome::Unknown(text) => {
        aliases.discover(text, &management)?;
        *text = aliases.shorten(text);
      }
      _ => {}
    }
    Ok(outcome)
  })
  .await;
  result.unwrap_or_else(|error| ToolOutcome::Failed(error.to_string()))
}
