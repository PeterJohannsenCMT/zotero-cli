mod cache;
mod commands;
mod config;
mod crossref;
mod display;
mod keywords;
mod storage;
mod sync;
mod tagging;
mod zotero_api;

use clap::{Parser, Subcommand};

use commands::completions::Shell;

#[derive(Parser)]
#[command(name = "zotero", about = "A CLI for your Zotero library", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List items in your library
    List {
        /// Only list items in this subcollection
        #[arg(long)]
        subcollection: Option<String>,
        /// Only list items with this tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// Search your library, printing matching keys one per line (for piping
    /// into `zotero open`/`zotero edit`/etc.)
    Search {
        /// Free-text query, matched against title, authors, tags, abstract, etc.
        query: Option<String>,
        /// Only match against the author/creator names
        #[arg(long)]
        author: Option<String>,
        /// Only match against the title
        #[arg(long)]
        title: Option<String>,
        /// Only match against the citation key (or raw item key)
        #[arg(long)]
        key: Option<String>,
        /// Only match against Zotero tags
        #[arg(long)]
        tags: Option<String>,
        /// Only match against local keywords (set via `zotero edit`)
        #[arg(long)]
        keywords: Option<String>,
        /// Only search items in this subcollection
        #[arg(long)]
        subcollection: Option<String>,
        /// Only search items with this tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// List or search your subcollections
    Collections {
        #[command(subcommand)]
        command: CollectionsCommand,
    },
    /// Add an item to your library from a DOI
    Add {
        /// The DOI to look up (e.g. 10.1000/xyz123)
        doi: String,
    },
    /// Open an item's PDF (or its URL/DOI if no PDF is attached)
    Open {
        /// AuthorYear tag or raw Zotero item key. If omitted, identifiers are
        /// read one per line from stdin (e.g. piped from `zotero search`).
        identifier: Option<String>,
    },
    /// Edit an item's metadata in your $EDITOR
    Edit {
        /// AuthorYear tag or raw Zotero item key. If omitted, identifiers are
        /// read one per line from stdin (e.g. piped from `zotero search`).
        identifier: Option<String>,
    },
    /// Export items as BibTeX, with citekeys matching their AuthorYear tag
    Export {
        /// AuthorYear tags or raw Zotero item keys to export. If omitted,
        /// identifiers are read one per line from stdin (e.g. piped from
        /// `zotero search`), or use --subcollection/--tag to select a set.
        identifiers: Vec<String>,
        /// Export items in this subcollection
        #[arg(long)]
        subcollection: Option<String>,
        /// Export items with this tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// Configure your Zotero API credentials and data directory
    Config,
    /// Print a shell completion script (with dynamic paper-key suggestions
    /// for `open`/`edit`)
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum CollectionsCommand {
    /// List all subcollections as an indented tree
    List,
    /// Search subcollections by name, printing matching names one per line
    Search {
        /// Free-text query, matched against subcollection names
        query: String,
    },
    /// Rename a subcollection
    Edit {
        /// The subcollection's current name
        name: String,
        /// The subcollection's new name
        new_name: String,
    },
}

fn main() {
    // Rust ignores SIGPIPE by default and turns a write into a closed pipe
    // into a panic; restore the standard Unix behavior (just exit) so piping
    // `zotero list`/`zotero search` into something that closes early (like a
    // failed `zotero open` with no argument, or `head`) doesn't panic.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::List { subcollection, tag } => commands::list::run(subcollection, tag),
        Commands::Search {
            query,
            author,
            title,
            key,
            tags,
            keywords,
            subcollection,
            tag,
        } => commands::search::run(
            query.as_deref(),
            author.as_deref(),
            title.as_deref(),
            key.as_deref(),
            tags.as_deref(),
            keywords.as_deref(),
            subcollection,
            tag,
        ),
        Commands::Collections { command } => match command {
            CollectionsCommand::List => commands::collections::list(),
            CollectionsCommand::Search { query } => commands::collections::search(&query),
            CollectionsCommand::Edit { name, new_name } => {
                commands::collections::edit(&name, &new_name)
            }
        },
        Commands::Add { doi } => commands::add::run(&doi),
        Commands::Open { identifier } => run_for_each(identifier, commands::open::run),
        Commands::Edit { identifier } => run_for_each(identifier, commands::edit::run),
        Commands::Export {
            identifiers,
            subcollection,
            tag,
        } => commands::export::run(identifiers, subcollection, tag),
        Commands::Config => commands::config_cmd::run(),
        Commands::Completions { shell } => commands::completions::run(shell),
    };

    if let Err(err) = result {
        eprintln!("error: {:#}", err);
        std::process::exit(1);
    }
}

/// Run `f` once for an identifier given on the command line, or once per
/// non-empty line read from stdin if no identifier was given.
fn run_for_each(
    identifier: Option<String>,
    f: impl Fn(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let ids = match identifier {
        Some(id) => vec![id],
        None => read_identifiers_from_stdin()?,
    };

    let mut any_failed = false;
    for id in &ids {
        if let Err(err) = f(id) {
            eprintln!("error: {:#} (for '{}')", err, id);
            any_failed = true;
        }
    }
    if any_failed {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn read_identifiers_from_stdin() -> anyhow::Result<Vec<String>> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let ids: Vec<String> = buf
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        anyhow::bail!(
            "no identifier given: pass one as an argument, or pipe identifiers in on stdin (e.g. from `zotero search`)"
        );
    }
    Ok(ids)
}
