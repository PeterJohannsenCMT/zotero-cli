use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::cache::Cache;
use crate::config::Config;
use crate::storage;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn run(identifier: &str) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let item = cache
        .find_by_identifier(identifier)
        .ok_or_else(|| anyhow!("no item found for '{}'", identifier))?
        .clone();

    let pdf_attachment = cache.items.values().find(|c| {
        c.parent_item() == Some(item.key.as_str())
            && c.item_type() == "attachment"
            && c.data
                .get("contentType")
                .and_then(Value::as_str)
                .map(|ct| ct == "application/pdf")
                .unwrap_or(false)
    });

    if let Some(attachment) = pdf_attachment {
        let link_mode = attachment
            .data
            .get("linkMode")
            .and_then(Value::as_str)
            .unwrap_or("");
        let filename = attachment
            .data
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("");

        if link_mode == "linked_file"
            && let Some(path) = attachment.data.get("path").and_then(Value::as_str)
        {
            let path = path.strip_prefix("attachment:").unwrap_or(path);
            open::that(path)?;
            return Ok(());
        }

        // imported_file / imported_url: file lives under storage/<key>/<filename>
        let data_dir = storage::find_data_dir(&config)?;
        let path = storage::resolve_attachment_path(&data_dir, &attachment.key, filename);
        if !path.exists() {
            let web_client = ZoteroClient::web(&config)?;
            let bytes = web_client
                .download_attachment_file(&attachment.key)
                .with_context(|| format!("downloading attachment file {}", attachment.key))?;
            if !bytes.starts_with(b"%PDF-") {
                bail!("downloaded attachment does not look like a PDF");
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        }
        open::that(&path)?;
        return Ok(());
    }

    if let Some(url) = item.url_or_doi() {
        eprintln!("warning: no PDF attachment found; opening URL/DOI instead");
        open::that(&url)?;
        return Ok(());
    }

    bail!(
        "item '{}' has no PDF attachment and no URL/DOI to open",
        identifier
    )
}
