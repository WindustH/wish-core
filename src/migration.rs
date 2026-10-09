//! Format migrations: bring a data directory and its configuration file up to what this build
//! reads.
//!
//! `run` is called once at startup, before the configuration is parsed or a database is opened. The
//! data directory records how far it has been migrated in `format.json` (`{"version": N}`). Each
//! step takes it one version further, and this build reads version [`LATEST`] only. A directory
//! without the file is either new, and is simply marked current, or older than this module, and is
//! taken as version 0.
//!
//! A step is frozen once released. It reads and writes the raw formats - the configuration file as
//! JSON, the databases through SQL - and never the types the rest of the program uses, which keep
//! changing after it. The rest of the program knows nothing of this module: it reads the current
//! formats and refuses others.
//!
//! Before the first pending step, the databases and the configuration file are copied to
//! `backups/before-migration-<from>-to-<to>-<time>/` in the data directory. The steps then run in
//! one transaction per database and against the configuration in memory, so nothing is written
//! unless every step succeeds. The databases commit, then the configuration file is replaced, then
//! the new version is recorded. Steps are written to be repeatable, so a crash between those writes
//! only repeats their work on the next start. Last, the databases are vacuumed.

mod m0001_mcp_switch;
mod m0002_web_search;
mod m0003_tool_batches;
mod m0004_compact_storage;
mod m0005_short_outcomes;
mod m0006_standby_lists;
mod m0007_skills_switch;
mod m0008_compact_management;
mod m0009_sessions_switch;
mod m0010_group_tables;
mod m0011_free_groups;
mod m0012_folders;
mod m0013_anthropic_user_agent;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// What a step works on: the configuration file's JSON, and each database the directory has, inside
/// its transaction.
pub struct Data<'a> {
  pub config: &'a mut Value,
  /// `management.sqlite`: the session index, call records, stream samples.
  pub management: Option<&'a Connection>,
  /// `wish.sqlite`: sessions themselves.
  pub wish: Option<&'a Connection>,
}

struct Step {
  summary: &'static str,
  apply: fn(&mut Data) -> Result<(), String>,
}

/// In order: the step at index `i` takes version `i` to `i + 1`.
const STEPS: &[Step] = &[
  Step { summary: m0001_mcp_switch::SUMMARY, apply: m0001_mcp_switch::apply },
  Step { summary: m0002_web_search::SUMMARY, apply: m0002_web_search::apply },
  Step { summary: m0003_tool_batches::SUMMARY, apply: m0003_tool_batches::apply },
  Step { summary: m0004_compact_storage::SUMMARY, apply: m0004_compact_storage::apply },
  Step { summary: m0005_short_outcomes::SUMMARY, apply: m0005_short_outcomes::apply },
  Step { summary: m0006_standby_lists::SUMMARY, apply: m0006_standby_lists::apply },
  Step { summary: m0007_skills_switch::SUMMARY, apply: m0007_skills_switch::apply },
  Step { summary: m0008_compact_management::SUMMARY, apply: m0008_compact_management::apply },
  Step { summary: m0009_sessions_switch::SUMMARY, apply: m0009_sessions_switch::apply },
  Step { summary: m0010_group_tables::SUMMARY, apply: m0010_group_tables::apply },
  Step { summary: m0011_free_groups::SUMMARY, apply: m0011_free_groups::apply },
  Step { summary: m0012_folders::SUMMARY, apply: m0012_folders::apply },
  Step { summary: m0013_anthropic_user_agent::SUMMARY, apply: m0013_anthropic_user_agent::apply },
];

/// The format this build reads.
pub const LATEST: u32 = STEPS.len() as u32;

const FORMAT_FILE: &str = "format.json";
/// The databases a data directory may have, and what a step gets them as.
const MANAGEMENT: &str = "management.sqlite";
const WISH: &str = "wish.sqlite";

/// Brings the configuration at `config_path` and its data directory up to [`LATEST`].
///
/// A configuration that cannot be read as JSON is left alone: reading it properly reports why.
pub fn run(config_path: &Path) -> Result<(), String> {
  let Ok(source) = std::fs::read(config_path) else { return Ok(()) };
  let Ok(mut config) = serde_json::from_slice::<Value>(&source) else { return Ok(()) };
  let data_dir = PathBuf::from(config.get("data_dir").and_then(Value::as_str).unwrap_or("data"));
  let Some(from) = plan(&data_dir)? else { return Ok(()) };
  eprintln!("migrating {} from format {from} to {LATEST}:", data_dir.display());
  for (index, step) in STEPS.iter().enumerate().skip(from as usize) {
    eprintln!("  {}: {}", index + 1, step.summary);
  }
  let backup = back_up(config_path, &data_dir, from)?;
  let open = |name: &str| -> Result<Option<Connection>, String> {
    let path = data_dir.join(name);
    if !path.exists() {
      return Ok(None);
    }
    Connection::open(&path)
      .map(Some)
      .map_err(|error| unchanged(format!("open {name}: {error}"), &backup))
  };
  let (mut management, mut wish) = (open(MANAGEMENT)?, open(WISH)?);
  let before = config.clone();
  apply_steps(from, &mut config, management.as_mut(), wish.as_mut(), &backup)?;
  commit(config_path, (config != before).then_some(&config), &data_dir)?;
  eprintln!("migrated; the copy taken before is in {}", backup.display());
  // Steps often delete, and some change how the file is laid out, which only a VACUUM applies. It
  // cannot run inside a transaction, and failing it loses nothing.
  for (name, connection) in [(WISH, &wish), (MANAGEMENT, &management)] {
    if let Some(Err(error)) = connection.as_ref().map(|file| file.execute_batch("VACUUM")) {
      eprintln!("compact {name}: {error}");
    }
  }
  Ok(())
}

/// The version the data directory is migrated from, or None when there is nothing to do: it is
/// current, or new, which is marked current here.
fn plan(data_dir: &Path) -> Result<Option<u32>, String> {
  let from = match read_version(data_dir)? {
    Some(version) => version,
    None if [MANAGEMENT, WISH].iter().any(|name| data_dir.join(name).exists()) => 0,
    None => return write_version(data_dir, LATEST).map(|()| None),
  };
  if from > LATEST {
    return Err(format!(
      "{} was written by a newer build (format {from}; this build reads format {LATEST})",
      data_dir.display()
    ));
  }
  Ok((from < LATEST).then_some(from))
}

/// Runs the steps after `from` against `config` and one transaction per database, and commits the
/// databases once every step succeeded.
fn apply_steps(
  from: u32,
  config: &mut Value,
  management: Option<&mut Connection>,
  wish: Option<&mut Connection>,
  backup: &Path,
) -> Result<(), String> {
  let failed = |error: rusqlite::Error| unchanged(error.to_string(), backup);
  let management = management.map(Connection::transaction).transpose().map_err(failed)?;
  let wish = wish.map(Connection::transaction).transpose().map_err(failed)?;
  let mut data = Data { config, management: management.as_deref(), wish: wish.as_deref() };
  for (index, step) in STEPS.iter().enumerate().skip(from as usize) {
    (step.apply)(&mut data)
      .map_err(|error| unchanged(format!("step {}: {error}", index + 1), backup))?;
  }
  for (name, transaction) in [(WISH, wish), (MANAGEMENT, management)] {
    if let Some(transaction) = transaction {
      transaction.commit().map_err(|error| {
        format!("commit {name}: {error}\nrestore the data directory from {}", backup.display())
      })?;
    }
  }
  Ok(())
}

/// Finishes a migration whose databases are committed: replaces the configuration file when the
/// steps changed it, then records the new version.
fn commit(config_path: &Path, config: Option<&Value>, data_dir: &Path) -> Result<(), String> {
  if let Some(config) = config {
    let text = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
    replace_file(config_path, &text).map_err(|error| {
      format!("write {}: {error}; the databases are migrated already", config_path.display())
    })?;
  }
  write_version(data_dir, LATEST)
}

/// Why a migration stopped before writing anything.
fn unchanged(message: String, backup: &Path) -> String {
  format!(
    "{message}\nnothing was changed; a copy taken before migrating is in {}",
    backup.display()
  )
}

fn read_version(data_dir: &Path) -> Result<Option<u32>, String> {
  let path = data_dir.join(FORMAT_FILE);
  let text = match std::fs::read(&path) {
    Ok(text) => text,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(format!("read {}: {error}", path.display())),
  };
  serde_json::from_slice::<Value>(&text)
    .ok()
    .and_then(|value| value["version"].as_u64())
    .and_then(|version| u32::try_from(version).ok())
    .map(Some)
    .ok_or_else(|| format!("{} does not hold a format version", path.display()))
}

fn write_version(data_dir: &Path, version: u32) -> Result<(), String> {
  std::fs::create_dir_all(data_dir)
    .map_err(|error| format!("create {}: {error}", data_dir.display()))?;
  let path = data_dir.join(FORMAT_FILE);
  replace_file(&path, format!("{}\n", json!({"version": version})).as_bytes())
    .map_err(|error| format!("write {}: {error}", path.display()))
}

/// Copies the databases (consistently, through SQLite) and the configuration file aside.
fn back_up(config_path: &Path, data_dir: &Path, from: u32) -> Result<PathBuf, String> {
  let time = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|elapsed| elapsed.as_secs())
    .unwrap_or_default();
  let directory =
    data_dir.join("backups").join(format!("before-migration-{from}-to-{LATEST}-{time}"));
  std::fs::create_dir_all(&directory)
    .map_err(|error| format!("create {}: {error}", directory.display()))?;
  for name in [MANAGEMENT, WISH] {
    let source = data_dir.join(name);
    if !source.exists() {
      continue;
    }
    let target = directory.join(name);
    Connection::open(&source)
      .and_then(|connection| connection.execute("VACUUM INTO ?1", [target.to_string_lossy()]))
      .map_err(|error| format!("back up {name}: {error}"))?;
  }
  std::fs::copy(config_path, directory.join("config.json"))
    .map_err(|error| format!("back up {}: {error}", config_path.display()))?;
  Ok(directory)
}

/// Writes a file whole or not at all, keeping its permissions.
fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
  let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
  std::fs::write(&temporary, bytes)?;
  if let Ok(metadata) = std::fs::metadata(path) {
    std::fs::set_permissions(&temporary, metadata.permissions())?;
  }
  std::fs::rename(&temporary, path).inspect_err(|_| {
    let _ = std::fs::remove_file(&temporary);
  })
}
