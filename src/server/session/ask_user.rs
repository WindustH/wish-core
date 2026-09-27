//! `ask_user`: a form of questions the agent puts to the person using the session, each one a
//! choice or a blank to fill in. The tool waits like any other - until the form is answered, its
//! timeout passes or the run is interrupted - so everything else about the session works as it
//! always does. A form answered after its timeout is delivered later, as a message of its own.

use crate::executor::{
  ExecutionControl,
  tool::{ToolCall, ToolOutcome},
};
use crate::protocol::Tool;
use crate::server::error::ApiError;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;

const MAX_QUESTIONS: usize = 8;
const MAX_OPTIONS: usize = 8;
const MAX_TIMEOUT_SECONDS: u64 = 7 * 24 * 3600;

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
  Choice,
  Text,
}

#[derive(Clone, Serialize)]
struct Choice {
  label: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  description: Option<String>,
}

/// One question, with every optional field settled.
#[derive(Clone, Serialize)]
struct Question {
  #[serde(rename = "type")]
  kind: Kind,
  question: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  header: Option<String>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  options: Vec<Choice>,
  multi_select: bool,
  allow_other: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  placeholder: Option<String>,
  multiline: bool,
}

enum Reply {
  Answered(Vec<Value>),
  Skipped,
}

/// What became of an answer given through the API.
pub enum Delivery {
  /// The waiting tool call returned it.
  Now,
  /// The call had timed out; the answers go to the session as a message.
  Later { questions: Value, answers: Vec<Value> },
  /// A timed-out form was skipped; nothing is sent.
  Dropped,
}

struct Pending {
  form: Vec<Question>,
  asked_at: u64,
  timeout_seconds: Option<u64>,
  /// Present while the tool call waits; taken when an answer is handed to it.
  reply: Option<oneshot::Sender<Reply>>,
  timed_out: bool,
}

/// The open forms of one session, by tool call ID.
#[derive(Default)]
pub struct Questions {
  pending: Mutex<HashMap<String, Pending>>,
}

pub fn specification() -> Tool {
  Tool {
    name: "ask_user".into(),
    description: "Ask the user one or more questions and wait for the answers. Use it when you cannot proceed well without information only the user has, or when approaches differ in trade-offs the user should choose between. Do not ask about anything you can find out yourself by reading files or running commands, and do not ask permission for routine work. Each question is either `choice` (the user picks from `options`, and can write an answer of their own unless `allow_other` is false) or `text` (the user writes the answer). Put related questions in one call. The result's `status` is `answered`, with one entry per question in `answers` (a question the user left out has `skipped: true`); `skipped` if the user declined the whole form; or `timed_out` if you set `timeout_seconds` and no answer came in time. After a timeout, continue with your best judgement and say what you assumed: if the user answers later, the answers arrive as a separate message. Leave out `timeout_seconds` when you cannot continue without the answers.".into(),
    input_schema: json!({
      "type": "object",
      "additionalProperties": false,
      "required": ["questions"],
      "properties": {
        "questions": {
          "type": "array", "minItems": 1, "maxItems": MAX_QUESTIONS,
          "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["type", "question"],
            "properties": {
              "type": {"type": "string", "enum": ["choice", "text"], "description": "`choice` to pick from options, `text` for a written answer."},
              "question": {"type": "string", "description": "The full question. Markdown is allowed."},
              "header": {"type": "string", "description": "A very short label shown above the question, a few words at most."},
              "options": {
                "type": "array", "minItems": 2, "maxItems": MAX_OPTIONS,
                "description": "The choices of a `choice` question. Put the one you recommend first.",
                "items": {
                  "type": "object",
                  "additionalProperties": false,
                  "required": ["label"],
                  "properties": {
                    "label": {"type": "string", "description": "The choice, short."},
                    "description": {"type": "string", "description": "What choosing it means, when the label alone does not say."}
                  }
                }
              },
              "multi_select": {"type": "boolean", "description": "For `choice`: allow picking several options. Default false."},
              "allow_other": {"type": "boolean", "description": "For `choice`: allow an answer of the user's own besides the options. Default true."},
              "placeholder": {"type": "string", "description": "For `text`: a hint shown in the empty answer field."},
              "multiline": {"type": "boolean", "description": "For `text`: offer a larger field for a longer answer."}
            }
          }
        },
        "timeout_seconds": {"type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_SECONDS, "description": "How long to wait before continuing without the answers."}
      }
    }),
  }
}

impl Questions {
  /// The open forms, oldest first, as the session status reports them.
  pub fn snapshot(&self) -> Vec<Value> {
    let pending = self.pending.lock().unwrap();
    let mut items: Vec<_> = pending
      .iter()
      .map(|(call_id, form)| {
        json!({"call_id": call_id, "questions": form.form, "asked_at": form.asked_at,
          "timeout_seconds": form.timeout_seconds, "timed_out": form.timed_out})
      })
      .collect();
    items.sort_by_key(|item| item["asked_at"].as_u64());
    items
  }

  /// Puts the form to the user and waits. `changed` runs whenever the open forms change.
  pub async fn ask(
    &self,
    call: &ToolCall,
    control: &ExecutionControl,
    changed: impl Fn(),
  ) -> ToolOutcome {
    let (form, timeout) = match parse_form(&call.arguments) {
      Ok(parsed) => parsed,
      Err(message) => return ToolOutcome::Failed(message),
    };
    let (sender, receiver) = oneshot::channel();
    self.pending.lock().unwrap().insert(
      call.call_id.clone(),
      Pending {
        form,
        asked_at: crate::session::statistics::Timestamp::now().0,
        timeout_seconds: timeout,
        reply: Some(sender),
        timed_out: false,
      },
    );
    // Whatever ends the wait - even the run's future being dropped - closes the form, except a
    // timeout, which leaves it open for a later answer.
    let mut open = Opened { questions: self, call_id: &call.call_id, keep: false };
    changed();
    let expire = async {
      match timeout {
        Some(seconds) => tokio::time::sleep(Duration::from_secs(seconds)).await,
        None => std::future::pending().await,
      }
    };
    let outcome = tokio::select! {
      reply = receiver => match reply {
        Ok(Reply::Answered(answers)) => ToolOutcome::Success(json!({"status": "answered", "answers": answers})),
        Ok(Reply::Skipped) => ToolOutcome::Success(json!({"status": "skipped",
          "note": "The user chose not to answer. Continue with your best judgement."})),
        Err(_) => ToolOutcome::Cancelled,
      },
      _ = expire => {
        open.keep = true;
        if let Some(form) = self.pending.lock().unwrap().get_mut(&call.call_id) {
          form.timed_out = true;
          form.reply = None;
        }
        ToolOutcome::Success(json!({"status": "timed_out", "waited_seconds": timeout,
          "note": "No answer came in time. The questions stay open to the user; if they answer later, the answers arrive as a separate message. Continue with your best judgement and say what you assumed."}))
      }
      _ = control.wait_for_cancellation() => ToolOutcome::Cancelled,
    };
    drop(open);
    changed();
    outcome
  }

  /// Answers an open form, or with `skip` declines it.
  pub fn answer(
    &self,
    call_id: &str,
    answers: Option<&[Value]>,
    skip: bool,
  ) -> Result<Delivery, ApiError> {
    let mut pending = self.pending.lock().unwrap();
    let form = pending.get_mut(call_id).ok_or_else(ApiError::not_found)?;
    let reply = if skip {
      Reply::Skipped
    } else {
      let answers =
        answers.ok_or_else(|| ApiError::bad_request("give `answers`, or `skip` the form"))?;
      Reply::Answered(read_answers(&form.form, answers)?)
    };
    match form.reply.take() {
      Some(sender) => {
        if sender.send(reply).is_err() {
          pending.remove(call_id);
          return Err(ApiError::conflict("the question is no longer open"));
        }
        Ok(Delivery::Now)
      }
      None if form.timed_out => {
        let questions = json!(pending.remove(call_id).map(|form| form.form));
        Ok(match reply {
          Reply::Answered(answers) => Delivery::Later { questions, answers },
          Reply::Skipped => Delivery::Dropped,
        })
      }
      // An answer is already on its way to the waiting call.
      None => Err(ApiError::conflict("the question is already answered")),
    }
  }
}

/// Closes a form when the wait ends, unless it timed out and stays open for a late answer.
struct Opened<'a> {
  questions: &'a Questions,
  call_id: &'a str,
  keep: bool,
}
impl Drop for Opened<'_> {
  fn drop(&mut self) {
    if !self.keep {
      self.questions.pending.lock().unwrap().remove(self.call_id);
    }
  }
}

/// The message that carries answers given after the call had timed out.
/// The message that brings answers given after a timeout; its metadata carries the questions
/// too, so the answers can be shown without the call that asked them.
pub fn late_answer_message(
  call_id: &str,
  questions: &Value,
  answers: &[Value],
) -> crate::protocol::Message {
  crate::protocol::Message::Developer {
    metadata: json!({"source": "ask_user_answer", "call_id": call_id, "questions": questions, "answers": answers}),
    fixed: Some(false),
    content: vec![crate::protocol::ContentBlock::Text {
      text: format!(
        "The user answered the questions you asked earlier (ask_user call {call_id}), after they had timed out:\n{}",
        json!(answers)
      ),
    }],
  }
}

fn parse_form(arguments: &Value) -> Result<(Vec<Question>, Option<u64>), String> {
  let questions = arguments
    .get("questions")
    .and_then(Value::as_array)
    .filter(|items| (1..=MAX_QUESTIONS).contains(&items.len()))
    .ok_or_else(|| format!("`questions` must be an array of 1 to {MAX_QUESTIONS} questions"))?;
  let timeout = match arguments.get("timeout_seconds") {
    None | Some(Value::Null) => None,
    Some(value) => Some(
      value.as_u64().filter(|seconds| (1..=MAX_TIMEOUT_SECONDS).contains(seconds)).ok_or_else(
        || format!("`timeout_seconds` must be a whole number from 1 to {MAX_TIMEOUT_SECONDS}"),
      )?,
    ),
  };
  let form = questions
    .iter()
    .enumerate()
    .map(|(index, question)| {
      parse_question(question).map_err(|message| format!("question {}: {message}", index + 1))
    })
    .collect::<Result<Vec<_>, _>>()?;
  Ok((form, timeout))
}

fn text(value: &Value, key: &str) -> Result<Option<String>, String> {
  match value.get(key) {
    None | Some(Value::Null) => Ok(None),
    Some(Value::String(text)) => Ok(Some(text.trim().to_owned()).filter(|text| !text.is_empty())),
    Some(_) => Err(format!("`{key}` must be a string")),
  }
}

fn flag(value: &Value, key: &str) -> Result<Option<bool>, String> {
  match value.get(key) {
    None | Some(Value::Null) => Ok(None),
    Some(Value::Bool(flag)) => Ok(Some(*flag)),
    Some(_) => Err(format!("`{key}` must be true or false")),
  }
}

fn parse_question(value: &Value) -> Result<Question, String> {
  if !value.is_object() {
    return Err("must be an object".into());
  }
  let kind = match value.get("type").and_then(Value::as_str) {
    Some("choice") => Kind::Choice,
    Some("text") => Kind::Text,
    _ => return Err("`type` must be \"choice\" or \"text\"".into()),
  };
  let question = text(value, "question")?.ok_or("`question` is required")?;
  let options = match (kind, value.get("options")) {
    (Kind::Text, None | Some(Value::Null)) => Vec::new(),
    (Kind::Text, Some(_)) => {
      return Err("a text question takes no `options`; use type \"choice\"".into());
    }
    (Kind::Choice, Some(Value::Array(items))) => {
      items.iter().map(parse_choice).collect::<Result<Vec<_>, _>>()?
    }
    (Kind::Choice, _) => return Err("a choice question needs `options`".into()),
  };
  if kind == Kind::Choice {
    if !(2..=MAX_OPTIONS).contains(&options.len()) {
      return Err(format!("a choice question needs 2 to {MAX_OPTIONS} options"));
    }
    let mut labels = HashSet::new();
    for option in &options {
      if !labels.insert(option.label.as_str()) {
        return Err(format!("option `{}` appears twice", option.label));
      }
    }
  }
  let choice = kind == Kind::Choice;
  Ok(Question {
    kind,
    question,
    header: text(value, "header")?,
    options,
    multi_select: choice && flag(value, "multi_select")?.unwrap_or(false),
    allow_other: choice && flag(value, "allow_other")?.unwrap_or(true),
    placeholder: if choice { None } else { text(value, "placeholder")? },
    multiline: !choice && flag(value, "multiline")?.unwrap_or(false),
  })
}

fn parse_choice(value: &Value) -> Result<Choice, String> {
  // A bare string is taken as the label, since models sometimes write options that way.
  if let Some(label) = value.as_str().map(str::trim).filter(|label| !label.is_empty()) {
    return Ok(Choice { label: label.to_owned(), description: None });
  }
  if !value.is_object() {
    return Err("each option must be an object with a `label`".into());
  }
  Ok(Choice {
    label: text(value, "label")?.ok_or("each option needs a `label`")?,
    description: text(value, "description")?,
  })
}

/// Checks answers against the form, returning them in the shape the model reads: each with its
/// question, and either what was chosen or written, or `skipped`.
fn read_answers(form: &[Question], answers: &[Value]) -> Result<Vec<Value>, ApiError> {
  if answers.len() != form.len() {
    return Err(ApiError::bad_request(format!("give {} answers, one per question", form.len())));
  }
  let mut read = Vec::with_capacity(form.len());
  let mut given = 0;
  for (index, (question, answer)) in form.iter().zip(answers).enumerate() {
    let invalid =
      |message: String| ApiError::bad_request(format!("answer {}: {message}", index + 1));
    let mut entry = json!({"question": question.question, "type": question.kind});
    if answer.get("skipped").and_then(Value::as_bool) == Some(true) {
      entry["skipped"] = json!(true);
      read.push(entry);
      continue;
    }
    let other = text(answer, "other").map_err(invalid)?;
    let written = text(answer, "text").map_err(invalid)?;
    match question.kind {
      Kind::Choice => {
        let selected = match answer.get("selected") {
          None | Some(Value::Null) => Vec::new(),
          Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| invalid("`selected` must list option labels".into()))?,
          Some(_) => return Err(invalid("`selected` must be an array".into())),
        };
        if let Some(label) = selected
          .iter()
          .find(|label| !question.options.iter().any(|option| &option.label == *label))
        {
          return Err(invalid(format!("`{label}` is not one of the options")));
        }
        if selected.len() > 1 && !question.multi_select {
          return Err(invalid("this question takes one option".into()));
        }
        if other.is_some() && !question.allow_other {
          return Err(invalid("this question takes only its options".into()));
        }
        if selected.is_empty() && other.is_none() {
          entry["skipped"] = json!(true);
        } else {
          entry["selected"] = json!(selected);
          if let Some(other) = other {
            entry["other"] = json!(other);
          }
          given += 1;
        }
      }
      Kind::Text => match written {
        Some(written) => {
          entry["text"] = json!(written);
          given += 1;
        }
        None => entry["skipped"] = json!(true),
      },
    }
    read.push(entry);
  }
  if given == 0 {
    return Err(ApiError::bad_request("answer at least one question, or skip the form"));
  }
  Ok(read)
}
