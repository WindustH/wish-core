//! Offline reference, embedded from the same Markdown files shipped in the repository.
use std::io::{self, Write};

struct Page {
  topic: &'static str,
  path: &'static str,
  title: &'static str,
  text: &'static str,
}
macro_rules! page {
  ($topic:literal, $path:literal, $title:literal) => {
    Page {
      topic: $topic,
      path: $path,
      title: $title,
      text: include_str!(concat!("../../../docs/", $path)),
    }
  };
}
const PAGES: &[Page] = &[
  page!("overview", "README.md", "Documentation overview"),
  page!("user", "user/README.md", "User guide index"),
  page!("getting-started", "user/getting-started.md", "Installation and first session"),
  page!("configuration", "user/configuration.md", "Configuration reference"),
  page!("deployment", "user/deployment.md", "Deployment, backups and upgrades"),
  page!("api", "user/api.md", "HTTP API and SSE"),
  page!("developer", "developer/README.md", "Developer guide index"),
  page!("architecture", "developer/architecture.md", "System architecture and startup"),
  page!("client", "developer/client.md", "Model clients, transport and retries"),
  page!("protocol", "developer/protocol.md", "Model protocols and stream replay"),
  page!("session", "developer/session.md", "Session engine, queue and history"),
  page!("executor", "developer/executor.md", "Execution and context compaction"),
  page!("storage-and-migration", "developer/storage-and-migration.md", "Storage and migrations"),
  page!(
    "tools-and-extensions",
    "developer/tools-and-extensions.md",
    "Shell, tools, extensions, MCP and skills"
  ),
];
const USAGE: &str = "usage:
  wish docs                         list embedded manual topics
  wish docs list                    list topics and source paths
  wish docs <topic>                 print a chapter as Markdown
  wish docs show <topic>            same as above
  wish docs search <words...>       find lines containing all words (case-insensitive)
  wish docs --all                   print every chapter

Examples:
  wish docs getting-started
  wish docs configuration
  wish docs architecture
  wish docs user/configuration.md
  wish docs search shell proxy
  wish docs configuration | less

The manual is compiled into this binary, matching its source version. No running server,
configuration file, session environment, repository checkout or network is needed.
Output is plain UTF-8 Markdown; pipe to a pager or redirect to a file if desired.
Topic names or paths from the index (optionally prefixed with docs/) are accepted.
Exit status: 0 success, 1 no search matches, 2 invalid arguments, 3 output failure.";

pub fn run(args: Vec<String>) -> i32 {
  let stdout = io::stdout();
  match execute(&args, &mut stdout.lock()) {
    Ok(code) => code,
    // Unix pipelines such as `wish docs --all | head` may close early.
    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
    Err(error) => {
      eprintln!("wish docs: {error}");
      3
    }
  }
}

fn execute(args: &[String], out: &mut impl Write) -> io::Result<i32> {
  match args.first().map(String::as_str) {
    None | Some("list") if args.len() <= 1 => {
      writeln!(out, "Wish {} — embedded manual\n", env!("CARGO_PKG_VERSION"))?;
      for page in PAGES {
        writeln!(out, "  {:24} {}\n  {:24} docs/{}", page.topic, page.title, "", page.path)?;
      }
      writeln!(
        out,
        "\nRead: wish docs <topic>\nSearch: wish docs search <words...>\nHelp: wish docs --help"
      )?;
    }
    Some("--help" | "-h" | "help") if args.len() == 1 => writeln!(out, "{USAGE}")?,
    Some("--all") if args.len() == 1 => {
      for page in PAGES {
        writeln!(out, "<!-- docs/{} -->\n\n{}\n", page.path, page.text)?;
      }
    }
    Some("search") if args.len() > 1 => {
      let words: Vec<String> =
        args[1..].iter().flat_map(|arg| arg.split_whitespace()).map(str::to_lowercase).collect();
      if words.is_empty() {
        return invalid("search needs non-empty words");
      }
      let mut matched = false;
      for page in PAGES {
        for (index, line) in page.text.lines().enumerate() {
          let lower = line.to_lowercase();
          if words.iter().all(|word| lower.contains(word)) {
            writeln!(out, "docs/{}:{}: {}", page.path, index + 1, line)?;
            matched = true;
          }
        }
      }
      if !matched {
        writeln!(out, "No matches in the embedded manual.")?;
        return Ok(1);
      }
    }
    Some("show") if args.len() == 2 => return show(&args[1], out),
    Some("show" | "search" | "list" | "--all" | "--help" | "-h" | "help") => {
      return invalid("invalid arguments; run wish docs --help");
    }
    Some(topic) if args.len() == 1 => return show(topic, out),
    _ => return invalid("expected a topic or subcommand; run wish docs --help"),
  }
  Ok(0)
}

fn show(topic: &str, out: &mut impl Write) -> io::Result<i32> {
  let path = topic.strip_prefix("docs/").unwrap_or(topic);
  let Some(page) = PAGES.iter().find(|page| page.topic == topic || page.path == path) else {
    return invalid(&format!("unknown topic {topic:?}; run wish docs list"));
  };
  out.write_all(page.text.as_bytes())?;
  Ok(0)
}

fn invalid(message: &str) -> io::Result<i32> {
  eprintln!("wish docs: {message}");
  Ok(2)
}
