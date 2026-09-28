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
//! only repeats their work on the next start.

mod m0001_mcp_switch;

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
  #[allow(dead_code, reason = "no step has changed the sessions themselves yet")]
  pub wish: Option<&'a Connection>,
}

struct Step {
  summary: &'static str,
  apply: fn(&mut Data) -> Result<(), String>,
}

/// In order: the step at index `i` takes version `i` to `i + 1`.
const STEPS: &[Step] =
  &[Step { summary: m0001_mcp_switch::SUMMARY, apply: m0001_mcp_switch::apply }];

/// The format this build reads.
pub const LATEST: u32 = STEPS.len() as u32;

const FORMAT_FILE: &str = "format.json";
const DATABASES: [&str; 2] = ["management.sqlite", "wish.sqlite"];

/// Brings the configuration at `config_path` and its data directory up to [`LATEST`].
///
/// A configuration that cannot be read as JSON is left alone: reading it properly reports why.
pub fn run(config_path: &Path) -> Result<(), String> {
  let Ok(source) = std::fs::read(config_path) else { return Ok(()) };
  let Ok(mut config) = serde_json::from_slice::<Value>(&source) else { return Ok(()) };
  let data_dir = PathBuf::from(config.get("data_dir").and_then(Value::as_str).unwrap_or("data"));
  let from = match read_version(&data_dir)? {
    Some(version) => version,
    None if DATABASES.iter().any(|name| data_dir.join(name).exists()) => 0,
    None => return write_version(&data_dir, LATEST),
  };
  if from > LATEST {
    return Err(format!(
      "{} was written by a newer build (format {from}; this build reads format {LATEST})",
      data_dir.display()
    ));
  }
  if from == LATEST {
    return Ok(());
  }
  let pending = &STEPS[from as usize..];
  eprintln!("migrating {} from format {from} to {LATEST}:", data_dir.display());
  for (offset, step) in pending.iter().enumerate() {
    eprintln!("  {}: {}", from as usize + offset + 1, step.summary);
  }
  let backup = back_up(config_path, &data_dir, from)?;
  let failed = |message: String| {
    format!(
      "{message}\nnothing was changed; a copy taken before migrating is in {}",
      backup.display()
    )
  };

  let open = |name: &str| -> Result<Option<Connection>, String> {
    let path = data_dir.join(name);
    if !path.exists() {
      return Ok(None);
    }
    Connection::open(&path).map(Some).map_err(|error| failed(format!("open {name}: {error}")))
  };
  let (mut management, mut wish) = (open(DATABASES[0])?, open(DATABASES[1])?);
  let management = begin(&mut management).map_err(|error| failed(error.to_string()))?;
  let wish = begin(&mut wish).map_err(|error| failed(error.to_string()))?;
  let before = config.clone();
  {
    let mut data =
      Data { config: &mut config, management: management.as_deref(), wish: wish.as_deref() };
    for (offset, step) in pending.iter().enumerate() {
      (step.apply)(&mut data)
        .map_err(|error| failed(format!("step {}: {error}", from as usize + offset + 1)))?;
    }
  }
  for (name, transaction) in [(DATABASES[1], wish), (DATABASES[0], management)] {
    if let Some(transaction) = transaction {
      transaction.commit().map_err(|error| {
        format!("commit {name}: {error}\nrestore the data directory from {}", backup.display())
      })?;
    }
  }
  if config != before {
    let text = serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?;
    replace_file(config_path, &text).map_err(|error| {
      format!("write {}: {error}; the databases are migrated already", config_path.display())
    })?;
  }
  write_version(&data_dir, LATEST)?;
  eprintln!("migrated; the copy taken before is in {}", backup.display());
  Ok(())
}

fn begin(
  connection: &mut Option<Connection>,
) -> rusqlite::Result<Option<rusqlite::Transaction<'_>>> {
  connection.as_mut().map(Connection::transaction).transpose()
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
  for name in DATABASES {
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
