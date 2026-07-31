use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;

use crate::cache::Cache;
use crate::tagging;
use crate::zotero_api::{ApiItem, Collection, ZoteroClient};

/// Bring the local cache up to date with the server (an incremental fetch
/// after the first run), then backfill an AuthorYear tag onto any item that
/// doesn't have one yet, writing it back to Zotero so it's a real, persistent,
/// user-editable tag rather than something recomputed every time.
pub fn sync_library(client: &ZoteroClient, cache: &mut Cache) -> Result<()> {
    pull_changes(client, cache)?;
    dedupe_citation_keys(client, cache)?;
    normalize_citation_keys(client, cache)?;
    backfill_citation_keys(client, cache)?;
    cache.save()?;
    Ok(())
}

/// Collapse an item down to a single AuthorYear-shaped tag if it somehow
/// ended up with more than one (this happened during development when a
/// buggy version failed to recognize its own previously-written tag and kept
/// appending new collision suffixes on every run).
fn dedupe_citation_keys(client: &ZoteroClient, cache: &mut Cache) -> Result<()> {
    let keys: Vec<String> = cache.items.keys().cloned().collect();
    for key in keys {
        let item = cache.items[&key].clone();
        let tags = item.tags();
        let candidates = tagging::author_year_candidates(&tags);
        if candidates.len() <= 1 {
            continue;
        }
        let Some(keep) = tagging::find_author_year_tag(&tags) else {
            continue;
        };
        let Some(tags_arr) = item.data.get("tags").and_then(Value::as_array) else {
            continue;
        };
        let filtered: Vec<Value> = tags_arr
            .iter()
            .filter(|t| {
                let tag_str = t.get("tag").and_then(Value::as_str).unwrap_or("");
                tag_str == keep || !candidates.iter().any(|c| c == tag_str)
            })
            .cloned()
            .collect();
        let patch = serde_json::json!({ "tags": filtered.clone() });

        if let Ok(new_version) = client.update_item(&item.key, item.version, patch)
            && let Some(cached) = cache.items.get_mut(&key)
            && let Some(obj) = cached.data.as_object_mut()
        {
            obj.insert("tags".to_string(), Value::Array(filtered));
            cached.version = new_version;
        }
    }
    Ok(())
}

/// Rename any citation key that isn't plain-ASCII/lowercase yet (accented
/// characters from the author's name, or a capitalized surname) to the
/// canonical form. Runs once per item; already-clean keys are left alone.
fn normalize_citation_keys(client: &ZoteroClient, cache: &mut Cache) -> Result<()> {
    let mut existing_lower: HashSet<String> = HashSet::new();
    for item in cache.items.values() {
        for tag in item.tags() {
            existing_lower.insert(tag.to_lowercase());
        }
    }

    let keys: Vec<String> = cache.items.keys().cloned().collect();
    for key in keys {
        let item = cache.items[&key].clone();
        let Some(current) = tagging::find_author_year_tag(&item.tags()) else {
            continue;
        };
        if tagging::is_clean_citation_key(&current) {
            continue;
        }
        let Some(base) = tagging::derive_base_tag_from_item(&item) else {
            continue;
        };

        existing_lower.remove(&current.to_lowercase());
        let Some(new_tag) = tagging::unique_tag_local(&base, &existing_lower) else {
            existing_lower.insert(current.to_lowercase());
            continue;
        };

        let Some(tags_arr) = item.data.get("tags").and_then(Value::as_array) else {
            existing_lower.insert(current.to_lowercase());
            continue;
        };
        let renamed: Vec<Value> = tags_arr
            .iter()
            .map(|t| {
                if t.get("tag").and_then(Value::as_str) == Some(current.as_str()) {
                    serde_json::json!({ "tag": new_tag })
                } else {
                    t.clone()
                }
            })
            .collect();
        let patch = serde_json::json!({ "tags": renamed.clone() });

        if let Ok(new_version) = client.update_item(&item.key, item.version, patch) {
            existing_lower.insert(new_tag.to_lowercase());
            if let Some(cached) = cache.items.get_mut(&key)
                && let Some(obj) = cached.data.as_object_mut()
            {
                obj.insert("tags".to_string(), Value::Array(renamed));
                cached.version = new_version;
            }
        } else {
            existing_lower.insert(current.to_lowercase());
        }
    }
    Ok(())
}

fn pull_changes(client: &ZoteroClient, cache: &mut Cache) -> Result<()> {
    let since = cache.library_version;

    if since > 0 {
        let deleted = client.deleted_since(since)?;
        for key in deleted
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            cache.items.remove(key);
        }
        for key in deleted
            .get("collections")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            cache.collections.remove(key);
        }
    }

    for value in client.collections_since(since)? {
        let collection: Collection = serde_json::from_value(value)?;
        cache.collections.insert(collection.key.clone(), collection);
    }

    let (item_values, new_version) = client.items_since(since)?;
    for value in item_values {
        let item: ApiItem = serde_json::from_value(value)?;
        cache.items.insert(item.key.clone(), item);
    }

    if new_version > cache.library_version {
        cache.library_version = new_version;
    }
    Ok(())
}

fn backfill_citation_keys(client: &ZoteroClient, cache: &mut Cache) -> Result<()> {
    let mut existing_lower: HashSet<String> = HashSet::new();
    for item in cache.items.values() {
        for tag in item.tags() {
            existing_lower.insert(tag.to_lowercase());
        }
    }

    let candidate_keys: Vec<String> = cache
        .items
        .values()
        .filter(|it| it.item_type() != "attachment" && it.item_type() != "note")
        .filter(|it| it.citation_key().is_none())
        .map(|it| it.key.clone())
        .collect();

    for key in candidate_keys {
        let item = cache.items[&key].clone();
        let Some(base) = tagging::derive_base_tag_from_item(&item) else {
            continue;
        };
        let Some(tag) = tagging::unique_tag_local(&base, &existing_lower) else {
            continue;
        };

        let mut tags_value = item
            .data
            .get("tags")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        if let Some(arr) = tags_value.as_array_mut() {
            arr.push(serde_json::json!({ "tag": tag }));
        }
        let patch = serde_json::json!({ "tags": tags_value.clone() });

        if let Ok(new_version) = client.update_item(&item.key, item.version, patch) {
            existing_lower.insert(tag.to_lowercase());
            if let Some(cached) = cache.items.get_mut(&key)
                && let Some(obj) = cached.data.as_object_mut()
            {
                obj.insert("tags".to_string(), tags_value);
                cached.version = new_version;
            }
        }
    }
    Ok(())
}
