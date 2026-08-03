use anyhow::{Context, Result, anyhow, bail};
use std::path::Path;

use crate::cache::Cache;
use crate::config::Config;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn pdf(identifier: &str, pdf_path: &Path) -> Result<()> {
    if !pdf_path.exists() {
        bail!("PDF file does not exist: {}", pdf_path.display());
    }
    if !pdf_path.is_file() {
        bail!("PDF path is not a file: {}", pdf_path.display());
    }
    if pdf_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| !ext.eq_ignore_ascii_case("pdf"))
    {
        bail!("attachment must be a PDF file: {}", pdf_path.display());
    }

    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::web(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let item = cache
        .find_by_identifier(identifier)
        .ok_or_else(|| anyhow!("no item found for '{}'", identifier))?
        .clone();
    if item.item_type() == "attachment"
        || item.item_type() == "note"
        || item.parent_item().is_some()
    {
        bail!("'{}' is not a top-level reference item", identifier);
    }

    let filename = pdf_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow!(
                "PDF path has no valid UTF-8 filename: {}",
                pdf_path.display()
            )
        })?;
    let title = pdf_path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(filename);

    let template = read_client
        .attachment_template("imported_file")
        .context("fetching PDF attachment template")?;
    let created = write_client
        .create_pdf_attachment(&item.key, template, title, filename)
        .context("creating Zotero attachment item")?;

    let reported_version =
        write_client.upload_new_attachment_file(&created.key, pdf_path, filename)?;
    let mut fresh = write_client.get_item(&created.key)?;
    if let Some(new_version) = reported_version {
        fresh.version = new_version;
    }
    cache.items.insert(fresh.key.clone(), fresh);
    cache.save()?;

    println!(
        "Attached '{}' to '{}' (attachment key {}).",
        filename,
        item.title(),
        created.key
    );
    Ok(())
}
