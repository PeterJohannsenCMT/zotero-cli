use anyhow::{Result, anyhow};

use crate::cache::Cache;
use crate::config::Config;
use crate::sync;
use crate::zotero_api::{Collection, ZoteroClient};

/// Print every subcollection as an indented tree, mirroring their nesting in
/// Zotero.
pub fn list() -> Result<()> {
    let cache = load_synced_cache()?;

    if cache.collections.is_empty() {
        eprintln!("No subcollections found.");
        return Ok(());
    }

    print_tree(&cache, None, 0);
    Ok(())
}

/// Print the names of subcollections matching `query`, one per line (for
/// piping into `--subcollection`).
pub fn search(query: &str) -> Result<()> {
    let cache = load_synced_cache()?;
    let query_lower = query.to_lowercase();

    let mut matches: Vec<&Collection> = cache
        .collections
        .values()
        .filter(|c| c.name().to_lowercase().contains(&query_lower))
        .collect();

    if matches.is_empty() {
        eprintln!("No subcollections found for '{}'.", query);
        return Ok(());
    }

    matches.sort_by_key(|c| c.name().to_lowercase());
    for c in matches {
        println!("{}", c.name());
    }
    Ok(())
}

/// Rename the subcollection called `name` to `new_name`.
pub fn edit(name: &str, new_name: &str) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let key = cache.resolve_collection_by_name(name)?;
    // Fetch the live version rather than trusting the cache: it may be
    // stale (e.g. edited from another Zotero client since our last sync),
    // and a stale version makes the conditional update below fail with 412.
    let version = write_client.get_collection(&key)?.version;

    let patch = serde_json::json!({ "name": new_name });
    let new_version = write_client.update_collection(&key, version, patch)?;

    if let Some(collection) = cache.collections.get_mut(&key)
        && let Some(obj) = collection.data.as_object_mut()
    {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(new_name.to_string()),
        );
        collection.version = new_version;
    }
    cache.save()?;

    println!("Renamed '{}' to '{}'.", name, new_name);
    Ok(())
}

pub fn move_item(identifier: &str, collection: &str) -> Result<()> {
    set_item_collections(identifier, collection, CollectionEdit::Replace)
}

pub fn add_item(identifier: &str, collection: &str) -> Result<()> {
    set_item_collections(identifier, collection, CollectionEdit::Add)
}

pub fn remove_item(identifier: &str, collection: &str) -> Result<()> {
    set_item_collections(identifier, collection, CollectionEdit::Remove)
}

/// Set the default subcollection context used by `zotero list`, `zotero search`,
/// and filtered `zotero export`.
pub fn use_collection(collection: &str) -> Result<()> {
    let mut config = Config::load()?;
    let cache = load_synced_cache()?;
    let key = cache.resolve_collection_by_name(collection)?;
    let name = cache
        .collection_name(&key)
        .unwrap_or_else(|| collection.to_string());

    config.current_collection_key = Some(key);
    config.save()?;

    println!("Now using subcollection '{}'.", name);
    Ok(())
}

/// Clear the default subcollection context, returning to the library top level.
pub fn clear_current() -> Result<()> {
    let mut config = Config::load()?;
    config.current_collection_key = None;
    config.save()?;
    println!("Now using the library top level.");
    Ok(())
}

/// Print the current default subcollection context.
pub fn current() -> Result<()> {
    let config = Config::load()?;
    let Some(key) = config.current_collection_key.as_deref() else {
        println!("Top level");
        return Ok(());
    };

    let cache = load_synced_cache()?;
    let name = cache.collection_name(key).ok_or_else(|| {
        anyhow!(
            "current subcollection no longer exists; run `zotero collections clear` or `zotero collections use <name>`"
        )
    })?;
    println!("{}", name);
    Ok(())
}

enum CollectionEdit {
    Replace,
    Add,
    Remove,
}

fn set_item_collections(identifier: &str, collection: &str, edit: CollectionEdit) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let collection_key = cache.resolve_collection_by_name(collection)?;
    let cached_item = match cache.find_by_identifier(identifier) {
        Some(item) => item.clone(),
        None => {
            // If the user just changed an AuthorYear tag elsewhere, our fresh
            // cache window can make the new identifier temporarily invisible.
            // Force one real sync before giving up.
            sync::force_sync(&read_client, &write_client, &mut cache)?;
            cache
                .find_by_identifier(identifier)
                .ok_or_else(|| anyhow!("no item found for '{}'", identifier))?
                .clone()
        }
    };

    // Fetch the live item before writing. Tag edits made outside this CLI bump
    // Zotero's item version, and using the cached version for a collections
    // patch causes a 412 Precondition Failed. Building the patch from the live
    // item also preserves the just-edited tags in our cache.
    let mut item = write_client.get_item(&cached_item.key)?;

    let current = item
        .data
        .get("collections")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut keys: Vec<String> = current
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();

    match edit {
        CollectionEdit::Replace => keys = vec![collection_key.clone()],
        CollectionEdit::Add => {
            if !keys.iter().any(|key| key == &collection_key) {
                keys.push(collection_key.clone());
            }
        }
        CollectionEdit::Remove => keys.retain(|key| key != &collection_key),
    }

    let patch = serde_json::json!({ "collections": keys });
    let new_version = write_client.update_item(&item.key, item.version, patch.clone())?;

    if let Some(obj) = item.data.as_object_mut() {
        obj.insert("collections".to_string(), patch["collections"].clone());
    }
    item.version = new_version;
    cache.items.insert(item.key.clone(), item.clone());
    cache.save()?;

    match edit {
        CollectionEdit::Replace => println!("Moved '{}' to '{}'.", item.title(), collection),
        CollectionEdit::Add => println!("Added '{}' to '{}'.", item.title(), collection),
        CollectionEdit::Remove => println!("Removed '{}' from '{}'.", item.title(), collection),
    }
    Ok(())
}

fn load_synced_cache() -> Result<Cache> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;
    Ok(cache)
}

fn print_tree(cache: &Cache, parent: Option<&str>, depth: usize) {
    let mut children: Vec<&Collection> = cache
        .collections
        .values()
        .filter(|c| c.parent_key() == parent)
        .collect();
    children.sort_by_key(|c| c.name().to_lowercase());

    for child in children {
        println!("{}{}", "  ".repeat(depth), child.name());
        print_tree(cache, Some(child.key.as_str()), depth + 1);
    }
}
