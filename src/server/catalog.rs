//! Server-side caching of model-catalog pages.
//!
//! A catalog page is the same answer to the same question for minutes at a time: the set of models
//! a service offers changes on the service's schedule, not the caller's. This module keeps the
//! pages one provider has already read, so a picker opened again - and every consumer walking the
//! same cursor chain - reads them from memory instead of the service.
//!
//! Rules that run through the whole module:
//!
//! - A page is cached under everything that shapes its call: host, path, cursor, page size and the
//!   unauthenticated flag. The protocol is fixed by the provider's own client, and a provider
//!   rebuilt after a configuration change starts with an empty cache, so no entry outlives the
//!   settings it was read under.
//! - A page is fresh for [`FRESH_FOR`]. An expired page is refetched on the next read, not swept
//!   in the background: nothing here wakes on its own.
//! - A failed refetch serves the expired page with a warning naming its age, so an unreachable
//!   upstream degrades to a stale catalog rather than an error. The entry is kept for the retry.
//! - One fetch runs per provider at a time. A second caller for the same page waits, then finds
//!   the answer in the cache; concurrent walks of one catalog interleave page by page instead of
//!   fetching the same pages twice.

use crate::protocol::model_list::{ModelCatalog, ModelListQuery};
use crate::server::provider::ModelClient;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a read page answers without asking the service again.
const FRESH_FOR: Duration = Duration::from_secs(5 * 60);

/// Everything of a catalog call that its answer depends on.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
  base_url: String,
  path: String,
  cursor: Option<String>,
  page_size: u32,
  unauthenticated: bool,
}

impl Key {
  fn of(query: &ModelListQuery) -> Self {
    Self {
      base_url: query.base_url.clone(),
      path: query.path.clone(),
      cursor: query.cursor.clone(),
      page_size: query.page_size,
      unauthenticated: query.unauthenticated,
    }
  }
}

/// One page and when it was read.
struct Entry {
  page: ModelCatalog,
  fetched_at: Instant,
}

/// The catalog pages one provider has read, with one fetch in flight at a time.
#[derive(Default)]
pub struct CatalogCache {
  pages: Mutex<HashMap<Key, Entry>>,
  fetching: tokio::sync::Mutex<()>,
}

impl CatalogCache {
  /// The page a query asks for: from memory while fresh, refetched when not, and the expired
  /// page with a warning when the refetch fails.
  ///
  /// # Errors
  ///
  /// The refetch's own error, when no page was ever read for the query it describes.
  pub async fn page(
    &self,
    client: &ModelClient,
    query: &ModelListQuery,
  ) -> Result<ModelCatalog, crate::Error> {
    let key = Key::of(query);
    if let Some(page) = self.fresh(&key, Instant::now()) {
      return Ok(page);
    }
    let _one_fetch_at_a_time = self.fetching.lock().await;
    if let Some(page) = self.fresh(&key, Instant::now()) {
      return Ok(page);
    }
    match client.get_model_list(query).await {
      Ok(page) => {
        self
          .pages
          .lock()
          .unwrap()
          .insert(key, Entry { page: page.clone(), fetched_at: Instant::now() });
        Ok(page)
      }
      Err(error) => match self.stale(&key) {
        Some(page) => Ok(page),
        None => Err(error),
      },
    }
  }

  /// The page under a key, while it is still fresh to read at `now`.
  fn fresh(&self, key: &Key, now: Instant) -> Option<ModelCatalog> {
    let pages = self.pages.lock().unwrap();
    match pages.get(key) {
      Some(entry) if is_fresh(entry.fetched_at, now) => Some(entry.page.clone()),
      _ => None,
    }
  }

  /// The page under a key whatever its age, with a warning naming how old it is.
  fn stale(&self, key: &Key) -> Option<ModelCatalog> {
    let pages = self.pages.lock().unwrap();
    let entry = pages.get(key)?;
    let mut page = entry.page.clone();
    page.warnings.push(format!(
      "served from cache after an upstream error (page read {} seconds ago)",
      entry.fetched_at.elapsed().as_secs()
    ));
    Some(page)
  }
}

/// Whether a page read at `fetched_at` still answers at `now`.
fn is_fresh(fetched_at: Instant, now: Instant) -> bool {
  now.duration_since(fetched_at) < FRESH_FOR
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::protocol::model_list::{Model, ModelListProtocol};

  fn catalog(id: &str) -> ModelCatalog {
    ModelCatalog {
      protocol: ModelListProtocol::OpenAiModels,
      models: vec![Model {
        id: id.to_owned(),
        name: None,
        owner: None,
        created_at: None,
        context_window: None,
        max_output_tokens: None,
      }],
      next_cursor: None,
      warnings: Vec::new(),
    }
  }

  fn query(cursor: Option<&str>) -> ModelListQuery {
    ModelListQuery {
      base_url: "https://example.test".to_owned(),
      path: "/models".to_owned(),
      cursor: cursor.map(str::to_owned),
      page_size: 100,
      unauthenticated: false,
    }
  }

  #[test]
  fn a_page_is_fresh_for_its_window_and_no_longer() {
    let read_at = Instant::now();
    assert!(is_fresh(read_at, read_at + FRESH_FOR - Duration::from_secs(1)));
    assert!(!is_fresh(read_at, read_at + FRESH_FOR));
    assert!(!is_fresh(read_at, read_at + FRESH_FOR + Duration::from_secs(1)));
  }

  #[test]
  fn a_page_answers_only_under_its_own_call() {
    let cache = CatalogCache::default();
    let key = Key::of(&query(None));
    cache
      .pages
      .lock()
      .unwrap()
      .insert(key.clone(), Entry { page: catalog("m1"), fetched_at: Instant::now() });
    let now = Instant::now();
    assert!(cache.fresh(&key, now).is_some());
    assert!(cache.fresh(&Key::of(&query(Some("after-this"))), now).is_none());
    assert!(
      cache
        .fresh(&Key::of(&ModelListQuery { unauthenticated: true, ..query(None) }), now)
        .is_none()
    );
    assert!(cache.fresh(&Key::of(&ModelListQuery { page_size: 99, ..query(None) }), now).is_none());
  }

  #[test]
  fn an_expired_page_is_served_stale_with_its_age_named() {
    let cache = CatalogCache::default();
    let key = Key::of(&query(None));
    cache
      .pages
      .lock()
      .unwrap()
      .insert(key.clone(), Entry { page: catalog("m1"), fetched_at: Instant::now() });
    let later = Instant::now() + FRESH_FOR + Duration::from_secs(30);
    assert!(cache.fresh(&key, later).is_none());
    let served = cache.stale(&key).expect("the expired page is still there to serve");
    assert_eq!(served.models[0].id, "m1");
    assert!(served.warnings.iter().any(|warning| warning.starts_with("served from cache")));
    // Serving stale does not consume the entry: the retry starts from the same page.
    assert!(cache.stale(&key).is_some());
  }
}
