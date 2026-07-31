use anyhow::Result;

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
    let client = ZoteroClient::from_config(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&client, &mut cache)?;

    let key = cache.resolve_collection_by_name(name)?;
    // Fetch the live version rather than trusting the cache: it may be
    // stale (e.g. edited from another Zotero client since our last sync),
    // and a stale version makes the conditional update below fail with 412.
    let version = client.get_collection(&key)?.version;

    let patch = serde_json::json!({ "name": new_name });
    let new_version = client.update_collection(&key, version, patch)?;

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

fn load_synced_cache() -> Result<Cache> {
    let config = Config::load()?;
    let client = ZoteroClient::from_config(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&client, &mut cache)?;
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
