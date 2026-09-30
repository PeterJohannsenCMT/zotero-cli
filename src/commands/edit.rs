use anyhow::{Context, Result, anyhow, bail};
use std::process::Command;

use crate::cache::Cache;
use crate::config::Config;
use crate::editable_item::EditableItem;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn run(identifier: &str) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let cached_item = match cache.find_by_identifier(identifier) {
        Some(item) => item.clone(),
        None => {
            // A recent external tag change can make an identifier absent from
            // the cached view during the freshness window. Sync once before
            // reporting that it doesn't exist.
            sync::force_sync(&read_client, &write_client, &mut cache)?;
            cache
                .find_by_identifier(identifier)
                .ok_or_else(|| anyhow!("no item found for '{}'", identifier))?
                .clone()
        }
    };
    // Edit the live item so external tag changes don't leave us patching with
    // a stale Zotero version (412 Precondition Failed) or displaying old data.
    let item = write_client.get_item(&cached_item.key)?;
    let editable = EditableItem::from_data(&item.data);
    let toml_text = editable
        .to_editor_toml()
        .context("serializing item for editing")?;

    let tmp_path = std::env::temp_dir().join(format!("zotero-cli-edit-{}.toml", item.key));
    std::fs::write(&tmp_path, &toml_text)
        .with_context(|| format!("writing temp file {}", tmp_path.display()))?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .with_context(|| format!("launching editor '{}'", editor))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp_path);
        bail!("editor exited with a non-zero status; aborting edit");
    }

    let edited_text = std::fs::read_to_string(&tmp_path)
        .with_context(|| format!("reading back {}", tmp_path.display()))?;
    let _ = std::fs::remove_file(&tmp_path);

    let edited: EditableItem = toml::from_str(&edited_text).context("parsing edited TOML")?;
    let patch = edited.patch_for(&item.data)?;
    if patch.as_object().is_some_and(|fields| fields.is_empty()) {
        return Ok(());
    }
    let new_version = write_client.update_item(&item.key, item.version, patch.clone())?;

    let mut updated_item = item;
    if let Some(obj) = updated_item.data.as_object_mut()
        && let Some(patch_obj) = patch.as_object()
    {
        for (k, v) in patch_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    updated_item.version = new_version;
    cache.items.insert(updated_item.key.clone(), updated_item);
    cache.save()?;

    Ok(())
}
