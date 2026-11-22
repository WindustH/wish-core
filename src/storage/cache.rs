use lru::LruCache;
use std::{any::Any, sync::Arc};

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum CacheKey {
  Object(String),
  Item(String, u64),
  Page(String, u64),
}
#[derive(Clone)]
pub(crate) struct CachedValue {
  pub value: Arc<dyn Any + Send + Sync>,
  pub weight: u64,
}

/// Used only on the storage thread. Byte accounting includes a small per-entry estimate.
pub(crate) struct Cache {
  values: LruCache<CacheKey, CachedValue>,
  capacity: u64,
  used: u64,
}
impl Cache {
  pub fn new(capacity: u64) -> Self {
    Self { values: LruCache::unbounded(), capacity, used: 0 }
  }
  pub fn get(&mut self, key: &CacheKey) -> Option<CachedValue> {
    self.values.get(key).cloned()
  }
  pub fn insert(&mut self, key: CacheKey, value: CachedValue) {
    self.invalidate(&key);
    if value.weight > self.capacity {
      return;
    }
    while self.used > self.capacity - value.weight {
      let Some((_, removed)) = self.values.pop_lru() else { break };
      self.used -= removed.weight;
    }
    self.used += value.weight;
    self.values.put(key, value);
  }
  pub fn invalidate(&mut self, key: &CacheKey) {
    if let Some(value) = self.values.pop(key) {
      self.used -= value.weight;
    }
  }
  pub fn invalidate_namespace(&mut self, namespace: &str, prefix: &str) {
    let keys: Vec<_> = self
      .values
      .iter()
      .filter_map(|(key, _)| {
        let name = match key {
          CacheKey::Object(name) | CacheKey::Item(name, _) | CacheKey::Page(name, _) => name,
        };
        (name == namespace || name.starts_with(prefix)).then(|| key.clone())
      })
      .collect();
    for key in keys {
      self.invalidate(&key);
    }
  }
}
