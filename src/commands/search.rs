use anyhow::{Result, bail};
use serde_json::Value;

use crate::cache::Cache;
use crate::config::Config;
use crate::display;
use crate::sync;
use crate::zotero_api::{ApiItem, ZoteroClient};

/// Print the citation key (or raw item key, if it has none) of every item
/// matching the given criteria, one per line, so the output can be piped
/// straight into `zotero open`/`zotero edit`/etc.
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
    subcollection: Option<String>,
    tag: Option<String>,
) -> Result<()> {
    type FieldFn = fn(&ApiItem) -> String;

    let filters: Vec<(&str, FieldFn)> = [
        (query, haystack as FieldFn),
        (author, author_haystack as FieldFn),
        (title, title_haystack as FieldFn),
        (key, key_haystack as FieldFn),
    ]
    .into_iter()
    .filter_map(|(q, f)| q.map(|q| (q, f)))
    .collect();

    if filters.is_empty() {
        bail!("no search term given: pass a query, or use --author/--title/--key");
    }

    let config = Config::load()?;
    let client = ZoteroClient::from_config(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&client, &mut cache)?;

    let collection_key = match &subcollection {
        Some(name) => Some(cache.resolve_collection_by_name(name)?),
        None => None,
    };

    let terms: Vec<(Vec<String>, FieldFn)> = filters
        .into_iter()
        .map(|(q, f)| (terms_of(q), f))
        .collect();

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
    } else {
        display::print_keys(&items);
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

/// Just the item's citation key (falling back to its raw Zotero key),
/// lowercased.
fn key_haystack(item: &ApiItem) -> String {
    display::display_key(item).to_lowercase()
}
