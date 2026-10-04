//! Skills: instructions, and the scripts or reference files beside them, for particular kinds of
//! tasks. A skill is a directory holding a `SKILL.md` whose YAML front matter names it and says what
//! it is for, the format other agents read too. The model is told of none: it finds them with
//! `wish skill` in its shell (`cli`), through the bridge, so adding or removing one never changes
//! what the model is sent.
//!
//! Skills are found, in order, in the session's `.agents/skills`, in Wish's own directory and in the
//! configured ones, each searched a few levels deep so they can sit in folders by kind. A name is
//! the first directory's that has it; when that directory has it twice, in different folders,
//! neither is used, since which one was meant can't be told. One switched off is left out wherever
//! it is.
pub mod cli;

use crate::server::config::SkillsConfig;
use serde::Serialize;
use std::{
  collections::{HashMap, HashSet},
  path::{Path, PathBuf},
};

const SKILL_FILE: &str = "SKILL.md";
/// How many folders under a root a skill may sit.
const DEPTH: usize = 4;
/// The most of a skill's other files `show` names.
const FILE_LIMIT: usize = 60;
/// How much of a `SKILL.md` a search reads.
const SEARCH_BYTES: usize = 64 * 1024;

/// What the model is told about skills: how to find them, and none of them.
pub const INSTRUCTIONS: &str = "# Skills\n\
   Skills are instructions, sometimes with scripts or reference files, for particular kinds of \
   tasks: a file format, a tool or service, a procedure or house style. None are listed here; the \
   `wish skill` command in the shell finds them.\n\
   - `wish skill find <words>` shows the skills that fit a task, best first; `wish skill list` \
   shows them all.\n\
   - `wish skill show <name>` prints a skill's instructions and the files beside them.\n\
   Before starting a task that may have one, look, and follow the skill you load.";

/// A place skills are found: where it comes from, as settings show it, and the directory.
#[derive(Clone, Debug)]
pub struct Root {
  /// `project`, `wish`, or a configured directory as written.
  pub source: String,
  pub dir: PathBuf,
}

/// The places to look, first first: the project's, Wish's own, then the configured ones.
pub fn roots(project: Option<&Path>, own: &Path, config: &SkillsConfig) -> Vec<Root> {
  let project =
    project.map(|cwd| Root { source: "project".into(), dir: cwd.join(".agents/skills") });
  let own = Root { source: "wish".into(), dir: own.to_owned() };
  let configured = config
    .dirs
    .iter()
    .filter(|dir| !dir.trim().is_empty())
    .map(|dir| Root { source: dir.clone(), dir: expand_home(dir.trim()) });
  project.into_iter().chain([own]).chain(configured).collect()
}

/// A path with a leading `~` read as the home directory.
fn expand_home(path: &str) -> PathBuf {
  let home =
    || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
  match path.strip_prefix('~') {
    Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => match home() {
      Some(home) => home.join(rest.trim_start_matches(['/', '\\'])),
      None => PathBuf::from(path),
    },
    _ => PathBuf::from(path),
  }
}

#[derive(Clone, Debug, Serialize)]
pub struct Skill {
  pub name: String,
  pub description: String,
  /// The folders it sits in under its root, if any: `configuration` for `configuration/nvim`.
  pub category: Option<String>,
  /// Its directory under its root: `configuration/nvim`.
  pub path: String,
  /// The directory holding its `SKILL.md`.
  pub dir: PathBuf,
  pub source: String,
}

/// What a look through the roots found: every skill, first first, and what could not be read.
#[derive(Default)]
pub struct Found {
  pub skills: Vec<Skill>,
  pub problems: Vec<(PathBuf, String)>,
}
impl Found {
  /// How each skill stands among those of its name, in the order of `skills`.
  pub fn standings(&self) -> Vec<Standing> {
    // The directory that first has a name, and how many skills of the name it has.
    let mut owners: HashMap<&str, (&str, usize)> = HashMap::new();
    for skill in &self.skills {
      let owner = owners.entry(&skill.name).or_insert((&skill.source, 0));
      if owner.0 == skill.source {
        owner.1 += 1;
      }
    }
    self
      .skills
      .iter()
      .map(|skill| match owners[skill.name.as_str()] {
        (source, _) if source != skill.source => Standing::Shadowed,
        (_, 1) => Standing::Used,
        _ => Standing::Conflict,
      })
      .collect()
  }

  /// The skills a session sees: each name's, without those switched off.
  pub fn usable(&self, config: &SkillsConfig) -> Vec<&Skill> {
    self
      .skills
      .iter()
      .zip(self.standings())
      .filter(|(skill, standing)| {
        *standing == Standing::Used && !config.disabled.contains(&skill.name)
      })
      .map(|(skill, _)| skill)
      .collect()
  }
}

/// How a skill stands among those of its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
  /// Sessions see it by its name.
  Used,
  /// An earlier directory has a skill of its name.
  Shadowed,
  /// Its directory has another skill of its name, so neither is used.
  Conflict,
}

/// Every skill under the roots, in their order.
pub fn discover(roots: &[Root]) -> Found {
  let mut found = Found::default();
  let mut seen = HashSet::new();
  for root in roots {
    walk(root, &root.dir, 0, &mut seen, &mut found);
  }
  found
}

fn walk(root: &Root, dir: &Path, depth: usize, seen: &mut HashSet<PathBuf>, found: &mut Found) {
  // Links are followed, so a folder reached twice is read once.
  let Ok(canonical) = dir.canonicalize() else { return };
  if !seen.insert(canonical) {
    return;
  }
  let file = dir.join(SKILL_FILE);
  if file.is_file() {
    match std::fs::read_to_string(&file) {
      Ok(text) => found.skills.push(read_skill(root, dir, &text)),
      Err(error) => found.problems.push((file, error.to_string())),
    }
    return;
  }
  if depth == DEPTH {
    return;
  }
  let Ok(entries) = std::fs::read_dir(dir) else { return };
  let mut children: Vec<PathBuf> = entries
    .flatten()
    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
    .map(|entry| entry.path())
    .filter(|path| path.is_dir())
    .collect();
  children.sort();
  for child in children {
    walk(root, &child, depth + 1, seen, found);
  }
}

fn read_skill(root: &Root, dir: &Path, text: &str) -> Skill {
  let (fields, body) = front_matter(text);
  let field = |key: &str| {
    fields
      .iter()
      .find(|(name, _)| name == key)
      .map(|(_, value)| value.trim().to_owned())
      .filter(|value| !value.is_empty())
  };
  let folder =
    || dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
  // A skill without a description is known by the first line of what it says.
  let description = field("description").unwrap_or_else(|| {
    body
      .lines()
      .map(|line| line.trim().trim_start_matches('#').trim())
      .find(|line| !line.is_empty())
      .unwrap_or_default()
      .to_owned()
  });
  let relative = |path: &Path| {
    path.strip_prefix(&root.dir).ok().map(|path| path.to_string_lossy().replace('\\', "/"))
  };
  let category = dir.parent().and_then(relative).filter(|path| !path.is_empty());
  Skill {
    name: field("name").unwrap_or_else(folder),
    description,
    category,
    path: relative(dir).unwrap_or_default(),
    dir: dir.to_owned(),
    source: root.source.clone(),
  }
}

/// The top-level fields of a `SKILL.md`'s YAML front matter, and what follows it. Only what skills
/// use is read: `key: value` lines, quoted or not, continued on indented lines, and `|` or `>`
/// blocks. A file without front matter is all body.
fn front_matter(text: &str) -> (Vec<(String, String)>, &str) {
  let text = text.strip_prefix('\u{feff}').unwrap_or(text);
  let Some(rest) = text.strip_prefix("---").filter(|rest| rest.starts_with(['\n', '\r'])) else {
    return (Vec::new(), text);
  };
  let rest = rest.trim_start_matches(['\r', '\n']);
  let (header, body) = match rest.find("\n---") {
    Some(end) => {
      let after = &rest[end + 4..];
      (&rest[..end], after.split_once('\n').map_or("", |(_, body)| body))
    }
    None => return (Vec::new(), text),
  };
  let mut fields: Vec<(String, String)> = Vec::new();
  // How the value being read goes on: folded into one line, kept as lines, or a plain scalar.
  let mut block: Option<char> = None;
  for line in header.lines() {
    let indented = line.starts_with([' ', '\t']);
    if !indented && let Some((key, value)) = line.split_once(':') {
      if key.is_empty() || key.contains(char::is_whitespace) || key.starts_with('#') {
        continue;
      }
      let value = value.trim();
      block = match value.chars().next() {
        Some(mark @ ('|' | '>')) => Some(mark),
        _ => None,
      };
      let value = if block.is_some() { String::new() } else { unquote(value) };
      fields.push((key.trim().to_owned(), value));
      continue;
    }
    let Some((_, value)) = fields.last_mut() else { continue };
    let line = line.trim();
    if line.is_empty() {
      if block == Some('|') {
        value.push('\n');
      }
      continue;
    }
    if !value.is_empty() {
      value.push(if block == Some('|') { '\n' } else { ' ' });
    }
    value.push_str(line);
  }
  (fields, body)
}

fn unquote(value: &str) -> String {
  for quote in ['"', '\''] {
    if let Some(inner) = value.strip_prefix(quote).and_then(|value| value.strip_suffix(quote)) {
      return if quote == '"' { inner.replace("\\\"", "\"") } else { inner.replace("''", "'") };
    }
  }
  value.to_owned()
}

/// The skills that fit a query, best first: its words weighed where they are found - the name
/// most, then the folder and description, then the instructions. Words of Chinese, Japanese or
/// Korean, written without spaces, are matched two characters at a time.
pub fn find<'a>(skills: &[&'a Skill], query: &str) -> Vec<&'a Skill> {
  let words: Vec<String> = query
    .to_lowercase()
    .split(|c: char| c.is_whitespace() || ",;:/|".contains(c))
    .filter(|word| !word.is_empty())
    .map(str::to_owned)
    .collect();
  let mut scored: Vec<(u32, &Skill)> = skills
    .iter()
    .map(|skill| {
      let name = skill.name.to_lowercase();
      let fields = [
        (name.clone(), 6),
        (skill.category.as_deref().unwrap_or_default().to_lowercase(), 3),
        (skill.description.to_lowercase(), 3),
        (read_prefix(&skill.dir.join(SKILL_FILE)).to_lowercase(), 1),
      ];
      let mut score = 0;
      for word in &words {
        if *word == name {
          score += 10;
        }
        for (text, weight) in &fields {
          score += if text.contains(word.as_str()) {
            weight * 2
          } else {
            pieces(word).filter(|piece| text.contains(piece.as_str())).count() as u32 * weight / 2
          };
        }
      }
      (score, *skill)
    })
    .filter(|(score, _)| *score > 0)
    .collect();
  scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
  scored.into_iter().map(|(_, skill)| skill).collect()
}

/// A word of CJK script in two-character pieces; none for other words.
fn pieces(word: &str) -> impl Iterator<Item = String> + '_ {
  let chars: Vec<char> = word.chars().collect();
  let cjk = chars.iter().any(
    |c| matches!(*c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF),
  );
  let count = if cjk && chars.len() > 2 { chars.len() - 1 } else { 0 };
  (0..count).map(move |start| chars[start..start + 2].iter().collect())
}

fn read_prefix(path: &Path) -> String {
  use std::io::Read;
  let mut bytes = Vec::new();
  let _ = std::fs::File::open(path)
    .and_then(|file| file.take(SEARCH_BYTES as u64).read_to_end(&mut bytes));
  String::from_utf8_lossy(&bytes).into_owned()
}

/// A skill as `show` gives it: its instructions, and the files beside them.
#[derive(Serialize)]
pub struct Shown {
  pub name: String,
  pub description: String,
  pub dir: PathBuf,
  pub body: String,
  /// Its other files, relative to its directory, the first [`FILE_LIMIT`] of them by path.
  pub files: Vec<String>,
  pub more_files: usize,
}

pub fn show(skill: &Skill) -> std::io::Result<Shown> {
  let text = std::fs::read_to_string(skill.dir.join(SKILL_FILE))?;
  let body = front_matter(&text).1.trim().to_owned();
  let mut files = Vec::new();
  list_files(&skill.dir, &skill.dir, 0, &mut files);
  files.sort();
  let more_files = files.len().saturating_sub(FILE_LIMIT);
  files.truncate(FILE_LIMIT);
  Ok(Shown {
    name: skill.name.clone(),
    description: skill.description.clone(),
    dir: skill.dir.clone(),
    body,
    files,
    more_files,
  })
}

fn list_files(root: &Path, dir: &Path, depth: usize, files: &mut Vec<String>) {
  let Ok(entries) = std::fs::read_dir(dir) else { return };
  for entry in entries.flatten() {
    let name = entry.file_name().to_string_lossy().into_owned();
    let path = entry.path();
    if name.starts_with('.') || (depth == 0 && name == SKILL_FILE) {
      continue;
    }
    if path.is_dir() {
      if depth < DEPTH {
        list_files(root, &path, depth + 1, files);
      }
    } else if let Ok(relative) = path.strip_prefix(root) {
      files.push(relative.to_string_lossy().replace('\\', "/"));
    }
  }
}
