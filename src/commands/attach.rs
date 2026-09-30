use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::path::Path;

use crate::cache::Cache;
use crate::commands::add;
use crate::config::Config;
use crate::crossref;
use crate::sync;
use crate::zotero_api::{ApiItem, ZoteroClient};

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
    // PDF uploads use Zotero's Web API storage authorization/register flow;
    // keep this command on the Web client until local file-import support is added.
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

    Ok(())
}

pub fn find_pdf(identifier: &str) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    // PDF uploads use Zotero's Web API storage authorization/register flow;
    // keep this command on the Web client like `attach-pdf`.
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

    if pdf_attachment(&cache, &item).is_some() {
        return Ok(());
    }

    let doi = item
        .data
        .get("DOI")
        .and_then(Value::as_str)
        .filter(|doi| !doi.trim().is_empty())
        .ok_or_else(|| anyhow!("'{}' has no DOI to look up on CrossRef", item.title()))?;

    let work = crossref::fetch_work(doi)?;
    let pdf_candidates = add::pdf_candidates_for_work(doi, &work);
    if pdf_candidates.is_empty() {
        bail!("could not find a direct PDF URL for this DOI");
    }
    let tag = item.citation_key().unwrap_or_else(|| item.key.clone());
    let attachment =
        add::download_and_attach_pdf(&read_client, &write_client, &item, &tag, &pdf_candidates)
            .with_context(|| format!("attaching PDF to '{}'", item.title()))?;

    cache
        .items
        .insert(attachment.key.clone(), attachment.clone());
    cache.save()?;

    Ok(())
}

fn pdf_attachment<'a>(cache: &'a Cache, item: &ApiItem) -> Option<&'a ApiItem> {
    cache.items.values().find(|candidate| {
        candidate.parent_item() == Some(item.key.as_str())
            && candidate.item_type() == "attachment"
            && candidate
                .data
                .get("contentType")
                .and_then(Value::as_str)
                .map(|content_type| content_type == "application/pdf")
                .unwrap_or(false)
    })
}
