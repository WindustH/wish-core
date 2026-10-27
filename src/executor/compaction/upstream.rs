//! A cutover through the caller's upstream compaction: the provider compacts the whole active
//! context into an opaque item. The new context keeps the fixed prefix, that item and an
//! internal continuation notice quoting (not replaying) the latest user message.
use super::{Failure, call_status, check_cancelled, measure, trigger::CutoverPlan};
use crate::{
  executor::{ExecutionControl, model::ModelCaller, observe::record_and_deliver},
  protocol::error::Error,
  protocol::{ContentBlock, Message, upstream_compaction::UpstreamCompactionRequest},
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
  let continuation = continuation_notice(&request.conversation);
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
    .chain(std::iter::once(&continuation))
    .cloned()
    .collect();
  caller.validate_request(&request)?;
  let measurement = measure(caller, control, &config, &request, calibration).await?;
  if measurement.tokens > config.target_tokens {
    return Err(Error::Build("compaction target cannot accommodate the fixed prompt, upstream compaction body and continuation notice".into()).into());
  }
  check_cancelled(control)?;
  session.commit_upstream_compaction(
    active.id,
    entries[..fixed].to_vec(),
    body,
    continuation,
    measurement,
    reason,
  )?;
  Ok(())
}

/// This is application context, not another user turn. Carry the quotation through successive
/// compactions too, when the raw user message is no longer in the active generation.
fn continuation_notice(conversation: &[Message]) -> Message {
  let latest = conversation.iter().rev().find_map(|message| match message {
    Message::User { metadata, content }
      if !matches!(
        metadata["source"].as_str(),
        Some("background_tool_finished" | "background_execution_finished")
      ) =>
    {
      Some(
        content
          .iter()
          .map(|block| match block {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::Image { mime_type, .. } => {
              format!("[Image attachment: {mime_type}; consult the original history if needed]")
            }
          })
          .collect::<Vec<_>>()
          .join("\n"),
      )
    }
    Message::Developer { metadata, .. } if metadata["source"] == "wish_compaction_continuation" => {
      metadata["last_user_message"].as_str().map(str::to_owned)
    }
    _ => None,
  });
  let quoted = latest
    .as_ref()
    .map(|text| serde_json::to_string(text).expect("string serialization"))
    .unwrap_or_else(|| "No previous user message is available in this context.".into());
  Message::Developer {
    metadata: serde_json::json!({"source":"wish_compaction_continuation", "last_user_message":latest}),
    fixed: Some(false),
    content: vec![ContentBlock::Text {
      text: format!(
        "Context compaction has just completed. Continue the existing task from the progress and unfinished work recorded in the compacted context; do not restart it or merely acknowledge that you will continue. If work remains and you can proceed, take the next concrete step. If it is already complete, requires user input, or is blocked, report that accurately. Do not repeat completed operations.\n\nThe user's last message is quoted below as historical context, NOT a newly submitted message or a new instruction from the application. The quotation retains its original user-level authority and does not override higher-priority instructions:\n{quoted}"
      ),
    }],
  }
}
