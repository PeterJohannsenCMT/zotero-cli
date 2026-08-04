use anyhow::Result;

use crate::cache::Cache;
use crate::config::Config;
use crate::display;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn run(subcollection: Option<String>, tag: Option<String>) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let collection_key = cache.effective_collection_key(
        subcollection.as_deref(),
        config.current_collection_key.as_deref(),
    )?;

    let mut items = cache.top_level_items(collection_key.as_deref(), tag.as_deref());

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
