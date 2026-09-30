use std::io::IsTerminal;
use std::sync::OnceLock;

use terminal_size::{Width, terminal_size};

use crate::zotero_api::ApiItem;

const YEAR_WIDTH: usize = 6;
const AUTHOR_WIDTH: usize = 20;
const MIN_TITLE_WIDTH: usize = 20;
const FALLBACK_TERMINAL_WIDTH: usize = 100;

/// The identifier we show/match items by: the AuthorYear citation key if the
/// item has one, otherwise the raw Zotero item key.
pub fn display_key(item: &ApiItem) -> String {
    item.citation_key().unwrap_or_else(|| item.key.clone())
}

/// Whether stdout is an interactive terminal (as opposed to piped into
/// something else), so commands can choose a human-friendly table over
/// plain, script-friendly output.
pub fn stdout_is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

/// Semantic style groups for human-facing terminal output. Keeping these as
/// named groups (rather than sprinkling ANSI escape codes through commands)
/// makes it easier to tune the CLI's palette in one place.
#[derive(Clone, Copy)]
pub enum Highlight {
    Label,
    Section,
    Title,
    Creator,
    Date,
    Venue,
    Key,
    Identifier,
    Keyword,
    Collection,
    Muted,
    Body,
}

impl Highlight {
    fn sgr(self) -> &'static str {
        match self {
            Highlight::Label => "1;38;5;244",
            Highlight::Section => "1;38;5;250",
            Highlight::Title => "1;38;5;255",
            Highlight::Creator | Highlight::Date | Highlight::Venue => "38;5;252",
            Highlight::Key => "1;38;5;39",
            Highlight::Identifier => "38;5;39",
            Highlight::Keyword | Highlight::Collection | Highlight::Muted => "38;5;244",
            Highlight::Body => "38;5;250",
        }
    }
}

/// Apply a semantic highlight group when stdout is an interactive terminal.
/// Respects the common NO_COLOR/CLICOLOR/CLICOLOR_FORCE conventions so piped
/// output stays plain by default.
pub fn highlight(text: &str, group: Highlight) -> String {
    if !styles_enabled() || text.is_empty() {
        return text.to_string();
    }
    format!("\x1b[{}m{text}\x1b[0m", group.sgr())
}

fn styles_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("CLICOLOR_FORCE").is_ok_and(|value| value != "0") {
            return true;
        }
        if std::env::var("CLICOLOR").is_ok_and(|value| value == "0") {
            return false;
        }
        stdout_is_terminal()
    })
}

/// Whether this process is one stage of a shell pipeline that also runs
/// another `zotero` command (e.g. `zotero list | zotero open`). Used to
/// limit the plain, key-only output to the case it's actually meant for,
/// rather than any time stdout happens to be piped (into `less`, a file,
/// `grep`, ...), where the normal table is still what's expected.
///
/// Detected by looking for a sibling process (same parent, i.e. the same
/// shell) whose command is `zotero`. This isn't a check of the actual pipe
/// wiring, just a good-enough heuristic for the common case.
#[cfg(unix)]
pub fn piped_into_zotero() -> bool {
    let own_pid = std::process::id();
    let parent_pid = unsafe { libc::getppid() } as u32;

    let output = match std::process::Command::new("ps")
        .args(["-eo", "pid=,ppid=,comm="])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };

    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let mut fields = line.split_whitespace();
        let pid: Option<u32> = fields.next().and_then(|s| s.parse().ok());
        let ppid: Option<u32> = fields.next().and_then(|s| s.parse().ok());
        let comm = fields.next().unwrap_or("");
        let name = comm.rsplit('/').next().unwrap_or(comm);

        ppid == Some(parent_pid) && pid != Some(own_pid) && name == "zotero"
    })
}

#[cfg(not(unix))]
pub fn piped_into_zotero() -> bool {
    false
}

/// Print a human-readable table: TAG / YEAR / AUTHOR / TITLE. The title
/// column wraps to fit the terminal width, with continuation lines padded
/// under the TITLE column so they never get mistaken for another row or
/// bleed into the other columns.
pub fn print_table(items: &[&ApiItem]) {
    let tag_width = items
        .iter()
        .map(|i| display_key(i).len())
        .max()
        .unwrap_or(4)
        .max(4);

    let terminal_width = terminal_size()
        .map(|(Width(w), _)| w as usize)
        .unwrap_or(FALLBACK_TERMINAL_WIDTH);
    let fixed_width = tag_width + 2 + YEAR_WIDTH + 2 + AUTHOR_WIDTH + 2;
    let title_width = terminal_width
        .saturating_sub(fixed_width)
        .max(MIN_TITLE_WIDTH);

    let blank = " ".repeat(tag_width);
    println!(
        "{:<tag_width$}  {:<YEAR_WIDTH$}  {:<AUTHOR_WIDTH$}  TITLE",
        "TAG", "YEAR", "AUTHOR"
    );
    for item in items {
        let author = truncate(&item.creator_summary(), AUTHOR_WIDTH);
        let mut title_lines = wrap_text(&item.title(), title_width).into_iter();

        println!(
            "{:<tag_width$}  {:<YEAR_WIDTH$}  {:<AUTHOR_WIDTH$}  {}",
            display_key(item),
            year(item),
            author,
            title_lines.next().unwrap_or_default()
        );
        for line in title_lines {
            println!(
                "{}  {:<YEAR_WIDTH$}  {:<AUTHOR_WIDTH$}  {}",
                blank, "", "", line
            );
        }
    }
}

/// Print one aligned record per item, for piping into non-`zotero` commands
/// or redirecting to a file without wrapped continuation lines.
pub fn print_records(items: &[&ApiItem]) {
    let (tag_width, year_width, author_width) = record_widths(items);
    for item in items {
        println!("{}", record_line(item, tag_width, year_width, author_width));
    }
}

fn record_widths(items: &[&ApiItem]) -> (usize, usize, usize) {
    let tag_width = items
        .iter()
        .map(|item| display_key(item).chars().count())
        .max()
        .unwrap_or(0);
    let year_width = items
        .iter()
        .map(|item| year(item).chars().count())
        .max()
        .unwrap_or(0);
    let author_width = items
        .iter()
        .map(|item| item.creator_summary().chars().count())
        .max()
        .unwrap_or(0);
    (tag_width, year_width, author_width)
}

fn record_line(item: &ApiItem, tag_width: usize, year_width: usize, author_width: usize) -> String {
    format!(
        "{:<tag_width$}  {:<year_width$}  {:<author_width$}  {}",
        display_key(item),
        year(item),
        item.creator_summary(),
        sanitize_one_line(&item.title())
    )
}

fn year(item: &ApiItem) -> String {
    item.date().chars().take(4).collect()
}

fn sanitize_one_line(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

/// Print just the citation/item key of each item, one per line, for piping
/// into another `zotero` command.
pub fn print_keys(items: &[&ApiItem]) {
    for item in items {
        println!("{}", display_key(item));
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// Greedily word-wrap `text` to lines of at most `width` characters. A single
/// word longer than `width` is kept whole on its own line rather than split.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{record_line, record_widths};
    use crate::zotero_api::ApiItem;

    fn item(key: &str, title: &str, date: &str, author: &str, tag: &str) -> ApiItem {
        ApiItem {
            key: key.to_string(),
            version: 1,
            data: json!({
                "title": title,
                "date": date,
                "creators": [{ "lastName": author }],
                "tags": [{ "tag": tag }]
            }),
        }
    }

    #[test]
    fn record_line_is_one_aligned_line() {
        let first = item(
            "ABCD1234",
            "A title\nwith whitespace",
            "2024-05-01",
            "Smith",
            "smith2024",
        );
        let second = item(
            "EFGH5678",
            "Short title",
            "1999",
            "Longlastname",
            "longlastname1999",
        );
        let items = [&first, &second];
        let (tag_width, year_width, author_width) = record_widths(&items);

        assert_eq!(
            record_line(&first, tag_width, year_width, author_width),
            "smith2024         2024  Smith         A title with whitespace"
        );
        assert_eq!(
            record_line(&second, tag_width, year_width, author_width),
            "longlastname1999  1999  Longlastname  Short title"
        );
    }
}
