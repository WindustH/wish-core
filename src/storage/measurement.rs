//! Database space attribution, independent of application and HTTP response types.
use super::DatabaseShape;
use serde::Serialize;
#[derive(Serialize)]
pub struct StorageCategory {
  pub kind: String,
  pub bytes: u64,
}
/// What a database holds, by kind, in order: each kind's bytes. `groups` names the kinds and the
/// tables and indexes that make them up, by a part of their names; `items` splits the engine's item
/// table by what its lists hold. What belongs to no kind is `other`; free pages and the journal -
/// what the files take beyond the database's pages: the write-ahead log and shared memory - close
/// the list, so the kinds add up to `on_disk`, the files' bytes.
pub fn database_breakdown(
  shape: &DatabaseShape,
  groups: &[(&str, &[&str])],
  items: &[(&str, &[&str])],
  on_disk: u64,
) -> Vec<StorageCategory> {
  let mut rows: Vec<(&str, u64)> = Vec::new();
  let mut counted = 0u64;
  for (kind, names) in groups {
    let bytes: u64 = shape
      .tables
      .iter()
      .filter(|(table, _)| names.iter().any(|name| table.contains(name)))
      .map(|(_, bytes)| bytes)
      .sum();
    counted += bytes;
    rows.push((kind, bytes));
  }
  // The item table is one b-tree: its pages are shared out by the bytes each kind's items take.
  if !items.is_empty() {
    let table = shape.tables.get("wish_items").copied().unwrap_or(0);
    let total: u64 = shape.items_by_kind.values().sum();
    counted += table;
    let (first, mut shared) = (rows.len(), 0);
    for (kind, list_kinds) in items {
      let payload: u64 = list_kinds.iter().filter_map(|name| shape.items_by_kind.get(*name)).sum();
      let bytes =
        if total == 0 { 0 } else { (table as u128 * payload as u128 / total as u128) as u64 };
      shared += bytes;
      rows.push((kind, bytes));
    }
    // What rounding left over goes to the first kind, so the kinds add up to the table.
    rows[first].1 += table - shared;
  }
  let free = shape.free_pages * shape.page_size;
  let pages = shape.pages * shape.page_size;
  rows.push(("other", pages.saturating_sub(counted + free)));
  rows.push(("free", free));
  rows.push(("journal", on_disk.saturating_sub(pages)));
  rows.into_iter().map(|(kind, bytes)| StorageCategory { kind: kind.to_owned(), bytes }).collect()
}
