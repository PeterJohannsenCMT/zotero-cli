use anyhow::Result;

use crate::cache::Cache;
use crate::config::Config;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn run() -> Result<()> {
    let config = Config::load()?;
    let client = ZoteroClient::from_config(&config)?;
    let mut cache = Cache::load()?;
    sync::force_sync(&client, &mut cache)?;
    eprintln!("Synced with Zotero.");
    Ok(())
}
