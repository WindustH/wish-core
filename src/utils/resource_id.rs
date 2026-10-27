//! Stable identifiers used in task records and session resource references.
/// A shell execution id: `<pid>-<nanoseconds>-<counter>`.
pub fn is_execution_id(word: &str) -> bool {
  let mut parts = word.split('-');
  (0..3).all(|_| {
    parts.next().is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
  }) && parts.next().is_none()
}
