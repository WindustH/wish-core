//! Releasing the history the context no longer uses.
use super::{Entry, HistoryItem, HistoryRecord, SessionEvent};
use crate::protocol::Message;
use crate::session::persistence::{SessionRecord, SessionTransaction};
use crate::session::{EntryId, Session, SessionError, SessionState};
use crate::utils::time::Timestamp;
use std::collections::{HashMap, HashSet};

/// Names the resources a message uses, such as files kept beside the session.
type References = std::sync::Arc<dyn Fn(&Message, &mut HashSet<String>) + Send + Sync>;

/// What pruning released, or would release.
#[derive(Clone, Debug, Default)]
pub struct HistoryPrune {
  /// Messages released.
  pub messages: u64,
  pub selection: HashMap<String, HashSet<u64>>,
  /// Events released.
  pub events: u64,
  /// Stored bytes released: the values and the text indexed for history search.
  pub bytes: u64,
  /// Resources only released messages used, as `References` names them. Nothing kept uses them.
  pub resources: HashSet<String>,
}

impl Session {
  /// Releases the history the context no longer uses: messages outside the active and standby
  /// contexts that are not waiting in the queue, every event but a suspended run's outcome, and
  /// the history records and search index of both. With `before`, only what was recorded earlier
  /// goes. Released positions keep their numbers, so entry IDs and history sequences still mean
  /// what they meant.
  ///
  /// With `apply` false only measure the selected rows.
  pub(crate) fn prune_history_while_frozen(
    &mut self,
    before: Option<Timestamp>,
    apply: bool,
    references: References,
    expected: Option<HashMap<String, HashSet<u64>>>,
  ) -> Result<HistoryPrune, SessionError> {
    self.require_stable()?;
    let prune = move |transaction: &mut SessionTransaction<'_, '_>| {
      transaction.prune_history(before, references, expected, apply)
    };
    if !apply {
      return self.read(prune);
    }
    // Only list values and their index change. The caller holds the publication guard from
    // planning through the final commit, so acquiring the queue lock again would deadlock.
    let mut record = self.record.clone();
    let key = self.key.clone();
    let recorded_at = Timestamp::now();
    self.storage.transaction(move |store| {
      prune(&mut SessionTransaction { record: &mut record, store, key: &key, recorded_at })
    })
  }
}

/// What a prune selected.
struct Released {
  /// Entry IDs.
  entries: Vec<u64>,
  /// Event IDs.
  events: Vec<u64>,
  /// The history sequences of both.
  sequences: Vec<u64>,
  /// See [`HistoryPrune::resources`].
  resources: HashSet<String>,
}

impl SessionTransaction<'_, '_> {
  fn prune_history(
    &mut self,
    before: Option<Timestamp>,
    references: References,
    expected: Option<HashMap<String, HashSet<u64>>>,
    apply: bool,
  ) -> Result<HistoryPrune, SessionError> {
    let old = |at: Timestamp| before.is_none_or(|before| at < before);
    let used = self.used_entries()?;
    let released = self.select_released(&used, old, references)?;
    let selection = HashMap::from([
      ("entries".to_owned(), released.entries.iter().copied().collect()),
      ("events".to_owned(), released.events.iter().copied().collect()),
      ("history".to_owned(), released.sequences.iter().copied().collect()),
    ]);
    if expected.is_some_and(|plan| plan != selection) {
      return Err(SessionError::Busy);
    }
    let record = &self.record;
    let mut bytes =
      self.store.prune_history_records(&record.history, &released.sequences, apply)?;
    bytes +=
      self.store.release_items::<HistoryRecord>(&record.history, &released.sequences, apply)?;
    bytes += self.store.release_items::<Entry>(&record.entries, &released.entries, apply)?;
    bytes += self.store.release_items::<SessionEvent>(&record.events, &released.events, apply)?;
    Ok(HistoryPrune {
      selection,
      messages: released.entries.len() as u64,
      events: released.events.len() as u64,
      bytes,
      resources: released.resources,
    })
  }
  /// What goes: the entries nothing uses and the events, except a suspended run's outcome, that
  /// `old` accepts the time of, with their history records; and the resources only those entries
  /// name.
  fn select_released(
    &mut self,
    used: &HashSet<usize>,
    old: impl Fn(Timestamp) -> bool,
    references: References,
  ) -> Result<Released, SessionError> {
    let (mut released, mut kept) = (HashSet::new(), HashSet::new());
    let mut entries = Vec::new();
    let list = self.record.entries.clone();
    self.store.for_each_item::<Entry, SessionError>(&list, 0, |_, entry| {
      if !used.contains(&entry.id.0) && old(entry.recorded_at) {
        entries.push(entry.id.0 as u64);
        references(&entry.message, &mut released);
      } else {
        references(&entry.message, &mut kept);
      }
      Ok(())
    })?;
    let kept_outcome = match &self.record.state {
      SessionState::Suspended { outcome } => Some(*outcome),
      _ => None,
    };
    let released_entries: HashSet<u64> = entries.iter().copied().collect();
    let (mut events, mut sequences) = (Vec::new(), Vec::new());
    let list = self.record.history.clone();
    let event_list = self.record.events.clone();
    self.store.for_each_item::<HistoryRecord, SessionError>(&list, 0, |store, record| {
      match record.item {
        HistoryItem::Message(id) if released_entries.contains(&(id.0 as u64)) => {
          sequences.push(record.sequence);
        }
        HistoryItem::Event(id) if old(record.recorded_at) && Some(id) != kept_outcome => {
          if let Some(event) = store.get_item::<SessionEvent>(&event_list, id.0)?
            && let Ok(value) = serde_json::to_value(event)
          {
            super::super::resources::collect(&value, &mut released);
          }
          events.push(id.0);
          sequences.push(record.sequence);
        }
        HistoryItem::Event(id) => {
          if let Some(event) = store.get_item::<SessionEvent>(&event_list, id.0)?
            && let Ok(value) = serde_json::to_value(event)
          {
            super::super::resources::collect(&value, &mut kept);
          }
        }
        _ => {}
      }
      Ok(())
    })?;
    let resources = released.difference(&kept).cloned().collect();
    Ok(Released { entries, events, sequences, resources })
  }
}

impl<R: std::ops::Deref<Target = SessionRecord>> SessionTransaction<'_, '_, R> {
  /// The entries the context uses: both generations', and the input still waiting.
  pub(in crate::session) fn used_entries(&mut self) -> Result<HashSet<usize>, SessionError> {
    let mut lists = Vec::new();
    for id in [self.record.active, self.record.standby] {
      lists.push((self.load_generation(id)?.entries, 0));
    }
    lists.push((self.record.queue.clone(), self.record.queue_head));
    let mut used = HashSet::new();
    for (list, start) in lists {
      self.store.for_each_item::<EntryId, SessionError>(&list, start, |_, id| {
        used.insert(id.0);
        Ok(())
      })?;
    }
    Ok(used)
  }
}
