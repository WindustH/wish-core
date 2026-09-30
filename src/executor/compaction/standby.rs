//! Standby summaries: closed spans of the active context summarized ahead of a cutover, into the
//! standby generation the cutover then builds on. A run keeps at most one in flight beside its work
//! ([`StandbySummarizer`]); a cutover summarizes every remaining span at once ([`catch_up`]).
use super::{
  Failure, check_cancelled, collect_assistant_content, count_kept_prefix, measure,
  trigger::{UsageReading, read_usage},
};
use crate::executor::{
  ExecutionControl,
  model::{ModelCaller, ModelResult, execute_model},
  observe::{deliver_new_events, record_and_deliver},
};
use crate::{
  protocol::{
    ContentBlock, Message, Request, Response,
    model_use::{context::find_replay_unit_boundaries, stream::IncompleteReason},
  },
  session::{
    GenerationId, RunOutcome, Session, SessionError, SessionEvent, SessionState, TokenMeasurement,
    statistics::CallObservation,
  },
  utils::time::Timestamp,
};
use futures_util::{
  future::{BoxFuture, Either, join_all, select},
  pin_mut,
};
use std::future::Future;

/// A span to summarize, and the call that summarizes it.
struct StandbySummaryPlan {
  summary_request: Request,
  /// Active entries the summary request repeats before its instruction.
  input_entry_count: u64,
  generation: GenerationId,
  start: u64,
  end: u64,
  measurement: TokenMeasurement,
}
impl StandbySummaryPlan {
  /// The event that announces this summary.
  fn started_event(&self) -> SessionEvent {
    SessionEvent::CompactionSummaryStarted {
      source_start: self.start,
      source_end: self.end,
      measurement: self.measurement,
    }
  }
}

/// A summary written, not yet committed.
struct StandbySummaryResult {
  input_entry_count: u64,
  generation: GenerationId,
  start: u64,
  end: u64,
  summary: Message,
  response: Response,
  observation: CallObservation,
}

/// How a summary call ended.
type SummaryOutcome = Result<StandbySummaryResult, Failure>;

async fn plan_standby_summary(
  caller: &impl ModelCaller,
  session: &Session,
  control: &ExecutionControl,
) -> Result<Option<StandbySummaryPlan>, Failure> {
  Ok(plan_standby_summaries(caller, session, control, 1).await?.pop())
}

/// Up to `limit` consecutive spans, the first starting at the first unsummarized entry and each
/// later one where the previous ends.
async fn plan_standby_summaries(
  caller: &impl ModelCaller,
  session: &Session,
  control: &ExecutionControl,
  limit: usize,
) -> Result<Vec<StandbySummaryPlan>, Failure> {
  let Some(config) = session.get_config().compaction.clone() else {
    return Ok(Vec::new());
  };
  if caller.supports_upstream_compaction() || control.is_cancelled() {
    return Ok(Vec::new());
  }
  let UsageReading { active, request, last_call, calibration } = read_usage(session)?;
  let fixed = count_kept_prefix(&request.conversation);
  let standby = session.get_standby_generation()?;
  let processed = standby
    .summarized_end(&active)
    .map(|end| end as usize)
    .unwrap_or((active.compaction_cursor as usize).max(fixed));
  let eligible = last_call.as_ref().map(|call| call.input_entry_count as usize).unwrap_or(0);
  let mut plans = Vec::new();
  if eligible <= processed || processed >= request.conversation.len() {
    return Ok(plans);
  }
  let boundaries = find_replay_unit_boundaries(&request.conversation)?;
  let mut start = processed;
  for end in boundaries.into_iter().filter(|end| *end > processed && *end <= eligible) {
    if request.conversation[start..end]
      .iter()
      .any(|message| matches!(message, Message::UpstreamCompaction { .. }))
    {
      break;
    }
    let span_request = build_span_request(&request, &request.conversation[start..end]);
    caller.validate_request(&span_request)?;
    let measurement = measure(caller, control, &config, &span_request, calibration).await?;
    if measurement.tokens < config.segment_tokens {
      continue;
    }
    let summary_request = build_summary_request(&request, eligible, start, end);
    caller.validate_request(&summary_request)?;
    plans.push(StandbySummaryPlan {
      summary_request,
      input_entry_count: eligible as u64,
      generation: active.id,
      start: start as u64,
      end: end as u64,
      measurement,
    });
    if plans.len() == limit {
      break;
    }
    start = end;
  }
  Ok(plans)
}

/// Makes the summary call of `plan`. A summary that reaches its output limit continues like a
/// conversation call does, so the output cap it inherits from the session bounds one segment
/// rather than the whole summary.
async fn execute_standby_summary(
  caller: &impl ModelCaller,
  control: &ExecutionControl,
  plan: StandbySummaryPlan,
) -> SummaryOutcome {
  let (result, observation) =
    execute_model(caller, &plan.summary_request, control, None, &mut |_| {}).await;
  let response = match result {
    ModelResult::Complete(response) => response,
    ModelResult::Interrupted(_) => return Err(Failure::Outcome(RunOutcome::Interrupted)),
    // A summary keeps nothing of a stream that failed: the error that ended it is the failure.
    ModelResult::Failed(RunOutcome::StreamFailed(partial)) => match partial.reason {
      IncompleteReason::Failed(error) => return Err(error.into()),
      reason => unreachable!("a failed stream ends for its error, not {reason:?}"),
    },
    ModelResult::Failed(outcome) => return Err(Failure::Outcome(outcome)),
  };
  let content = collect_assistant_content(&response, "summary").map_err(Failure::Outcome)?;
  check_cancelled(control)?;
  Ok(StandbySummaryResult {
    input_entry_count: plan.input_entry_count,
    generation: plan.generation,
    start: plan.start,
    end: plan.end,
    summary: Message::User { metadata: Default::default(), content },
    response,
    observation,
  })
}

/// Appends a written summary to the standby, unless the active generation changed meanwhile.
fn commit_standby_summary(
  session: &mut Session,
  result: StandbySummaryResult,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<(), SessionError> {
  let active = session.get_active_generation()?;
  if active.id != result.generation {
    return Ok(());
  }
  let call =
    session.record_completed_compaction_call(result.observation, result.input_entry_count)?;
  session.append_standby_summary(
    result.generation,
    result.start,
    result.end,
    result.summary,
    result.response,
    call,
  )?;
  deliver_new_events(session, delivered, observe)
}

/// Commits a summary that finished beside the run's work, or records why it failed. A standby
/// summary is speculative maintenance: its failure stays visible in history and live state, but
/// neither fails nor suspends the conversation it was preparing for.
fn settle_standby_summary(
  summary: SummaryOutcome,
  session: &mut Session,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<(), SessionError> {
  match summary {
    Ok(result) => commit_standby_summary(session, result, delivered, observe),
    Err(Failure::Outcome(outcome)) => record_and_deliver(
      session,
      delivered,
      observe,
      SessionEvent::CompactionSummaryFailed { outcome },
    ),
    Err(Failure::Session(error)) => Err(error),
  }
}

/// Summarizes every remaining span at once, for a cutover. Each request repeats the same prefix
/// and names its own span, so none waits for another. They commit in order, since each continues
/// the standby where the one before it ended; a failure keeps the spans committed before it.
pub(super) async fn catch_up(
  caller: &impl ModelCaller,
  session: &mut Session,
  control: &ExecutionControl,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<(), Failure> {
  let plans = plan_standby_summaries(caller, session, control, usize::MAX).await?;
  let started_at = Timestamp::now();
  session.record_events(plans.iter().map(|plan| (started_at, plan.started_event())).collect())?;
  deliver_new_events(session, delivered, observe)?;
  let results =
    join_all(plans.into_iter().map(|plan| execute_standby_summary(caller, control, plan))).await;
  for result in results {
    commit_standby_summary(session, result?, delivered, observe)?;
  }
  Ok(())
}

const SUMMARY_EXCERPT_CHARS: usize = 240;

/// The summary call: the conversation the last completed conversation call sent, unchanged, so it
/// reads that call's prompt cache, then one instruction naming the span to summarize by its first
/// and last entries. The model keeps the session's tools, reasoning and cache settings for the same
/// reason; the instruction, not a stripped request, keeps it from continuing the task.
fn build_summary_request(original: &Request, prefix: usize, start: usize, end: usize) -> Request {
  let conversation = &original.conversation;
  let describe = |position: usize| {
    let (kind, excerpt) = describe_entry(&conversation[position])?;
    let same = |message: &Message| {
      describe_entry(message).is_some_and(|(other, text)| other == kind && text == excerpt)
    };
    let occurrence = conversation[..=position].iter().filter(|message| same(message)).count();
    let total = conversation[..prefix].iter().filter(|message| same(message)).count();
    let excerpt = serde_json::to_string(&excerpt).expect("a string serializes");
    Some(if total > 1 {
      format!(
        "the {kind} entry whose visible excerpt is {excerpt} (occurrence {occurrence} of {total} with that excerpt)"
      )
    } else {
      format!("the {kind} entry whose visible excerpt is {excerpt}")
    })
  };
  let first =
    (start..end).find_map(describe).unwrap_or_else(|| "the first unsummarized entry".into());
  let last = (start..end).rev().find_map(describe).unwrap_or_else(|| first.clone());
  let instruction = format!(
    "Produce compact replacement context for one span of the conversation above.\n\nStart at {first}. Stop after {last}.\n\nPreserve goals, constraints, decisions, useful facts, tool outcomes and unfinished work that later development needs. Do not summarize or restate entries before the start boundary or after the end boundary. Do not call tools or continue the task. Return only the replacement summary."
  );
  let mut messages = conversation[..prefix].to_vec();
  messages.push(Message::User {
    metadata: Default::default(),
    content: vec![ContentBlock::Text { text: instruction }],
  });
  Request {
    // A summary can run long. Stream it so the first-byte deadline covers only upstream admission,
    // not the complete summary generation.
    stream: true,
    model: original.model.clone(),
    conversation: messages,
    tools: original.tools.clone(),
    tool_choice: original.tool_choice,
    max_output_tokens: original.max_output_tokens,
    reasoning: original.reasoning.clone(),
    cache: original.cache.clone(),
  }
}

/// The kind and whitespace-normalized opening of an entry the model can see.
fn describe_entry(message: &Message) -> Option<(&'static str, String)> {
  let text = |content: &[ContentBlock]| {
    content
      .iter()
      .map(|block| match block {
        ContentBlock::Text { text } => text.as_str(),
        ContentBlock::Image { .. } => "[image]",
      })
      .collect::<Vec<_>>()
      .join(" ")
  };
  let (kind, raw) = match message {
    Message::User { content, .. } => ("user", text(content)),
    Message::Assistant { content, .. } => ("assistant", text(content)),
    Message::System { content, .. } | Message::Developer { content, .. } => {
      ("instruction", text(content))
    }
    Message::ToolUse { name, arguments, .. } => ("tool call", format!("{name} {arguments}")),
    Message::ToolResult { name, content, .. } => (
      "tool result",
      format!(
        "{name} {}",
        content.as_str().map(str::to_owned).unwrap_or_else(|| content.to_string())
      ),
    ),
    Message::Reasoning { .. } | Message::UpstreamCompaction { .. } => return None,
  };
  let excerpt: String = raw
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
    .chars()
    .take(SUMMARY_EXCERPT_CHARS)
    .collect();
  (!excerpt.is_empty()).then_some((kind, excerpt))
}

/// A span flattened into one message, only to measure it against `segment_tokens`.
fn build_span_request(original: &Request, messages: &[Message]) -> Request {
  let mut blocks = Vec::new();
  for message in messages {
    let (role, content) = match message {
      Message::User { content, .. } => ("user", content.clone()),
      Message::Assistant { content, .. } => ("assistant", content.clone()),
      Message::System { content, .. } | Message::Developer { content, .. } => {
        ("instruction", content.clone())
      }
      Message::Reasoning { plaintext, .. } => {
        ("reasoning", vec![ContentBlock::Text { text: plaintext.clone() }])
      }
      Message::ToolUse { name, arguments, .. } => {
        ("tool call", vec![ContentBlock::Text { text: format!("{name}: {arguments}") }])
      }
      Message::ToolResult { name, content, .. } => {
        ("tool result", vec![ContentBlock::Text { text: format!("{name}: {content}") }])
      }
      Message::UpstreamCompaction { .. } => {
        ("opaque context", vec![ContentBlock::Text { text: "[opaque upstream context]".into() }])
      }
    };
    blocks.push(ContentBlock::Text { text: format!("[{role}]\n") });
    blocks.extend(content);
  }
  Request {
    stream: true,
    model: original.model.clone(),
    conversation: vec![Message::User { metadata: Default::default(), content: blocks }],
    tools: Vec::new(),
    tool_choice: None,
    max_output_tokens: original.max_output_tokens,
    reasoning: original.reasoning.clone(),
    cache: None,
  }
}

/// What a wait for the summary in flight ended with.
pub(in crate::executor) enum StandbyWait {
  /// The summary, if there was one, is settled.
  Settled,
  /// The run was cancelled first; the summary is dropped.
  Cancelled,
  /// Input was queued; the summary is still in flight.
  Input,
}

/// The standby summary a run prepares beside its work: at most one in flight, polled in the run's
/// own task beside the next model call or tool batch, and settled before the run returns.
pub(in crate::executor) struct StandbySummarizer<'a, C> {
  caller: &'a C,
  control: &'a ExecutionControl,
  in_flight: Option<BoxFuture<'a, SummaryOutcome>>,
}
impl<'a, C: ModelCaller> StandbySummarizer<'a, C> {
  pub fn new(caller: &'a C, control: &'a ExecutionControl) -> Self {
    Self { caller, control, in_flight: None }
  }

  /// Drops the summary in flight, prepared for settings the session no longer has.
  pub fn discard(&mut self) {
    self.in_flight = None;
  }

  /// Plans the next span and starts its summary, when none is in flight. Nothing starts on a
  /// caller that compacts upstream or in a cancelled run, and a planning failure is skipped: the
  /// summary is speculative.
  pub async fn start_next(
    &mut self,
    session: &mut Session,
    delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<(), SessionError> {
    if self.in_flight.is_some()
      || session.get_config().compaction.is_none()
      || self.caller.supports_upstream_compaction()
      || self.control.is_cancelled()
    {
      return Ok(());
    }
    let Ok(Some(plan)) = plan_standby_summary(self.caller, session, self.control).await else {
      return Ok(());
    };
    record_and_deliver(session, delivered, observe, plan.started_event())?;
    self.in_flight = Some(Box::pin(execute_standby_summary(self.caller, self.control, plan)));
    Ok(())
  }

  /// Drives `work` beside the summary in flight, both polled in this task: the work's output, and
  /// the summary if it finished first. A summary still in flight when the work ends stays so.
  pub async fn beside<T>(&mut self, work: impl Future<Output = T>) -> (T, Option<FinishedSummary>) {
    let Some(mut task) = self.in_flight.take() else {
      return (work.await, None);
    };
    pin_mut!(work);
    match select(task.as_mut(), work.as_mut()).await {
      Either::Left((summary, _)) => (work.await, Some(FinishedSummary(summary))),
      Either::Right((output, _)) => {
        self.in_flight = Some(task);
        (output, None)
      }
    }
  }

  /// Awaits the summary in flight, if any, and settles it.
  pub async fn settle(
    &mut self,
    session: &mut Session,
    delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<StandbyWait, SessionError> {
    let Some(task) = self.in_flight.take() else {
      return Ok(StandbyWait::Settled);
    };
    match self.control.run_until_cancelled(task).await {
      Some(summary) => {
        settle_standby_summary(summary, session, delivered, observe)?;
        Ok(StandbyWait::Settled)
      }
      None => Ok(StandbyWait::Cancelled),
    }
  }

  /// Waits out the summary in flight once the conversation has finished. While the session is
  /// idle, input queued meanwhile ends the wait at once, the summary still in flight, so the run
  /// collects the input and goes on beside its next action.
  pub async fn wait_after_finish(
    &mut self,
    session: &mut Session,
    delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<StandbyWait, SessionError> {
    let control = self.control;
    if let (Some(task), SessionState::Idle) = (self.in_flight.as_mut(), session.get_state()) {
      // Watch before looking at the queue, so input queued in between is not missed.
      let mut arrivals = session.watch_input_arrivals();
      if session.has_queued_input()? {
        return Ok(StandbyWait::Input);
      }
      let cancelled = control.wait_for_cancellation();
      let arrived = arrivals.changed();
      pin_mut!(cancelled, arrived);
      let summary = match select(task.as_mut(), select(cancelled, arrived)).await {
        Either::Left((summary, _)) => Some(summary),
        Either::Right((Either::Right(_), _)) => return Ok(StandbyWait::Input),
        // Cancellation is settled below, as for a summary awaited without input in view.
        Either::Right((Either::Left(_), _)) => None,
      };
      if let Some(summary) = summary {
        self.in_flight = None;
        settle_standby_summary(summary, session, delivered, observe)?;
        return Ok(StandbyWait::Settled);
      }
    }
    self.settle(session, delivered, observe).await
  }
}

/// A summary that finished beside the run's work, to settle once the work no longer holds the
/// session.
pub(in crate::executor) struct FinishedSummary(SummaryOutcome);
impl FinishedSummary {
  pub fn settle(
    self,
    session: &mut Session,
    delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<(), SessionError> {
    settle_standby_summary(self.0, session, delivered, observe)
  }
}
