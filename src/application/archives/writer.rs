//! Stream package entries and publish the verified ZIP before the caller releases any source.
use super::*;
use sha2::{Digest, Sha256};
use std::{
  fs::{self, File, OpenOptions},
  io::{self, Read, Seek, Write},
  path::{Path, PathBuf},
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

struct Temporary(PathBuf);
impl Drop for Temporary {
  fn drop(&mut self) {
    let _ = fs::remove_file(&self.0);
  }
}

/// ZIP headers may be backpatched. Limit the furthest file offset, including the central directory.
struct PackageFile {
  file: File,
  position: u64,
  limit: u64,
}
impl Write for PackageFile {
  fn write(&mut self, data: &[u8]) -> io::Result<usize> {
    if self.position.saturating_add(data.len() as u64) > self.limit {
      return Err(io::Error::other("export exceeds 4 GiB"));
    }
    let n = self.file.write(data)?;
    self.position += n as u64;
    Ok(n)
  }
  fn flush(&mut self) -> io::Result<()> {
    self.file.flush()
  }
}
impl Seek for PackageFile {
  fn seek(&mut self, from: io::SeekFrom) -> io::Result<u64> {
    self.position = self.file.seek(from)?;
    Ok(self.position)
  }
}
pub struct Writer {
  zip: ZipWriter<PackageFile>,
  temporary: Temporary,
  destination: PathBuf,
  pub manifest: Value,
  pub original: u64,
  blob_count: u64,
  task_count: u64,
  blob_bytes: u64,
  shell_bytes: u64,
  export: bool,
  stop: tokio_util::sync::CancellationToken,
}
impl Writer {
  pub fn new(
    app: &App,
    kind: &str,
    session: Option<&str>,
    name: &str,
    batch: Option<&str>,
    export: bool,
  ) -> Result<Self, Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let dir = app.data_dir.packages(export);
    fs::create_dir_all(&dir).map_err(Error::internal)?;
    let temp = dir.join(format!(".{id}.zip.tmp"));
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
      use std::os::unix::fs::OpenOptionsExt;
      options.mode(0o600);
    }
    let file = options.open(&temp).map_err(Error::internal)?;
    let now = crate::utils::time::Timestamp::now().0;
    let mut manifest = json!({"archive_version":1,"format_version":crate::migration::LATEST,"wish_version":env!("CARGO_PKG_VERSION"),"archive_id":id,"kind":kind,"disposition":if export{"export"}else{"archive"},"session_id":session,"batch_id":batch,"name":name,"archived_at":now,
    "omitted":{"temp_dir":true,"history_search_index":true,"external_link_targets":true,"shell_env_secrets":true},"external_links":[]});
    if export {
      manifest["expires_at"] = json!(now + 30 * 60 * 1000);
    }
    Ok(Self {
      zip: ZipWriter::new(PackageFile {
        file,
        position: 0,
        limit: if export { 4 << 30 } else { u64::MAX },
      }),
      export,
      temporary: Temporary(temp),
      destination: dir.join(format!("{id}.zip")),
      manifest,
      original: 0,
      blob_count: 0,
      task_count: 0,
      blob_bytes: 0,
      shell_bytes: 0,
      stop: app.lifecycle.stop.clone(),
    })
  }
  fn check(&self) -> Result<(), Error> {
    if self.stop.is_cancelled() { Err(Error::shutting_down()) } else { Ok(()) }
  }
  pub fn entry(&mut self, name: &str, stored: bool) -> Result<(), Error> {
    self.check()?;
    let options = SimpleFileOptions::default().large_file(true);
    let options = if stored {
      options.compression_method(CompressionMethod::Stored)
    } else {
      options.compression_method(CompressionMethod::Deflated).compression_level(Some(3))
    };
    self.zip.start_file(name, options).map_err(Error::internal)
  }
  pub fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
    self.check()?;
    self.zip.write_all(bytes).map_err(Error::internal)?;
    self.original += bytes.len() as u64;
    Ok(())
  }
  pub fn json(&mut self, name: &str, value: &Value) -> Result<(), Error> {
    self.entry(name, false)?;
    self.bytes(&serde_json::to_vec(value).map_err(Error::internal)?)
  }
  pub fn tree(&mut self, root: &Path, relative: &Path, prefix: &str) -> Result<(), Error> {
    self.check()?;
    let path = root.join(relative);
    let meta = match fs::symlink_metadata(&path) {
      Ok(m) => m,
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
      Err(e) => return Err(Error::internal(e)),
    };
    if meta.is_symlink() {
      return Ok(());
    }
    if meta.is_dir() {
      for child in fs::read_dir(path).map_err(Error::internal)? {
        let name = child.map_err(Error::internal)?.file_name();
        let name =
          name.to_str().ok_or_else(|| Error::invalid("archive requires UTF-8 filenames"))?;
        if name.contains(['\\', '\0']) {
          return Err(Error::invalid("unsafe archive filename"));
        }
        if name.ends_with(".tmp") || name.starts_with("terminal-login-") && name.ends_with(".sh") {
          continue;
        }
        self.tree(root, &relative.join(name), prefix)?;
      }
    } else if meta.is_file() {
      let mut file = open_owned(root, relative).map_err(Error::internal)?;
      let len = file.metadata().map_err(Error::internal)?.len();
      if prefix == "blobs" {
        self.blob_bytes += len;
        if relative.parent() == Some(Path::new(""))
          && relative.to_str().is_some_and(crate::storage::blobs::is_blob_id)
        {
          self.blob_count += 1;
        }
      } else if prefix == "shell" {
        self.shell_bytes += len;
        if relative.components().count() == 2
          && relative.file_name().is_some_and(|name| name == "task.json")
        {
          self.task_count += 1;
        }
      }
      let mut buf = vec![0; 65536];
      let count = file.read(&mut buf[..512]).map_err(Error::internal)?;
      let mime = crate::storage::blobs::sniff_mime_type(&buf[..count]);
      let stored = mime.starts_with("audio/")
        || mime.starts_with("video/")
        || (mime.starts_with("image/") && mime != "image/svg+xml")
        || buf[..count].starts_with(b"%PDF")
        || buf[..count].starts_with(b"PK\x03\x04");
      let name = format!(
        "{prefix}/{}",
        relative
          .to_str()
          .ok_or_else(|| Error::invalid("invalid archive filename"))?
          .replace(std::path::MAIN_SEPARATOR, "/")
      );
      if prefix == "blobs" && len <= 65536 && relative.extension().is_some_and(|ext| ext == "json")
      {
        file.rewind().map_err(Error::internal)?;
        file.read_exact(&mut buf[..len as usize]).map_err(Error::internal)?;
        if let Ok(blob) =
          serde_json::from_slice::<crate::storage::blobs::Blob>(&buf[..len as usize])
          && blob.linked
          && let Some(links) = self.manifest["external_links"].as_array_mut()
        {
          links.push(json!({"id":blob.id,"path":blob.path}));
        }
      }
      if prefix == "blobs" && relative == Path::new(".library.json") {
        file.rewind().map_err(Error::internal)?;
        if len > 16 * 1024 * 1024 {
          return Err(Error::invalid("oversized Library metadata"));
        }
        let metadata: Value =
          serde_json::from_reader((&mut file).take(len)).map_err(Error::internal)?;
        if let (Some(folders), Some(links)) =
          (metadata["folders"].as_object(), self.manifest["external_links"].as_array_mut())
        {
          for (id, folder) in folders {
            if let Some(path) = folder["linked_path"].as_str() {
              links.push(json!({"id":id,"path":path,"kind":"folder"}));
            }
          }
        }
      }
      self.entry(&name, stored)?;
      file.rewind().map_err(Error::internal)?;
      let mut left = len;
      while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..n]).map_err(Error::internal)?;
        if n == 0 {
          if !self.export {
            return Err(Error::conflict("archive source changed while reading"));
          }
          if prefix == "shell" {
            self.shell_bytes -= left;
          } else if prefix == "blobs" {
            self.blob_bytes -= left;
          }
          break;
        }
        self.bytes(&buf[..n])?;
        left -= n as u64;
      }
    }
    Ok(())
  }
  pub fn finish(mut self) -> Result<Value, Error> {
    self.manifest["counts"]["blobs"] = json!(self.blob_count);
    self.manifest["counts"]["shell_tasks"] = json!(self.task_count);
    self.manifest["counts"]["external_links"] =
      json!(self.manifest["external_links"].as_array().map_or(0, Vec::len));
    self.manifest["bytes"]["blobs"] = json!(self.blob_bytes);
    self.manifest["bytes"]["shell"] = json!(self.shell_bytes);
    self.manifest["original_bytes"] = json!(self.original);
    let manifest = self.manifest.clone();
    self.json("manifest.json", &manifest)?;
    let mut file = self.zip.finish().map_err(Error::internal)?.file;
    file.sync_all().map_err(Error::internal)?;
    let len = file.metadata().map_err(Error::internal)?.len();
    file.rewind().map_err(Error::internal)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
      if self.stop.is_cancelled() {
        return Err(Error::shutting_down());
      }
      let n = file.read(&mut buf).map_err(Error::internal)?;
      if n == 0 {
        break;
      }
      hash.update(&buf[..n]);
    }
    let mut archive = ZipArchive::new(file).map_err(Error::internal)?;
    // Before releasing source data, verify every entry CRC. Exports retain the source.
    if !self.export {
      for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(Error::internal)?;
        while entry.read(&mut buf).map_err(Error::internal)? > 0 {
          if self.stop.is_cancelled() {
            return Err(Error::shutting_down());
          }
        }
      }
    }
    drop(archive);
    fs::rename(&self.temporary.0, &self.destination).map_err(Error::internal)?;
    #[cfg(unix)]
    File::open(self.destination.parent().expect("archive directory"))
      .and_then(|d| d.sync_all())
      .map_err(Error::internal)?;
    self.manifest["archive_bytes"] = json!(len);
    self.manifest["sha256"] = json!(hex::encode(hash.finalize()));
    Ok(self.manifest)
  }
}

// Anchor each path component to the preceding open directory. A concurrent rename
// cannot redirect an archive read through a link to a file outside the session.
#[cfg(unix)]
fn open_owned(root: &Path, relative: &Path) -> io::Result<File> {
  use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::OpenOptionsExt},
  };
  let mut dir =
    OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY).open(root)?;
  let mut parts = relative.components().peekable();
  while let Some(std::path::Component::Normal(part)) = parts.next() {
    let name = std::ffi::CString::new(part.as_bytes())?;
    let flags = libc::O_RDONLY
      | libc::O_NOFOLLOW
      | libc::O_NONBLOCK
      | libc::O_CLOEXEC
      | if parts.peek().is_some() { libc::O_DIRECTORY } else { 0 };
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
      return Err(io::Error::last_os_error());
    }
    dir = unsafe { File::from_raw_fd(fd) };
  }
  if !dir.metadata()?.is_file() {
    return Err(io::Error::other("archive input changed type"));
  }
  Ok(dir)
}
#[cfg(not(unix))]
fn open_owned(root: &Path, relative: &Path) -> io::Result<File> {
  let path = root.join(relative);
  if !path.canonicalize()?.starts_with(root.canonicalize()?) {
    return Err(io::Error::other("archive input outside owner directory"));
  }
  File::open(path)
}

pub(super) fn digest(path: &Path) -> Result<(String, u64), Error> {
  let mut file = File::open(path).map_err(Error::internal)?;
  let mut hasher = Sha256::new();
  let count = io::copy(&mut file, &mut hasher).map_err(Error::internal)?;
  Ok((hex::encode(hasher.finalize()), count))
}
pub(super) fn read_manifest(path: &Path, id: &str) -> Result<Value, Error> {
  let mut zip =
    ZipArchive::new(File::open(path).map_err(Error::internal)?).map_err(Error::internal)?;
  let entry = zip.by_name("manifest.json").map_err(Error::internal)?;
  let mut data = Vec::new();
  entry.take(16 * 1024 * 1024 + 1).read_to_end(&mut data).map_err(Error::internal)?;
  if data.len() > 16 * 1024 * 1024 {
    return Err(Error::internal("oversized archive manifest"));
  }
  let mut m: Value = serde_json::from_slice(&data).map_err(Error::internal)?;
  if m["archive_id"] != id || m["archive_version"] != 1 || m["disposition"] != "archive" {
    return Err(Error::internal("invalid archive manifest"));
  }
  // An unindexed but complete package is recoverable. Adopting it never replays
  // its original prune or session deletion against data that may have changed.
  let (hash, len) = digest(path)?;
  m["sha256"] = json!(hash);
  m["archive_bytes"] = json!(len);
  Ok(m)
}
