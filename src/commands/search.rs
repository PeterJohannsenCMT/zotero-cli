use anyhow::{Result, bail};
use serde_json::Value;

use std::collections::HashMap;

use crate::cache::Cache;
use crate::config::Config;
use crate::display;
use crate::keywords::Keywords;
use crate::sync;
use crate::zotero_api::{ApiItem, ZoteroClient};

/// Print every item matching the given criteria as a table, or, when piped
/// straight into another `zotero` command (e.g. `zotero open`/`zotero
/// edit`/etc.), as just the citation key (or raw item key, if it has none)
/// of each item, one per line.
///
/// `query` matches against every searchable field; `author`, `title`, and
/// `key` restrict the match to just that field. Any combination of these may
/// be given together, in which case an item must satisfy all of them.
#[allow(clippy::too_many_arguments)]
pub fn run(
    query: Option<&str>,
    author: Option<&str>,
    title: Option<&str>,
    key: Option<&str>,
    tags: Option<&str>,
    keywords: Option<&str>,
    subcollection: Option<String>,
    tag: Option<String>,
) -> Result<()> {
    if [query, author, title, key, tags, keywords]
        .iter()
        .all(Option::is_none)
    {
        bail!(
            "no search term given: pass a query, or use --author/--title/--key/--tags/--keywords"
        );
    }

    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;
    let keyword_store = Keywords::load()?;

    let collection_key = cache.effective_collection_key(
        subcollection.as_deref(),
        config.current_collection_key.as_deref(),
    )?;

    type FieldFn<'a> = Box<dyn Fn(&ApiItem) -> String + 'a>;
    let keyword_map = keyword_store.all();
    let filters: Vec<(&str, FieldFn)> = [
        (query, Box::new(haystack) as FieldFn),
        (author, Box::new(author_haystack) as FieldFn),
        (title, Box::new(title_haystack) as FieldFn),
        (key, Box::new(key_haystack) as FieldFn),
        (tags, Box::new(tags_haystack) as FieldFn),
        (
            keywords,
            Box::new(move |item: &ApiItem| keywords_haystack(item, keyword_map)) as FieldFn,
        ),
    ]
    .into_iter()
    .filter_map(|(q, f)| q.map(|q| (q, f)))
    .collect();

    let terms: Vec<(Vec<String>, FieldFn)> =
        filters.into_iter().map(|(q, f)| (terms_of(q), f)).collect();

    let mut items = cache.top_level_items(collection_key.as_deref(), tag.as_deref());
    items.retain(|it| {
        terms.iter().all(|(terms, field)| {
            let hay = field(it);
            terms.iter().all(|t| hay.contains(t.as_str()))
        })
    });

    if items.is_empty() {
        eprintln!("No items found.");
        return Ok(());
    }

    items.sort_by_key(|item| display::display_key(item));

    if display::stdout_is_terminal() {
        display::print_table(&items);
    } else if display::piped_into_zotero() {
        display::print_keys(&items);
    } else {
        display::print_records(&items);
    }

    Ok(())
}

fn terms_of(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// All the free-text-searchable fields on an item, joined and lowercased.
fn haystack(item: &ApiItem) -> String {
    let mut parts = vec![
        item.title(),
        item.creator_summary(),
        item.date().to_string(),
    ];

    parts.push(author_haystack(item));
    parts.extend(item.tags());

    for field in ["abstractNote", "publicationTitle", "DOI", "url"] {
        if let Some(s) = item.data.get(field).and_then(Value::as_str) {
            parts.push(s.to_string());
        }
    }

    parts.join(" \u{1}").to_lowercase()
}

/// Just the creator/author fields, lowercased.
fn author_haystack(item: &ApiItem) -> String {
    let mut parts = vec![item.creator_summary()];

    if let Some(creators) = item.data.get("creators").and_then(Value::as_array) {
        for creator in creators {
            for field in ["firstName", "lastName", "name"] {
                if let Some(s) = creator.get(field).and_then(Value::as_str) {
                    parts.push(s.to_string());
                }
            }
        }
    }

    parts.join(" \u{1}").to_lowercase()
}

/// Just the title, lowercased.
fn title_haystack(item: &ApiItem) -> String {
    item.title().to_lowercase()
}

/// Just the Zotero tags, lowercased.
fn tags_haystack(item: &ApiItem) -> String {
    item.tags().join(" \u{1}").to_lowercase()
}

/// Just the existing local keywords, lowercased.
fn keywords_haystack(item: &ApiItem, keywords: &HashMap<String, Vec<String>>) -> String {
    keywords
        .get(&item.key)
        .map(|kw| kw.join(" \u{1}"))
        .unwrap_or_default()
        .to_lowercase()
}

/// Just the item's citation key (falling back to its raw Zotero key),
/// lowercased.
fn key_haystack(item: &ApiItem) -> String {
    display::display_key(item).to_lowercase()
}
