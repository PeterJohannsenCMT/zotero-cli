use anyhow::Result;

use crate::cache::Cache;
use crate::config::Config;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn run() -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::web(&config)?;
    let mut cache = Cache::load()?;
    sync::force_sync(&read_client, &write_client, &mut cache)?;
    eprintln!("Synced with Zotero.");
    Ok(())
}
