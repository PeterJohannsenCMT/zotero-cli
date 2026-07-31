use std::io::IsTerminal;

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
        let year: String = item.date().chars().take(4).collect();
        let author = truncate(&item.creator_summary(), AUTHOR_WIDTH);
        let mut title_lines = wrap_text(&item.title(), title_width).into_iter();

        println!(
            "{:<tag_width$}  {:<YEAR_WIDTH$}  {:<AUTHOR_WIDTH$}  {}",
            display_key(item),
            year,
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

/// Print just the citation/item key of each item, one per line, for piping
/// into other `zotero` commands.
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
