//! A local cutover's new context: standby plus the tail of the active context it has not
//! summarized, its oldest complete units removed until it fits the target.
use super::{Failure, check_cancelled, count_kept_prefix, measure, trigger::CutoverPlan};
use crate::executor::{ExecutionControl, model::ModelCaller};
use crate::{
  protocol::error::Error,
  protocol::{Request, model_use::context::find_replay_unit_boundaries},
  session::{CompactionReason, EntryId, GenerationId, Session, SessionError},
};

const MAX_COUNT_PROBES: usize = 8;

/// The context a local cutover starts from, before anything is removed.
struct Candidate {
  generation: GenerationId,
  /// The active generation's entries; a candidate that keeps them all changes nothing.
  original: Vec<EntryId>,
  entries: Vec<EntryId>,
  /// The first of `entries` still eligible for a summary.
  compaction_cursor: usize,
  /// `entries` as a request sends them.
  request: Request,
}

/// Standby plus the active tail it has not summarized, when the standby was built from the active
/// generation; the active context itself otherwise. `kept` is the prefix no summary covers.
fn build_candidate(
  session: &Session,
  mut request: Request,
  kept: usize,
) -> Result<Candidate, Failure> {
  let active = session.get_active_generation()?;
  let standby = session.get_standby_generation()?;
  let original = session.reader().read_generation_entry_ids(active.id)?;
  let (entries, compaction_cursor) = if let Some(end) = standby.summarized_end(&active) {
    let mut entries = session.reader().read_generation_entry_ids(standby.id)?;
    let compaction_cursor = entries.len();
    entries.extend_from_slice(&original[end as usize..]);
    (entries, compaction_cursor)
  } else {
    (original.clone(), (active.compaction_cursor as usize).max(kept))
  };
  request.conversation = entries
    .iter()
    .map(|id| {
      session
        .reader()
        .get_entry(*id)?
        .map(|entry| entry.message.clone())
        .ok_or(SessionError::InvalidEntry(*id))
    })
    .collect::<Result<Vec<_>, _>>()?;
  Ok(Candidate { generation: active.id, original, entries, compaction_cursor, request })
}

/// Switches to the smallest structurally complete candidate that fits `target_tokens`, keeping
/// the fixed prefix verbatim. A `Usage` or `Manual` cutover whose candidate already fits changes
/// nothing; a rejected context has to change.
pub(super) async fn trim_context(
  caller: &impl ModelCaller,
  session: &mut Session,
  control: &ExecutionControl,
  plan: CutoverPlan,
) -> Result<(), Failure> {
  let CutoverPlan { reason, request, config, calibration } = plan;
  let fixed = count_kept_prefix(&request.conversation);
  let Candidate { generation, original, entries, compaction_cursor, request: initial } =
    build_candidate(session, request, fixed)?;
  let boundaries = find_replay_unit_boundaries(&initial.conversation)?;
  let boundaries: Vec<_> = std::iter::once(fixed)
    .chain(boundaries.into_iter().filter(|end| *end > fixed && *end < initial.conversation.len()))
    .filter(|end| {
      !(matches!(reason, CompactionReason::ContextRejected) && *end == fixed && entries == original)
    })
    .collect();
  // Bounded binary search counts only complete, protocol-valid candidates. Keep an actual
  // fitting measurement; an estimate alone never authorizes a provider-counted cutover.
  let (mut low, mut high, mut probes) = (0, boundaries.len(), 0);
  let mut fitting = None;
  while low < high && probes < MAX_COUNT_PROBES {
    check_cancelled(control)?;
    // Reserve the final probe for the smallest valid tail if no fitting candidate is known.
    let final_probe = probes + 1 == MAX_COUNT_PROBES && fitting.is_none();
    let pivot = if final_probe { high - 1 } else { low + (high - low) / 2 };
    let mut index = pivot;
    let candidate = loop {
      check_cancelled(control)?;
      let mut request = initial.clone();
      request.conversation = initial.conversation[..fixed]
        .iter()
        .chain(&initial.conversation[boundaries[index]..])
        .cloned()
        .collect();
      if caller.validate_request(&request).is_ok() {
        break Some(request);
      }
      // Invalid candidates cost no network requests. Exhausting the right half leaves
      // the left half searchable instead of incorrectly skipping an earlier valid context.
      if final_probe {
        if index == low {
          break None;
        }
        index -= 1;
      } else {
        index += 1;
        if index == high {
          break None;
        }
      }
    };
    let Some(request) = candidate else {
      high = if final_probe { low } else { pivot };
      continue;
    };
    let remove_end = boundaries[index];
    probes += 1;
    let measurement = measure(caller, control, &config, &request, calibration).await?;
    if measurement.tokens > config.target_tokens {
      low = index + 1;
    } else {
      fitting = Some((remove_end, measurement));
      high = index;
    }
  }
  if let Some((remove_end, measurement)) = fitting {
    let kept: Vec<EntryId> =
      entries[..fixed].iter().chain(&entries[remove_end..]).copied().collect();
    if kept == original {
      return Ok(());
    }
    check_cancelled(control)?;
    session.commit_local_compaction(
      generation,
      kept,
      (fixed + compaction_cursor.saturating_sub(remove_end)) as u64,
      (remove_end - fixed) as u64,
      measurement,
      reason,
    )?;
    return Ok(());
  }
  Err(Error::Build("compaction target cannot accommodate the fixed prompt and a protocol-valid remaining context".into()).into())
}
