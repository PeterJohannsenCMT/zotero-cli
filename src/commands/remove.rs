use anyhow::{Result, anyhow};

use crate::cache::Cache;
use crate::config::Config;
use crate::keywords::Keywords;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn run(identifier: &str) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::web(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let cached_item = match cache.find_by_identifier(identifier) {
        Some(item) => item.clone(),
        None => {
            sync::force_sync(&read_client, &write_client, &mut cache)?;
            cache
                .find_by_identifier(identifier)
                .ok_or_else(|| anyhow!("no item found for '{}'", identifier))?
                .clone()
        }
    };

    // Delete the live item so we have a current Zotero version for the
    // conditional request instead of relying on the cache's possibly stale one.
    let item = write_client.get_item(&cached_item.key)?;
    let title = item.title();
    let removed_key = item.key.clone();
    let new_library_version = write_client.delete_item(&item.key, item.version)?;

    // If a top-level item was removed, child attachments/notes disappear with it;
    // drop them from the local cache and local keyword store immediately.
    let removed_keys: Vec<String> = cache
        .items
        .values()
        .filter(|it| it.key == removed_key || it.parent_item() == Some(removed_key.as_str()))
        .map(|it| it.key.clone())
        .collect();
    for key in &removed_keys {
        cache.items.remove(key);
    }
    if new_library_version > cache.library_version {
        cache.library_version = new_library_version;
    }
    cache.save()?;

    let mut keywords = Keywords::load()?;
    for key in &removed_keys {
        keywords.set(key, Vec::new());
    }
    keywords.save()?;

    println!("Removed '{}' ({}).", title, removed_key);
    Ok(())
}
