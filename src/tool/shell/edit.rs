use super::ShellError;
use imara_diff::{
  Algorithm, Diff, InternedInput, Interner, Token, UnifiedDiffConfig, UnifiedDiffPrinter,
};
use serde::Serialize;
use std::{
  fmt, io,
  path::{Path, PathBuf},
};

pub(super) struct EditCapture {
  pub path: PathBuf,
  before: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum EditResult {
  Pending { path: PathBuf },
  Complete { path: PathBuf, changed: bool, binary: bool, diff: Option<String> },
  Failed { path: PathBuf, error: String },
}

impl EditCapture {
  pub async fn capture(path: PathBuf) -> Result<Self, ShellError> {
    if !path.is_absolute() {
      return Err(ShellError::InvalidArguments(format!(
        "diff paths must be absolute: {}",
        path.display()
      )));
    }
    let before = match tokio::fs::read(&path).await {
      Ok(bytes) => bytes,
      Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
      Err(error) => {
        return Err(
          io::Error::new(
            error.kind(),
            format!("cannot read diff file {}: {error}", path.display()),
          )
          .into(),
        );
      }
    };
    Ok(Self { path, before })
  }

  /// Retain the result at completion, so later polls cannot observe unrelated subsequent edits.
  pub async fn finish(self) -> EditResult {
    let path = self.path.clone();
    match tokio::task::spawn_blocking(move || self.compare()).await {
      Ok(Ok(result)) => result,
      Ok(Err(error)) => EditResult::Failed { path, error: error.to_string() },
      Err(error) => EditResult::Failed { path, error: error.to_string() },
    }
  }

  fn compare(self) -> io::Result<EditResult> {
    let after = read_file(&self.path)?;
    let changed = self.before != after;
    let text = std::str::from_utf8(&self.before).ok().zip(std::str::from_utf8(&after).ok());
    let binary = text.is_none() || self.before.contains(&0) || after.contains(&0);
    let diff = if binary {
      None
    } else if !changed {
      Some(String::new())
    } else {
      let (before, after) = text.expect("validated text");
      let input = InternedInput::new(before, after);
      let mut diff = Diff::compute(Algorithm::Histogram, &input);
      diff.postprocess_lines(&input);
      let body = diff
        .unified_diff(&LineDiffPrinter(&input.interner), UnifiedDiffConfig::default(), &input)
        .to_string();
      // Quote paths so tabs and newlines in a filename cannot corrupt the headers.
      let label =
        serde_json::to_string(&self.path.to_string_lossy()).expect("serialize path string");
      Some(format!("--- {label}\n+++ {label}\n{body}"))
    };
    Ok(EditResult::Complete { path: self.path, changed, binary, diff })
  }
}

// The library's basic printer omits missing-newline markers and counts empty ranges from one.
// Keep Git-compatible line endings and range coordinates without reimplementing diff algorithms.
struct LineDiffPrinter<'a>(&'a Interner<&'a str>);

impl LineDiffPrinter<'_> {
  fn write_line(&self, mut output: impl fmt::Write, prefix: char, token: Token) -> fmt::Result {
    let line = self.0[token];
    write!(output, "{prefix}{line}")?;
    if !line.ends_with('\n') {
      writeln!(output, "\n\\ No newline at end of file")?;
    }
    Ok(())
  }
}

impl UnifiedDiffPrinter for LineDiffPrinter<'_> {
  fn display_header(
    &self,
    mut output: impl fmt::Write,
    start_before: u32,
    start_after: u32,
    len_before: u32,
    len_after: u32,
  ) -> fmt::Result {
    writeln!(
      output,
      "@@ -{},{} +{},{} @@",
      start_before + u32::from(len_before != 0),
      len_before,
      start_after + u32::from(len_after != 0),
      len_after
    )
  }

  fn display_context_token(&self, output: impl fmt::Write, token: Token) -> fmt::Result {
    self.write_line(output, ' ', token)
  }

  fn display_hunk(
    &self,
    mut output: impl fmt::Write,
    before: &[Token],
    after: &[Token],
  ) -> fmt::Result {
    for &token in before {
      self.write_line(&mut output, '-', token)?;
    }
    for &token in after {
      self.write_line(&mut output, '+', token)?;
    }
    Ok(())
  }
}

fn read_file(path: &Path) -> io::Result<Vec<u8>> {
  match std::fs::read(path) {
    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
    result => result,
  }
}
