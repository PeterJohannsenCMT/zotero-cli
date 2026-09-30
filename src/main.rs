mod cache;
mod commands;
mod config;
mod crossref;
mod display;
mod editable_item;
mod http;
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
        /// Only list items in this subcollection (overrides `collections use`)
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
        /// Only search items in this subcollection (overrides `collections use`)
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
    /// Add an item to your library from a DOI, or enter one by hand
    Add {
        /// The DOI to look up (e.g. 10.1000/xyz123). Use manual or --manual
        /// for items CrossRef doesn't have (books, webpages, datasets, ...).
        doi: Option<String>,
        /// Skip the CrossRef lookup and enter the item's metadata by hand in
        /// $EDITOR
        #[arg(long)]
        manual: bool,
        /// Zotero item type to create in manual mode, e.g. book,
        /// bookSection, webpage, report, thesis, dataset, conferencePaper
        #[arg(long, default_value = "journalArticle")]
        r#type: String,
        /// Do not try to download and attach a PDF after DOI lookup
        #[arg(long)]
        nopdf: bool,
    },
    /// Open an item's PDF (or its URL/DOI if no PDF is attached)
    Open {
        /// AuthorYear tag or raw Zotero item key. If omitted, identifiers are
        /// read one per line from stdin (e.g. piped from `zotero search`).
        identifier: Option<String>,
    },
    /// Show an item's details
    Show {
        /// AuthorYear tag or raw Zotero item key
        key: String,
    },
    /// Edit an item's metadata in your $EDITOR
    Edit {
        /// AuthorYear tag or raw Zotero item key. If omitted, identifiers are
        /// read one per line from stdin (e.g. piped from `zotero search`).
        identifier: Option<String>,
    },
    /// Remove an item from your Zotero library
    Remove {
        /// AuthorYear tag or raw Zotero item key
        #[arg(value_name = "REF_ID")]
        identifier: String,
    },
    /// Attach a PDF file to an item
    AttachPdf {
        /// AuthorYear tag or raw Zotero item key
        identifier: String,
        /// Path to the PDF file
        pdf: std::path::PathBuf,
    },
    /// Look up and attach a PDF for an existing item
    FetchPdf {
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
        /// Export as LaTeX \bibitem entries instead of BibTeX
        #[arg(long)]
        bibitem: bool,
        /// Do not rewrite recognized journal names to REVTeX/apsrev macros/abbreviations
        #[arg(long)]
        no_revtex_journals: bool,
        /// Use REVTeX/apsrev journal macros in BibTeX output (now the default)
        #[arg(long, hide = true)]
        revtex_journals: bool,
        /// Export items in this subcollection (overrides `collections use`)
        #[arg(long)]
        subcollection: Option<String>,
        /// Export items with this tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// Force a sync with the Zotero API, ignoring the freshness window
    Sync,
    /// Check for gaps in citation-key suffixes
    CheckCitationKeys,
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
    /// Set the default subcollection for list/search/export
    Use {
        /// Subcollection name to use by default
        collection: String,
    },
    /// Show the current default subcollection
    Current,
    /// Return to the library top level by default
    Clear,
    /// Rename a subcollection
    Edit {
        /// The subcollection's current name
        name: String,
        /// The subcollection's new name
        new_name: String,
    },
    /// Replace an item's collection membership with one subcollection
    MoveItem {
        /// AuthorYear tag or raw Zotero item key
        identifier: String,
        /// Destination subcollection name
        collection: String,
    },
    /// Add an item to a subcollection, keeping existing memberships
    AddItem {
        /// AuthorYear tag or raw Zotero item key
        identifier: String,
        /// Subcollection name
        collection: String,
    },
    /// Remove an item from a subcollection
    RemoveItem {
        /// AuthorYear tag or raw Zotero item key
        identifier: String,
        /// Subcollection name
        collection: String,
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
            CollectionsCommand::Use { collection } => {
                commands::collections::use_collection(&collection)
            }
            CollectionsCommand::Current => commands::collections::current(),
            CollectionsCommand::Clear => commands::collections::clear_current(),
            CollectionsCommand::Edit { name, new_name } => {
                commands::collections::edit(&name, &new_name)
            }
            CollectionsCommand::MoveItem {
                identifier,
                collection,
            } => commands::collections::move_item(&identifier, &collection),
            CollectionsCommand::AddItem {
                identifier,
                collection,
            } => commands::collections::add_item(&identifier, &collection),
            CollectionsCommand::RemoveItem {
                identifier,
                collection,
            } => commands::collections::remove_item(&identifier, &collection),
        },
        Commands::Add {
            doi,
            manual,
            r#type,
            nopdf,
        } => {
            if manual || doi.as_deref() == Some("manual") {
                commands::add::run_manual(&r#type)
            } else {
                match doi {
                    Some(doi) => commands::add::run(&doi, !nopdf),
                    None => Err(anyhow::anyhow!(
                        "either pass a DOI to look up, or `manual` to enter an item by hand"
                    )),
                }
            }
        }
        Commands::Open { identifier } => run_for_each(identifier, commands::open::run),
        Commands::Show { key } => commands::show::run(&key),
        Commands::Edit { identifier } => run_for_each(identifier, commands::edit::run),
        Commands::Remove { identifier } => commands::remove::run(&identifier),
        Commands::AttachPdf { identifier, pdf } => commands::attach::pdf(&identifier, &pdf),
        Commands::FetchPdf { identifier } => run_for_each(identifier, commands::attach::find_pdf),
        Commands::Export {
            identifiers,
            bibitem,
            no_revtex_journals,
            revtex_journals: _,
            subcollection,
            tag,
        } => commands::export::run(
            identifiers,
            subcollection,
            tag,
            bibitem,
            !no_revtex_journals,
        ),
        Commands::Sync => commands::sync::run(),
        Commands::CheckCitationKeys => commands::check_citation_keys::run(),
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
