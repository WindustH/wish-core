//! A cutover through the caller's upstream compaction: the provider compacts the whole active
//! context into an opaque item, and the new context is the fixed prefix, that item and the latest
//! user message, all three indivisible.
use super::{Failure, call_status, check_cancelled, measure, trigger::CutoverPlan};
use crate::{
  Error,
  executor::{ExecutionControl, model::ModelCaller, observe::record_and_deliver},
  protocol::{Message, upstream_compaction::UpstreamCompactionRequest},
  session::{
    RunOutcome, Session, SessionEvent,
    statistics::{CallObservation, ModelCallPurpose},
  },
};

pub(super) async fn replace_with_upstream(
  caller: &impl ModelCaller,
  session: &mut Session,
  control: &ExecutionControl,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
  plan: CutoverPlan,
) -> Result<(), Failure> {
  let CutoverPlan { reason, mut request, config, calibration } = plan;
  let fixed =
    request.conversation.iter().take_while(|message| message.is_fixed_instruction()).count();
  let active = session.get_active_generation()?;
  let entries = session.reader().read_generation_entry_ids(active.id)?;
  let latest_user =
    request.conversation.iter().rposition(|message| matches!(message, Message::User { .. }));
  check_cancelled(control)?;
  session.start_model_call(ModelCallPurpose::UpstreamCompaction, entries.len() as u64)?;
  record_and_deliver(
    session,
    delivered,
    observe,
    SessionEvent::UpstreamCompactionStarted { generation: active.id },
  )?;
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
  let result = match control.run_until_cancelled(caller.compact_upstream(&input)).await {
    Some(result) => result.map_err(RunOutcome::Failed),
    None => Err(RunOutcome::Interrupted),
  };
  let mut observation = CallObservation {
    usage: result.as_ref().map(|response| response.usage).unwrap_or_default(),
    ..Default::default()
  };
  observation.finish(started);
  session.complete_compaction_call(observation, call_status(&result))?;
  let response = result.map_err(Failure::Outcome)?;
  let body: Vec<_> = response
    .conversation
    .iter()
    .filter(|message| matches!(message, Message::UpstreamCompaction { .. }))
    .cloned()
    .collect();
  record_and_deliver(
    session,
    delivered,
    observe,
    SessionEvent::UpstreamCompactionCompleted(Box::new(response)),
  )?;
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
  let measurement = measure(caller, control, &config, &request, calibration).await?;
  if measurement.tokens > config.target_tokens {
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
