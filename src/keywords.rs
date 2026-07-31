use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Free-form keywords the user attaches to items via `zotero edit`. These
/// are purely local (Zotero's item schema has no "keywords" field, so they
/// can't be synced through the Web API) and are stored separately from
/// `cache.json`, which is disposable and gets deleted to force a resync.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Keywords(HashMap<String, Vec<String>>);

impl Keywords {
    fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("could not determine config directory")?
            .join("zotero-cli");
        Ok(dir.join("keywords.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(serde_json::from_str(&text).unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).context("serializing keywords")?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn for_item(&self, key: &str) -> Vec<String> {
        self.0.get(key).cloned().unwrap_or_default()
    }

    pub fn set(&mut self, key: &str, keywords: Vec<String>) {
        if keywords.is_empty() {
            self.0.remove(key);
        } else {
            self.0.insert(key.to_string(), keywords);
        }
    }

    pub fn all(&self) -> &HashMap<String, Vec<String>> {
        &self.0
    }
}
