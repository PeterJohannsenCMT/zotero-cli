use anyhow::Result;
use serde_json::Value;

use crate::cache::Cache;
use crate::config::Config;
use crate::display;
use crate::sync;
use crate::zotero_api::{ApiItem, ZoteroClient};

/// Print the citation key (or raw item key, if it has none) of every item
/// matching `query`, one per line, so the output can be piped straight into
/// `zotero open`/`zotero edit`/etc.
pub fn run(query: &str, subcollection: Option<String>, tag: Option<String>) -> Result<()> {
    let config = Config::load()?;
    let client = ZoteroClient::from_config(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&client, &mut cache)?;

    let collection_key = match &subcollection {
        Some(name) => Some(cache.resolve_collection_by_name(name)?),
        None => None,
    };

    let terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();

    let mut items = cache.top_level_items(collection_key.as_deref(), tag.as_deref());
    items.retain(|it| {
        let hay = haystack(it);
        terms.iter().all(|t| hay.contains(t.as_str()))
    });

    if items.is_empty() {
        eprintln!("No items found for '{}'.", query);
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

/// All the free-text-searchable fields on an item, joined and lowercased.
fn haystack(item: &ApiItem) -> String {
    let mut parts = vec![
        item.title(),
        item.creator_summary(),
        item.date().to_string(),
    ];

    if let Some(creators) = item.data.get("creators").and_then(Value::as_array) {
        for creator in creators {
            for field in ["firstName", "lastName", "name"] {
                if let Some(s) = creator.get(field).and_then(Value::as_str) {
                    parts.push(s.to_string());
                }
            }
        }
    }

    parts.extend(item.tags());

    for field in ["abstractNote", "publicationTitle", "DOI", "url"] {
        if let Some(s) = item.data.get(field).and_then(Value::as_str) {
            parts.push(s.to_string());
        }
    }

    parts.join(" \u{1}").to_lowercase()
}
