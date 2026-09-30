//! A local cutover's new context: standby plus the tail of the active context it has not
//! summarized, its oldest complete units removed until it fits the target.
use super::{Failure, check_cancelled, count_kept_prefix, measure, trigger::CutoverPlan};
use crate::executor::{ExecutionControl, model::ModelCaller};
use crate::{
  Error,
  protocol::{Request, model_use::context::find_replay_unit_boundaries},
  session::{CompactionReason, EntryId, GenerationId, Session, SessionError},
};

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
  let mut request = initial.clone();
  // Retain the fixed prefix verbatim. Try only structurally complete prefixes, in order.
  for remove_end in std::iter::once(fixed).chain(boundaries.into_iter().filter(|end| *end > fixed))
  {
    if remove_end == initial.conversation.len() {
      continue;
    }
    check_cancelled(control)?;
    request.conversation = initial.conversation[..fixed]
      .iter()
      .chain(&initial.conversation[remove_end..])
      .cloned()
      .collect();
    // A wire may reject a structurally closed suffix (e.g. a model-only beginning). Move to the
    // next complete boundary; never repair it by deleting a signature or inventing a tool result.
    if caller.validate_request(&request).is_err() {
      continue;
    }
    let measurement = measure(caller, control, &config, &request, calibration).await?;
    if measurement.tokens > config.target_tokens {
      continue;
    }
    let kept: Vec<EntryId> =
      entries[..fixed].iter().chain(&entries[remove_end..]).copied().collect();
    if kept == original {
      if matches!(reason, CompactionReason::ContextRejected) {
        continue;
      }
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
