use anyhow::Result;

use crate::cache::Cache;
use crate::config::Config;
use crate::sync;
use crate::zotero_api::{ApiItem, ZoteroClient};

pub fn run(subcollection: Option<String>, tag: Option<String>) -> Result<()> {
    let config = Config::load()?;
    let client = ZoteroClient::from_config(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&client, &mut cache)?;

    let collection_key = match &subcollection {
        Some(name) => Some(cache.resolve_collection_by_name(name)?),
        None => None,
    };

    let mut items = cache.top_level_items(collection_key.as_deref(), tag.as_deref());

    if items.is_empty() {
        println!("No items found.");
        return Ok(());
    }

    items.sort_by_key(|a| display_key(a));

    let tag_width = items
        .iter()
        .map(|i| display_key(i).len())
        .max()
        .unwrap_or(4)
        .max(4);

    println!(
        "{:<width$}  {:<6}  {:<20}  TITLE",
        "TAG",
        "YEAR",
        "AUTHOR",
        width = tag_width
    );
    for item in &items {
        let year: String = item.date().chars().take(4).collect();
        println!(
            "{:<width$}  {:<6}  {:<20}  {}",
            display_key(item),
            year,
            truncate(&item.creator_summary(), 20),
            item.title(),
            width = tag_width
        );
    }

    Ok(())
}

fn display_key(item: &ApiItem) -> String {
    item.citation_key().unwrap_or_else(|| item.key.clone())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}
