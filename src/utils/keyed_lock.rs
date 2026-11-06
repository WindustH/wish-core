//! Serialize work on one resource without keeping inactive resources alive in a global map.
use std::{
  collections::HashMap,
  hash::Hash,
  sync::{Arc, Mutex, Weak},
};

pub struct KeyedLocks<K>(Mutex<HashMap<K, Weak<Mutex<()>>>>);
impl<K: Eq + Hash> Default for KeyedLocks<K> {
  fn default() -> Self {
    Self(Mutex::new(HashMap::new()))
  }
}
impl<K: Eq + Hash> KeyedLocks<K> {
  pub fn get(&self, key: K) -> Arc<Mutex<()>> {
    let mut locks = self.0.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
      return lock;
    }
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
  }
}
