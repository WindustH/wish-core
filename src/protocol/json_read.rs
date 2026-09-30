//! Reading what a service wrote, the ways every reader of a body shares.
//!
//! One rule runs through all of them: what the service said is what the caller sees. A number is
//! kept in the digits it arrived in, because a balance re-encoded through a float can lose the
//! cent that matters; an empty string is no value at all, because a zero or a blank that was
//! invented is worse than a missing one.

use serde_json::Value;

/// A value exactly as the service wrote it, whether it wrote a string or a number.
pub(crate) fn read_scalar_text(value: &Value) -> Option<String> {
  match value {
    Value::String(text) => Some(text.clone()),
    Value::Number(number) => Some(number.to_string()),
    _ => None,
  }
}

/// A string that says something: trimmed, and absent when nothing is left.
pub(crate) fn read_trimmed_text(value: &Value) -> Option<String> {
  value.as_str().map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// The text of one string member of an object, when it is there, is a string and is not empty.
pub(crate) fn read_string_member(object: Option<&Value>, key: &str) -> Option<String> {
  let text = object?.get(key)?.as_str()?;
  (!text.is_empty()).then(|| text.to_owned())
}

/// One member read as text when it carries something: a non-empty string, or a number.
///
/// A code arrives as a string from most services and as a number from several others - the wire
/// envelope decides that, not us - and either spelling is the code.
pub(crate) fn read_member_text(object: Option<&Value>, key: &str) -> Option<String> {
  match object?.get(key)? {
    Value::String(text) => (!text.is_empty()).then(|| text.to_owned()),
    Value::Number(number) => Some(number.to_string()),
    _ => None,
  }
}

/// A share a service reported as a ratio (`0.22`), spelled as the percentage other services report
/// (`22`).
///
/// The point moves rather than the number being multiplied, so no digit is invented and none is
/// lost to a float: `0.125` reads `12.5`, `1` reads `100`, `0.0` reads `0`. A value that is not a
/// decimal number reads as nothing at all.
pub(crate) fn convert_ratio_to_percent(ratio: &str) -> Option<String> {
  let (sign, digits) = match ratio.strip_prefix('-') {
    Some(magnitude) => ("-", magnitude),
    None => ("", ratio),
  };
  let (whole, fraction) = match digits.split_once('.') {
    Some((whole, fraction)) => (whole, fraction),
    None => (digits, ""),
  };
  if whole.is_empty() && fraction.is_empty() {
    return None;
  }
  if !whole.chars().chain(fraction.chars()).all(|digit| digit.is_ascii_digit()) {
    return None;
  }
  let mut fraction = fraction.to_owned();
  while fraction.len() < 2 {
    fraction.push('0');
  }
  let (moved, rest) = fraction.split_at(2);
  let integer = match format!("{whole}{moved}").trim_start_matches('0') {
    "" => "0".to_owned(),
    trimmed => trimmed.to_owned(),
  };
  let mut percent = String::from(sign);
  percent.push_str(&integer);
  let rest = rest.trim_end_matches('0');
  if !rest.is_empty() {
    percent.push('.');
    percent.push_str(rest);
  }
  Some(percent)
}
