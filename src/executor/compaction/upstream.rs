use super::{Failure, Sizing, check_cancelled, measure};
use crate::{
  Error,
  executor::{ExecutionControl, model::ModelCaller, observe::notify_observers},
  protocol::{Message, Request, upstream_compaction::UpstreamCompactionRequest},
  session::{
    CompactionReason, RunOutcome, Session, SessionEvent,
    statistics::{CallObservation, ModelCallStatus, Timestamp},
  },
};
use futures_util::{
  future::{Either, select},
  pin_mut,
};

pub(super) struct Plan<'a> {
  pub request: Request,
  pub fixed: usize,
  pub reason: CompactionReason,
  pub sizing: Sizing<'a>,
}

pub(super) async fn replace_context(
  caller: &impl ModelCaller,
  session: &mut Session,
  control: &ExecutionControl,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
  plan: Plan<'_>,
) -> Result<(), Failure> {
  let Plan { mut request, fixed, reason, sizing } = plan;
  let active = session.get_active_generation()?;
  let list = session.get_generation_entries(active.id)?;
  let entries: Vec<_> =
    list.read_page(0, list.len()? as usize)?.items.iter().map(|id| **id).collect();
  let latest_user =
    request.conversation.iter().rposition(|message| matches!(message, Message::User { .. }));
  check_cancelled(control)?;
  session.start_upstream_compaction_call(entries.len() as u64)?;
  session.record_events(vec![(
    Timestamp::now(),
    SessionEvent::UpstreamCompactionStarted { generation: active.id },
  )])?;
  notify_observers(session, cursor, observe)?;
  let started = std::time::Instant::now();
  // Send the whole active conversation, including any earlier opaque compaction body, with the
  // prompt controls the conversation calls send, so a compaction made on a model call reads their
  // prompt cache.
  let input = UpstreamCompactionRequest {
    model: request.model.clone(),
    conversation: request.conversation.clone(),
    tools: request.tools.clone(),
    tool_choice: request.tool_choice,
    reasoning: request.reasoning.clone(),
    cache: request.cache.clone(),
  };
  let result = {
    let compacting = caller.compact_upstream(&input);
    let interrupted = control.wait_for_cancellation();
    pin_mut!(compacting, interrupted);
    match select(interrupted, compacting).await {
      Either::Left(_) => Err(Failure::Outcome(RunOutcome::Interrupted)),
      Either::Right((result, _)) => result.map_err(Failure::from),
    }
  };
  let status = match &result {
    Ok(_) => ModelCallStatus::Completed,
    Err(Failure::Outcome(RunOutcome::Interrupted)) => ModelCallStatus::Interrupted,
    Err(_) => ModelCallStatus::Failed,
  };
  session.complete_compaction_call(
    CallObservation {
      finished_at: Some(Timestamp::now()),
      elapsed_ms: Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64),
      usage: result.as_ref().map(|response| response.usage).unwrap_or_default(),
      ..Default::default()
    },
    status,
  )?;
  let response = result?;
  let body: Vec<_> = response
    .conversation
    .iter()
    .filter(|message| matches!(message, Message::UpstreamCompaction { .. }))
    .cloned()
    .collect();
  session.record_events(vec![(
    Timestamp::now(),
    SessionEvent::UpstreamCompactionCompleted(Box::new(response)),
  )])?;
  notify_observers(session, cursor, observe)?;
  if body.is_empty() {
    return Err(Error::Malformed("upstream compaction returned no compaction body".into()).into());
  }
  request.conversation = request.conversation[..fixed]
    .iter()
    .chain(&body)
    .chain(latest_user.map(|index| &input.conversation[index]))
    .cloned()
    .collect();
  caller.validate_request(&request)?;
  let measurement = measure(caller, control, sizing.config, &request, sizing.calibration).await?;
  if measurement.tokens > sizing.config.target_tokens {
    return Err(Error::Build("compaction target cannot accommodate the fixed prompt, upstream compaction body and latest user message".into()).into());
  }
  check_cancelled(control)?;
  session.commit_upstream_compaction(
    active.id,
    entries[..fixed].to_vec(),
    body,
    latest_user.map(|index| entries[index]),
    measurement,
    reason,
  )?;
  Ok(())
}
