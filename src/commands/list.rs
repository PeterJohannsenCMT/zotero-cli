use anyhow::Result;

use crate::cache::Cache;
use crate::config::Config;
use crate::display;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn run(subcollection: Option<String>, tag: Option<String>) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::web(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let collection_key = match &subcollection {
        Some(name) => Some(cache.resolve_collection_by_name(name)?),
        None => None,
    };

    let mut items = cache.top_level_items(collection_key.as_deref(), tag.as_deref());

    if items.is_empty() {
        eprintln!("No items found.");
        return Ok(());
    }

    items.sort_by_key(|item| display::display_key(item));

    if !display::stdout_is_terminal() && display::piped_into_zotero() {
        display::print_keys(&items);
    } else {
        display::print_table(&items);
    }

    Ok(())
}
