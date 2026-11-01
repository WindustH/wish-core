//! Private subprocess entry point. Rust codecs run outside the server's allocator/runtime;
//! the parent owns concurrency, output limits, cancellation and a hard wall-clock timeout.
use image::{ImageDecoder, ImageEncoder};
use lopdf::{Document, Object, dictionary};
use std::{
  collections::BTreeSet,
  io::{Read, Write},
  path::Path,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(crate) fn run() -> Result<()> {
  #[cfg(unix)]
  {
    // Applied before parsing untrusted input; the worker has no Tokio thread pool.
    let memory =
      libc::rlimit { rlim_cur: 3 * 1024 * 1024 * 1024, rlim_max: 3 * 1024 * 1024 * 1024 };
    let cpu = libc::rlimit { rlim_cur: 30, rlim_max: 30 };
    unsafe {
      if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0
        || libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0
      {
        return Err(std::io::Error::last_os_error().into());
      }
    }
  }
  let mut args = std::env::args_os().skip(2);
  let kind = args.next().ok_or("missing worker operation")?;
  let path = args.next().ok_or("missing source")?;
  let number = args.next().and_then(|s| s.to_str().and_then(|s| s.parse::<u32>().ok()));
  let path = Path::new(&path);
  let mut out = Bounded { inner: std::io::stdout().lock(), remaining: 64 * 1024 * 1024 };
  match kind.to_str() {
    Some("image") => image(path, number.ok_or("missing width")?, &mut out),
    Some("pdf-info") => {
      out.remaining = 2 * 1024 * 1024;
      let doc = load_pdf(path)?;
      let pages = doc.get_pages().len();
      if pages == 0 {
        return Err("PDF has no pages".into());
      }
      let bookmarks: Vec<_> = doc.get_toc().unwrap_or_default().toc.into_iter().take(10000)
        .map(|entry| serde_json::json!({"title":entry.title,"depth":entry.level.saturating_sub(1),"page":entry.page})).collect();
      serde_json::to_writer(out, &serde_json::json!({"pages":pages,"bookmarks":bookmarks}))?;
      Ok(())
    }
    Some("pdf-page") => page(load_pdf(path)?, number.ok_or("missing page")?, &mut out),
    _ => Err("unknown worker operation".into()),
  }
}
struct Bounded<W> {
  inner: W,
  remaining: usize,
}
impl<W: Write> Write for Bounded<W> {
  fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
    if bytes.len() > self.remaining {
      return Err(std::io::Error::other("Preview exceeds output limit"));
    }
    let written = self.inner.write(bytes)?;
    self.remaining -= written;
    Ok(written)
  }
  fn flush(&mut self) -> std::io::Result<()> {
    self.inner.flush()
  }
}
fn image(path: &Path, width: u32, out: &mut impl Write) -> Result<()> {
  if ![384, 768, 1536].contains(&width) {
    return Err("invalid image width".into());
  }
  let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
  let mut limits = image::Limits::default();
  limits.max_alloc = Some(256 * 1024 * 1024);
  limits.max_image_width = Some(32768);
  limits.max_image_height = Some(32768);
  reader.limits(limits);
  let mut decoder = reader.into_decoder()?;
  if decoder.total_bytes() > 256 * 1024 * 1024 {
    return Err("image dimensions exceed preview limit".into());
  }
  let orientation = decoder.orientation()?;
  let mut image = image::DynamicImage::from_decoder(decoder)?;
  image.apply_orientation(orientation);
  let image =
    if image.width().max(image.height()) > width { image.thumbnail(width, width) } else { image };
  let image = image.to_rgba8();
  image::codecs::webp::WebPEncoder::new_lossless(out).write_image(
    image.as_raw(),
    image.width(),
    image.height(),
    image::ExtendedColorType::Rgba8,
  )?;
  Ok(())
}
fn load_pdf(path: &Path) -> Result<Document> {
  const MAX: u64 = 1024 * 1024 * 1024;
  let file = std::fs::File::open(path)?;
  if file.metadata()?.len() > MAX {
    return Err("PDF exceeds preview source limit".into());
  }
  let mut bytes = Vec::new();
  file.take(MAX + 1).read_to_end(&mut bytes)?;
  if bytes.len() as u64 > MAX {
    return Err("PDF exceeds preview source limit".into());
  }
  Ok(Document::load_mem(&bytes)?)
}
fn page(mut doc: Document, number: u32, out: &mut impl Write) -> Result<()> {
  let pages = doc.get_pages();
  let id = *pages.get(&number).ok_or("PDF page out of range")?;
  let mut page = doc.get_dictionary(id)?.clone();
  // Flatten inherited page attributes before detaching the original page tree.
  let mut parent = page.get(b"Parent").and_then(Object::as_reference).ok();
  let mut visited = BTreeSet::new();
  while let Some(id) = parent {
    if !visited.insert(id) || visited.len() > 128 {
      return Err("invalid PDF page tree".into());
    }
    let ancestor = doc.get_dictionary(id)?;
    for key in [b"Resources".as_slice(), b"MediaBox", b"CropBox", b"Rotate"] {
      if !page.has(key)
        && let Ok(value) = ancestor.get(key)
      {
        page.set(key, value.clone());
      }
    }
    parent = ancestor.get(b"Parent").and_then(Object::as_reference).ok();
  }
  prune_page_images(&doc, id, &mut page);
  // Preserve static annotation appearances, but remove actions/back references that
  // could drag other pages, scripts or the original catalog into the single-page file.
  let annotations = doc
    .get_page_annotations(id)
    .unwrap_or_default()
    .into_iter()
    .map(|annotation| {
      let mut copy = annotation.clone();
      for key in [b"P".as_slice(), b"Parent", b"Dest", b"A", b"AA", b"Popup", b"IRT"] {
        copy.remove(key);
      }
      Object::Dictionary(copy)
    })
    .collect::<Vec<_>>();
  page.set("Annots", annotations);
  for key in [b"AA".as_slice(), b"B", b"Metadata", b"PieceInfo", b"StructParents", b"PresSteps"] {
    page.remove(key);
  }
  let tree = doc.new_object_id();
  page.set("Parent", tree);
  doc.objects.insert(id, Object::Dictionary(page));
  doc.objects.insert(
    tree,
    Object::Dictionary(
      dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(id)], "Count" => 1 },
    ),
  );
  let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => tree });
  doc.trailer = dictionary! { "Root" => root };
  // Even unusual resource/annotation graphs must not retain another page tree.
  for other in pages.values().filter(|other| **other != id) {
    doc.objects.remove(other);
  }
  doc.prune_objects();
  doc.renumber_objects();
  doc.save_to(out)?;
  Ok(())
}

// Some scanners put every page image in one inherited Resources dictionary. Copy
// only directly used images when the page has no nested graphics that can inherit
// that dictionary. Complex graphics keep their resource graph intact.
fn prune_page_images(doc: &Document, id: lopdf::ObjectId, page: &mut lopdf::Dictionary) {
  if page.has(b"Annots") {
    return;
  }
  let Some(mut resources) = page
    .get(b"Resources")
    .ok()
    .and_then(|value| doc.dereference(value).ok())
    .and_then(|(_, value)| value.as_dict().ok())
    .cloned()
  else {
    return;
  };
  if resources.has(b"Pattern") || resources.has(b"ExtGState") {
    return;
  }
  let Some(objects) = resources
    .get(b"XObject")
    .ok()
    .and_then(|value| doc.dereference(value).ok())
    .and_then(|(_, value)| value.as_dict().ok())
  else {
    return;
  };
  if objects.iter().any(|(_, value)| {
    doc
      .dereference(value)
      .ok()
      .and_then(|(_, value)| value.as_stream().ok())
      .and_then(|stream| stream.dict.get(b"Subtype").ok())
      .and_then(|value| value.as_name().ok())
      != Some(b"Image".as_slice())
  }) {
    return;
  }
  let Ok(content) = doc.get_page_content_with_limit(id, 16 * 1024 * 1024) else {
    return;
  };
  let Ok(content) = lopdf::content::Content::decode(&content) else {
    return;
  };
  let used = content
    .operations
    .iter()
    .filter(|operation| operation.operator == "Do")
    .filter_map(|operation| operation.operands.first().and_then(|value| value.as_name().ok()))
    .collect::<BTreeSet<_>>();
  let mut retained = lopdf::Dictionary::new();
  for (name, value) in objects.iter() {
    if used.contains(name.as_slice()) {
      retained.set(name.clone(), value.clone());
    }
  }
  resources.set("XObject", retained);
  page.set("Resources", resources);
}
