use anyhow::{Result, anyhow};
use std::collections::HashSet;

use crate::cache::Cache;
use crate::config::Config;
use crate::sync;
use crate::zotero_api::{ApiItem, ZoteroClient};

pub fn run(
    identifiers: Vec<String>,
    subcollection: Option<String>,
    tag: Option<String>,
) -> Result<()> {
    let config = Config::load()?;
    let client = ZoteroClient::from_config(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&client, &mut cache)?;

    let use_filters = subcollection.is_some() || tag.is_some();
    let mut any_failed = false;

    let targets: Vec<ApiItem> = if !identifiers.is_empty() || !use_filters {
        let ids = if identifiers.is_empty() {
            crate::read_identifiers_from_stdin()?
        } else {
            identifiers
        };

        let mut seen = HashSet::new();
        let mut resolved = Vec::new();
        for id in &ids {
            match cache.find_by_identifier(id) {
                Some(item) if seen.insert(item.key.clone()) => resolved.push(item.clone()),
                Some(_) => {}
                None => {
                    eprintln!("error: no item found for '{}'", id);
                    any_failed = true;
                }
            }
        }
        resolved
    } else {
        let collection_key = match &subcollection {
            Some(name) => Some(cache.resolve_collection_by_name(name)?),
            None => None,
        };
        let items = cache.top_level_items(collection_key.as_deref(), tag.as_deref());
        if items.is_empty() {
            eprintln!("No items found.");
            return Ok(());
        }
        items.into_iter().cloned().collect()
    };

    let mut first = true;
    for item in &targets {
        let bibtex = match client.item_bibtex(&item.key) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("error: {:#} (for '{}')", err, item.key);
                any_failed = true;
                continue;
            }
        };

        let citekey = item.citation_key().unwrap_or_else(|| {
            // eprintln!(
            //     "warning: '{}' has no AuthorYear tag; using its raw key as the citekey",
            //     item.key
            // );
            item.key.clone()
        });

        match replace_citekey(&bibtex, &citekey) {
            Ok(entry) => {
                if !first {
                    println!();
                }
                first = false;
                println!("{}", entry.trim_end());
            }
            Err(err) => {
                eprintln!("error: {:#} (for '{}')", err, item.key);
                any_failed = true;
            }
        }
    }

    if any_failed {
        std::process::exit(1);
    }
    Ok(())
}

/// Replace the citekey in a Zotero-rendered BibTeX entry (`@type{oldkey,
/// ...}`) with `new_key`, leaving every other field untouched.
fn replace_citekey(entry: &str, new_key: &str) -> Result<String> {
    let open = entry
        .find('{')
        .ok_or_else(|| anyhow!("unexpected bibtex format (no '{{' found)"))?;
    let comma = entry[open + 1..]
        .find(',')
        .map(|i| i + open + 1)
        .ok_or_else(|| anyhow!("unexpected bibtex format (no ',' found)"))?;
    Ok(format!(
        "{}{}{}",
        &entry[..open + 1],
        new_key,
        &entry[comma..]
    ))
}
