use super::*;
use crate::session::Session;
use crate::storage::archive::ArchiveReader;
use std::{
  collections::{HashMap, HashSet},
  path::Path,
};

#[derive(serde::Deserialize, Clone, Copy)]
#[serde(default, deny_unknown_fields)]
pub struct Include {
  pub include_blobs: bool,
  pub include_shell: bool,
  pub include_transcript: bool,
}
impl Default for Include {
  fn default() -> Self {
    Self { include_blobs: true, include_shell: true, include_transcript: true }
  }
}

// The stored value is already serialized JSON. Embed the original byte sequence, preserving
// enum representations, unknown fields, numeric precision and sparse list positions.
fn row(w: &mut Writer, fields: Value, value: &[u8]) -> Result<(), Error> {
  let mut fields = fields;
  if value.is_empty() {
    fields["released"] = json!(true);
  }
  let mut line = serde_json::to_vec(&fields).map_err(Error::internal)?;
  line.pop();
  line.extend_from_slice(b",\"value\":");
  w.bytes(&line)?;
  w.bytes(if value.is_empty() { b"null" } else { value })?;
  w.bytes(b"}\n")
}
fn transcript(w: &mut Writer, raw: &[u8]) -> Result<(), Error> {
  if raw.is_empty() {
    return Ok(());
  }
  let entry: Value = serde_json::from_slice(raw).map_err(Error::internal)?;
  if let Some((role, body)) = entry["message"].as_object().and_then(|m| m.iter().next()) {
    w.bytes(format!("\n## {role}\n\n").as_bytes())?;
    if let Some(content) = body["content"].as_array() {
      for part in content {
        if let Some(text) = part.pointer("/Text/text").and_then(Value::as_str) {
          w.bytes(text.as_bytes())?;
        } else if let Some(id) = part.pointer("/ImageRef/blob_id").and_then(Value::as_str) {
          w.bytes(format!("![image](blobs/{id})").as_bytes())?;
        } else {
          w.bytes(b"[media]")?;
        }
        w.bytes(b"\n")?;
      }
    } else if let Some(text) =
      body["plaintext"].as_str().filter(|s| !s.is_empty()).or_else(|| body["display"].as_str())
    {
      w.bytes(text.as_bytes())?;
      w.bytes(b"\n")?;
    } else {
      w.bytes(
        &serde_json::to_vec_pretty(body.get("display").unwrap_or(body)).map_err(Error::internal)?,
      )?;
      w.bytes(b"\n")?
    }
  }
  Ok(())
}
pub(crate) fn records(
  app: &App,
  id: &str,
  w: &mut Writer,
  selected: Option<&HashMap<String, HashSet<u64>>>,
  include_transcript: bool,
) -> Result<(), Error> {
  let reader = ArchiveReader::open(&app.data_dir.database(), Session::build_key(id))?;
  w.manifest["storage_schema_version"] = json!(reader.version()?);
  let lists = reader.lists()?;
  let storage_start = w.original;
  let mut entries = 0u64;
  if selected.is_none() {
    w.entry("storage/objects.jsonl", false)?;
    let mut after = String::new();
    loop {
      let page = reader.objects(&after)?;
      if page.is_empty() {
        break;
      }
      for (name, kind, value) in page {
        row(w, json!({"name":name,"kind":kind}), &value)?;
        after = name;
      }
    }
    w.entry("storage/lists.jsonl", false)?;
    for (key, name, kind, len) in &lists {
      w.bytes(
        &serde_json::to_vec(&json!({"id":key,"name":name,"kind":kind,"length":len}))
          .map_err(Error::internal)?,
      )?;
      w.bytes(b"\n")?;
    }
    w.entry("storage/items.jsonl", false)?;
  }
  for (key, name, _, _) in &lists {
    let short = name.rsplit('/').next().unwrap_or(name);
    if let Some(selection) = selected {
      if !selection.contains_key(short) {
        continue;
      }
      w.entry(&format!("storage/{short}.jsonl"), false)?;
    }
    visit(&reader, *key, selected.and_then(|s| s.get(short)), |pos, value| {
      if short == "entries" && !value.is_empty() {
        entries += 1;
      }
      row(w, json!({"list":name,"position":pos}), value)
    })?;
  }
  w.manifest["counts"]["entries"] = json!(entries);
  w.manifest["bytes"]["storage"] = json!(w.original - storage_start);
  if include_transcript {
    w.entry("transcript.md", false)?;
    w.bytes(b"# Session transcript\n")?;
    if let Some((key, _, _, _)) = lists.iter().find(|(_, n, _, _)| n.ends_with("/entries")) {
      visit(&reader, *key, selected.and_then(|s| s.get("entries")), |_, value| {
        transcript(w, value)
      })?;
    }
  }
  Ok(())
}
fn visit(
  reader: &ArchiveReader,
  list: i64,
  selected: Option<&HashSet<u64>>,
  mut emit: impl FnMut(u64, &[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
  let mut scan = |mut start: u64, end: u64| -> Result<(), Error> {
    while start <= end {
      let page = reader.items(list, start, end)?;
      if page.is_empty() {
        break;
      }
      for (pos, raw) in page {
        emit(pos, &raw)?;
        start = pos + 1;
      }
    }
    Ok(())
  };
  if let Some(ids) = selected {
    let mut positions: Vec<_> = ids.iter().copied().collect();
    positions.sort_unstable();
    let mut ids = positions.into_iter().peekable();
    while let Some(first) = ids.next() {
      let mut last = first;
      while ids.next_if(|&id| id == last + 1).is_some() {
        last += 1;
      }
      scan(first, last)?;
    }
    Ok(())
  } else {
    scan(0, i64::MAX as u64)
  }
}
pub(crate) fn pack(
  app: &App,
  id: &str,
  mut record: Value,
  include: Include,
  export: bool,
) -> Result<Value, Error> {
  let name = record["session"]["name"].as_str().unwrap_or(id);
  let mut w = Writer::new(app, "session", Some(id), name, None, export)?;
  for shell in ["shell_command", "shell_override"] {
    if let Some(vars) = record["session"]
      .get_mut(shell)
      .and_then(|cmd| cmd.get_mut("env"))
      .and_then(Value::as_object_mut)
    {
      for val in vars.values_mut() {
        *val = json!("<redacted>");
      }
    }
  }
  let placement = app.management.archive_placement(id)?;
  for field in ["folder", "pinned", "folder_path"] {
    record[field] = placement[field].clone();
  }
  for k in ["provider", "created_at", "updated_at"] {
    w.manifest[k] = record["session"][k].clone();
  }
  w.manifest["folder_path"] = placement["folder_path"].clone();
  w.manifest["model"] = record["status"]["config"]["model"].clone();
  w.manifest["included"] = json!({"blobs":include.include_blobs,"shell":include.include_shell,"transcript":include.include_transcript});
  w.json("management.json", &record)?;
  records(app, id, &mut w, None, include.include_transcript)?;
  if include.include_blobs {
    w.tree(&app.data_dir.blobs(id), Path::new(""), "blobs")?;
  }
  if include.include_shell {
    w.tree(&app.data_dir.shell(id), Path::new(""), "shell")?;
  }
  w.finish()
}
