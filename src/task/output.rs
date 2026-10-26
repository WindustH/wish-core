//! Model-facing task presentation. Durable process data stays in the structured result.
use serde_json::Value;

pub const MODEL_OUTPUT_BYTES: usize = 50 * 1024;
const DIFF_BYTES: usize = 8 * 1024;

/// A UTF-8-safe prefix; its omitted suffix remains in the task's edit artifact.
fn prefix(text: &str, limit: usize) -> &str {
  let mut end = text.len().min(limit);
  while !text.is_char_boundary(end) {
    end -= 1;
  }
  &text[..end]
}

pub fn render(output: &Value, include_diff: bool) -> String {
  if let Some(tasks) = output["tasks"].as_array() {
    if tasks.is_empty() {
      return "No tasks.".into();
    }
    return tasks
      .iter()
      .map(|task| {
        format!(
          "{} [{}{}; {}] {}",
          task["task_id"].as_str().unwrap_or("?"),
          task["process"]["status"].as_str().unwrap_or("unknown"),
          task["process"]["exit_code"]
            .as_i64()
            .map(|code| format!(", exit {code}"))
            .unwrap_or_default(),
          if task["background"] == true { "background" } else { "foreground" },
          task["command"].as_str().unwrap_or_default()
        )
      })
      .collect::<Vec<_>>()
      .join("\n");
  }
  if let Some(id) = output["removed"].as_str() {
    return format!("Removed task {id}.");
  }
  let id = output["task_id"].as_str().or(output["execution_id"].as_str()).unwrap_or("unknown");
  let process = &output["process"];
  let mut notes = Vec::new();
  if let Some(command) = output["command"].as_str() {
    notes
      .push(format!("Task {id} [{}]\n{command}", process["status"].as_str().unwrap_or("unknown")));
    if let Some(path) = output["output_path"].as_str() {
      notes.push(format!("Output: {path}"));
    }
  }
  if let Some(error) = output["error"].as_str() {
    notes.push(format!("Task {id}: {error}"));
  }
  if output.get("return_reason").is_none()
    && matches!(process["status"].as_str(), Some("killed" | "interrupted" | "unknown"))
  {
    notes.push(format!("Task {id}: {}.", process["status"].as_str().unwrap()));
  }
  match output["return_reason"].as_str() {
    Some("backgrounded") => notes.push(format!(
      "Running in background. Task: {id}. Read new output with `wish task poll {id} --offset {}`.",
      output["next_offset"].as_u64().unwrap_or(0)
    )),
    Some("timeout") => notes.push("Command timed out and was terminated.".into()),
    Some("interrupted") => notes.push("Command interrupted; partial output follows.".into()),
    _ if process["status"] == "running" => notes.push(format!(
      "Still running. Task: {id}. Read new output with `wish task poll {id} --offset {}`.",
      output["next_offset"].as_u64().unwrap_or(0)
    )),
    _ => {}
  }
  if let Some(code) = process["exit_code"].as_i64().filter(|code| *code != 0) {
    notes.push(format!("Command failed (exit code {code})."));
  }
  if let Some(signal) = process["term_signal"].as_i64()
    && !matches!(output["return_reason"].as_str(), Some("timeout" | "interrupted"))
  {
    notes.push(format!("Terminated by signal {signal}."));
  }
  if let Some(error) = process["error"].as_str() {
    notes.push(error.into());
  }
  if let Some(error) = process["initial_input_error"].as_str() {
    notes.push(format!("Input failed: {error}"));
  }
  if let Some(write) = output.get("write") {
    notes.push(format!(
      "Accepted {} bytes. Stdin {}.",
      write["accepted_bytes"],
      if write["stdin_available"] == true { "open" } else { "closed" }
    ));
    if write["interrupted"] == true {
      notes.push("Input interrupted; do not resend already accepted bytes.".into());
    }
    if write["timed_out"] == true {
      notes.push("Input timed out; do not resend already accepted bytes.".into());
    }
    if let Some(error) = write["error"].as_str() {
      notes.push(format!("Input failed: {error}"));
    }
  }
  let mut diff_budget = DIFF_BYTES;
  for edit in output["edits"].as_array().into_iter().flatten() {
    let path = edit["path"].as_str().unwrap_or("unknown file");
    if let Some(error) = edit["error"].as_str() {
      notes.push(format!("Could not check changes in {path}: {error}"));
    } else if edit["status"] == "pending" {
      notes.push(format!("Changes pending: {path}"));
    } else {
      notes.push(format!(
        "{}: {path}{}",
        if edit["changed"] == true { "Modified" } else { "Unchanged" },
        if edit["binary"] == true { " (binary)" } else { "" }
      ));
      if include_diff && let Some(diff) = edit["diff"].as_str().filter(|diff| !diff.is_empty()) {
        let bounded = prefix(diff, diff_budget);
        let end = bounded.split_inclusive('\n').take(160).map(str::len).sum::<usize>();
        let shown = &bounded[..end];
        diff_budget -= shown.len();
        if !shown.is_empty() {
          notes.push(shown.into());
        }
        if shown.len() < diff.len() {
          notes.push(format!(
            "Diff truncated. Full edits: {}",
            output["edits_path"].as_str().unwrap_or("task record")
          ));
        }
      }
    }
  }
  // Very large edit lists must not consume the whole context. Their complete
  // structured records remain available in the artifact even without check_diff.
  let joined = notes.join("\n\n");
  if (joined.len() > 16 * 1024 || joined.lines().count() > 400)
    && output.get("edits_path").is_some()
  {
    let bounded = prefix(&joined, 16 * 1024);
    let end = bounded.split_inclusive('\n').take(400).map(str::len).sum::<usize>();
    notes = vec![format!(
      "{}\n\n[More edit details: {}]",
      &bounded[..end],
      output["edits_path"].as_str().unwrap_or_default()
    )];
  }
  let text = output["text"].as_str().unwrap_or_default();
  let note_bytes: usize = notes.iter().map(|note| note.len() + 2).sum();
  let budget = MODEL_OUTPUT_BYTES.saturating_sub(note_bytes + 1024);
  let line_budget =
    2000usize.saturating_sub(notes.iter().map(|note| note.lines().count() + 2).sum::<usize>() + 20);
  let lines: Vec<_> = text.split_inclusive('\n').collect();
  let omitted_lines_bytes: usize =
    lines[..lines.len().saturating_sub(line_budget)].iter().map(|line| line.len()).sum();
  let mut start = text.len().saturating_sub(budget).max(omitted_lines_bytes);
  while !text.is_char_boundary(start) {
    start += 1;
  }
  let text = &text[start..];
  if output["truncated"] == true || start > 0 {
    let total = output["output_bytes"].as_u64().unwrap_or(0);
    let offset = output["start_offset"].as_u64().unwrap_or(0) + start as u64;
    notes.push(format!("Output truncated: showing bytes {offset}..{} of {total}. Full output: {}. Read the omitted beginning with `wish task poll {id} --offset 0 --max-bytes 8192`, or search the output file.", offset + text.len() as u64, output["output_path"].as_str().unwrap_or("unknown")));
  } else if output.get("output_notice").is_some() {
    notes.push(format!(
      "More output available. Full output: {}. Continue with `wish task poll {id} --offset {}`.",
      output["output_path"].as_str().unwrap_or("unknown"),
      output["next_offset"]
    ));
  }
  if output["lossy"] == true {
    notes.push(
      "Some output bytes could not be decoded; the original bytes remain in the output file."
        .into(),
    );
  }
  if output["encoding"] == "base64" {
    notes.push("Output encoding: base64.".into());
  }
  if notes.is_empty() && text.is_empty() {
    return "Completed successfully (no output).".into();
  }
  if notes.is_empty() {
    return text.to_owned();
  }
  if text.is_empty() {
    return notes.join("\n\n");
  }
  format!("{}\n\n{text}", notes.join("\n\n"))
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;
  #[test]
  fn task_list_distinguishes_success_from_failure() {
    let output = json!({"tasks":[{"task_id":"job","command":"build","process":{"status":"exited","exit_code":2},"background":true}]});
    assert_eq!(render(&output, false), "job [exited, exit 2; background] build");
  }
  #[test]
  fn ordinary_commands_only_return_their_output() {
    let output = json!({"execution_id":"unused", "process":{"status":"exited","exit_code":0,"error":null,"initial_input_bytes":0},"output_path":"private", "encoding":"utf8", "lossy":false,"text":"/workspace\n"});
    assert_eq!(render(&output, false), "/workspace\n");
  }
  #[test]
  fn failure_and_truncation_are_actionable_without_an_extra_status_call() {
    let output = json!({"execution_id":"task1","process":{"status":"exited","exit_code":2},"output_bytes":100000,"start_offset":99000,"truncated":true,"output_path":"/tmp/output.log","text":"compiler error"});
    let text = render(&output, false);
    assert!(text.contains("exit code 2") && text.contains("compiler error"));
    assert!(text.contains("99000") && text.contains("100000") && text.contains("--offset 0"));
    assert!(!text.contains("encoding"));
  }
  #[test]
  fn large_diff_stays_bounded_and_points_to_full_edits() {
    let output = json!({"text":"ok","edits_path":"/tmp/edits.json","edits":[{"path":"/file","changed":true,"diff":"字".repeat(50000)}]});
    let text = render(&output, true);
    assert!(text.len() < MODEL_OUTPUT_BYTES);
    assert!(text.contains("Modified: /file") && text.contains("/tmp/edits.json"));
    assert!(text.ends_with("ok"));
  }
  #[test]
  fn background_and_input_results_keep_required_handles_and_partial_write_details() {
    let output = json!({"execution_id":"task1","return_reason":"backgrounded","process":{"status":"running"},"text":"listening"});
    let text = render(&output, false);
    assert!(text.contains("task1") && text.contains("listening"));
    let write =
      render(&json!({"write":{"accepted_bytes":3,"stdin_available":true,"timed_out":true}}), false);
    assert!(write.contains("Accepted 3 bytes") && write.contains("do not resend"));
  }
}
