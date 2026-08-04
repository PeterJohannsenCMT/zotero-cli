use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::process::Command;

use crate::cache::Cache;
use crate::config::Config;
use crate::crossref;
use crate::editable_item::EditableItem;
use crate::keywords::Keywords;
use crate::sync;
use crate::tagging;
use crate::zotero_api::{ApiItem, ZoteroClient};

pub fn run(doi: &str) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::web(&config)?;

    let mut cache = Cache::load()?;
    sync::force_sync(&read_client, &write_client, &mut cache)
        .context("checking whether this DOI is already in your Zotero library")?;
    if let Some(existing) = find_existing_doi(&cache, doi) {
        bail!(
            "DOI '{}' is already present in your Zotero library as '{}' (key {})",
            doi,
            existing.title(),
            existing.key
        );
    }

    println!("Looking up {} on CrossRef...", doi);
    let work = crossref::fetch_work(doi)?;
    let canonical_doi = work.get("DOI").and_then(Value::as_str).unwrap_or(doi);
    if let Some(existing) = find_existing_doi(&cache, canonical_doi) {
        bail!(
            "DOI '{}' is already present in your Zotero library as '{}' (key {})",
            canonical_doi,
            existing.title(),
            existing.key
        );
    }

    let item_type = crossref::item_type_for_work(&work);
    let template = read_client.item_template(item_type)?;
    let mut item = crossref::fill_item_template(template, &work);

    let creators = crossref::creators(&work);
    let year = crossref::best_year(&work);
    let base = tagging::base_tag(&creators, year)?;
    let tag = tagging::unique_tag(&read_client, &base)?;
    item["tags"] = serde_json::json!([{ "tag": tag }]);

    let created = write_client.create_item(item)?;
    println!(
        "Added '{}' as [{}] (key {})",
        created.title(),
        tag,
        created.key
    );

    // Insert directly into the cache so it shows up immediately in `list`/
    // `search`/`open`/`edit` even within the freshness window that would
    // otherwise skip talking to the server again.
    cache.items.insert(created.key.clone(), created);
    cache.save()?;

    Ok(())
}

fn find_existing_doi<'a>(cache: &'a Cache, doi: &str) -> Option<&'a ApiItem> {
    let needle = normalize_doi(doi)?;
    cache.items.values().find(|item| {
        item.data
            .get("DOI")
            .and_then(Value::as_str)
            .and_then(normalize_doi)
            .as_deref()
            == Some(needle.as_str())
    })
}

fn normalize_doi(doi: &str) -> Option<String> {
    let mut doi = doi.trim();
    if doi.is_empty() {
        return None;
    }

    if doi
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("doi:"))
    {
        doi = doi[4..].trim_start();
    }
    for prefix in ["https://doi.org/", "http://doi.org/", "doi.org/"] {
        if doi
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
        {
            doi = &doi[prefix.len()..];
            break;
        }
    }

    let doi = doi.trim().trim_end_matches(['.', ',', ';']);
    if doi.is_empty() {
        None
    } else {
        Some(doi.to_ascii_lowercase())
    }
}

/// Add an item by hand, for cases CrossRef can't help with: books, webpages,
/// datasets, or a DOI lookup that came back empty/wrong. Opens a blank
/// metadata form (the same field set `zotero edit` uses) in `$EDITOR`.
pub fn run_manual(item_type: &str) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::web(&config)?;

    let template = read_client
        .item_template(item_type)
        .with_context(|| format!("fetching item template for type '{}'", item_type))?;

    let editable = EditableItem::blank();
    let toml_text =
        toml::to_string_pretty(&editable).context("serializing blank item for editing")?;

    let tmp_path = std::env::temp_dir().join(format!("zotero-cli-add-{}.toml", std::process::id()));
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
        bail!("editor exited with a non-zero status; aborting add");
    }

    let edited_text = std::fs::read_to_string(&tmp_path)
        .with_context(|| format!("reading back {}", tmp_path.display()))?;
    let _ = std::fs::remove_file(&tmp_path);

    let edited: EditableItem = toml::from_str(&edited_text).context("parsing edited TOML")?;
    if edited.title.trim().is_empty() {
        bail!("aborting: no title given");
    }

    let item = edited.fill_template(template);
    let mut created = write_client.create_item(item)?;

    match assign_citation_tag(&read_client, &write_client, &mut created) {
        Ok(Some(tag)) => println!(
            "Added '{}' as [{}] (key {})",
            created.title(),
            tag,
            created.key
        ),
        Ok(None) => println!(
            "Added '{}' (key {}) — no author/date given, so no AuthorYear tag was assigned; \
             add them with `zotero edit {}` and `zotero sync` will backfill one",
            created.title(),
            created.key,
            created.key
        ),
        Err(err) => eprintln!(
            "warning: added '{}' (key {}) but failed to assign an AuthorYear tag: {:#}",
            created.title(),
            created.key,
            err
        ),
    }

    if !edited.keywords.is_empty() {
        let mut keyword_store = Keywords::load()?;
        keyword_store.set(&created.key, edited.keywords.clone());
        keyword_store.save()?;
    }

    // Insert directly into the cache so it shows up immediately in `list`/
    // `search`/`open`/`edit` even within the freshness window that would
    // otherwise skip talking to the server again.
    let mut cache = Cache::load()?;
    cache.items.insert(created.key.clone(), created);
    cache.save()?;

    Ok(())
}

/// Derive an AuthorYear tag from a just-created item's creators/date (same
/// derivation `zotero sync`'s backfill uses) and write it back to Zotero.
/// Returns `None` (not an error) if the item doesn't have enough
/// information yet to derive one.
fn assign_citation_tag(
    read_client: &ZoteroClient,
    write_client: &ZoteroClient,
    item: &mut ApiItem,
) -> Result<Option<String>> {
    let Some(base) = tagging::derive_base_tag_from_item(item) else {
        return Ok(None);
    };
    let tag = tagging::unique_tag(read_client, &base)?;

    let mut tags = item.tags();
    tags.push(tag.clone());
    let tags_value: Vec<serde_json::Value> = tags
        .iter()
        .map(|t| serde_json::json!({ "tag": t }))
        .collect();
    let new_version = write_client.update_item(
        &item.key,
        item.version,
        serde_json::json!({ "tags": tags_value.clone() }),
    )?;

    if let Some(obj) = item.data.as_object_mut() {
        obj.insert("tags".to_string(), serde_json::Value::Array(tags_value));
    }
    item.version = new_version;

    Ok(Some(tag))
}
