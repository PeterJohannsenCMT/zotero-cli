use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::zotero_api::{ApiItem, Collection};

/// A local mirror of the library, keyed by item/collection key, plus the
/// library version we've synced up to. Persisted to disk so that repeated
/// `zotero` invocations only need to pull the delta since last time
/// (via the Zotero API's `since=` parameter) instead of the whole library.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Cache {
    #[serde(default)]
    pub library_version: i64,
    #[serde(default)]
    pub items: HashMap<String, ApiItem>,
    #[serde(default)]
    pub collections: HashMap<String, Collection>,
    /// Unix timestamp of the last time we actually talked to the server
    /// (as opposed to serving straight from this cache). See `sync::sync_library`.
    #[serde(default)]
    pub last_synced_at: u64,
}

impl Cache {
    fn path() -> Result<PathBuf> {
        let dir = dirs::cache_dir()
            .context("could not determine cache directory")?
            .join("zotero-cli");
        Ok(dir.join("cache.json"))
    }

    pub fn load() -> Result<Cache> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Cache::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading cache at {}", path.display()))?;
        // A corrupt or incompatible cache shouldn't be fatal: just resync from scratch.
        Ok(serde_json::from_str(&text).unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating cache directory {}", parent.display()))?;
        }
        let text = serde_json::to_string(self).context("serializing cache")?;
        std::fs::write(&path, text).with_context(|| format!("writing cache to {}", path.display()))
    }

    pub fn resolve_collection_by_name(&self, name: &str) -> Result<String> {
        let name_lower = name.to_lowercase();
        self.collections
            .values()
            .find(|c| c.name().to_lowercase() == name_lower)
            .map(|c| c.key.clone())
            .ok_or_else(|| anyhow!("no subcollection named '{}' found", name))
    }

    pub fn collection_name(&self, key: &str) -> Option<String> {
        self.collections.get(key).map(|c| c.name().to_string())
    }

    pub fn effective_collection_key(
        &self,
        explicit_name: Option<&str>,
        current_key: Option<&str>,
    ) -> Result<Option<String>> {
        if let Some(name) = explicit_name {
            return self.resolve_collection_by_name(name).map(Some);
        }
        if let Some(key) = current_key {
            if self.collections.contains_key(key) {
                return Ok(Some(key.to_string()));
            }
            return Err(anyhow!(
                "current subcollection no longer exists; run `zotero collections clear` or `zotero collections use <name>`"
            ));
        }
        Ok(None)
    }

    /// Look up an item by its raw Zotero key or its AuthorYear citation tag.
    pub fn find_by_identifier(&self, identifier: &str) -> Option<&ApiItem> {
        self.items.get(identifier).or_else(|| {
            self.items
                .values()
                .find(|it| it.citation_key().as_deref() == Some(identifier))
        })
    }

    /// Top-level (non-attachment, non-note) items, optionally restricted to a
    /// collection and/or a tag. Shared by `list` and `search`.
    pub fn top_level_items(
        &self,
        collection_key: Option<&str>,
        tag: Option<&str>,
    ) -> Vec<&ApiItem> {
        self.items
            .values()
            .filter(|it| it.item_type() != "attachment" && it.item_type() != "note")
            .filter(|it| it.parent_item().is_none())
            .filter(|it| collection_key.is_none_or(|ck| it.in_collection(ck)))
            .filter(|it| tag.is_none_or(|t| it.tags().iter().any(|x| x.eq_ignore_ascii_case(t))))
            .collect()
    }
}
