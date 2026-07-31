use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::process::Command;

use crate::cache::Cache;
use crate::config::Config;
use crate::sync;
use crate::zotero_api::ZoteroClient;

#[derive(Debug, Serialize, Deserialize)]
struct EditableCreator {
    #[serde(default = "default_creator_type")]
    creator_type: String,
    #[serde(default)]
    first_name: String,
    #[serde(default)]
    last_name: String,
}

fn default_creator_type() -> String {
    "author".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
struct EditableItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    publication_title: String,
    #[serde(default)]
    doi: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    abstract_note: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    creators: Vec<EditableCreator>,
}

impl EditableItem {
    fn from_data(data: &serde_json::Value) -> Self {
        let get_str = |key: &str| {
            data.get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let tags = data
            .get("tags")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.get("tag").and_then(serde_json::Value::as_str))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let creators = data
            .get("creators")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .map(|c| EditableCreator {
                        creator_type: c
                            .get("creatorType")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("author")
                            .to_string(),
                        first_name: c
                            .get("firstName")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        last_name: c
                            .get("lastName")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        EditableItem {
            title: get_str("title"),
            date: get_str("date"),
            publication_title: get_str("publicationTitle"),
            doi: get_str("DOI"),
            url: get_str("url"),
            abstract_note: get_str("abstractNote"),
            tags,
            creators,
        }
    }

    fn to_patch(&self) -> serde_json::Value {
        let tags: Vec<serde_json::Value> = self
            .tags
            .iter()
            .map(|t| serde_json::json!({ "tag": t }))
            .collect();
        let creators: Vec<serde_json::Value> = self
            .creators
            .iter()
            .map(|c| {
                serde_json::json!({
                    "creatorType": c.creator_type,
                    "firstName": c.first_name,
                    "lastName": c.last_name,
                })
            })
            .collect();
        serde_json::json!({
            "title": self.title,
            "date": self.date,
            "publicationTitle": self.publication_title,
            "DOI": self.doi,
            "url": self.url,
            "abstractNote": self.abstract_note,
            "tags": tags,
            "creators": creators,
        })
    }
}

pub fn run(identifier: &str) -> Result<()> {
    let config = Config::load()?;
    let client = ZoteroClient::from_config(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&client, &mut cache)?;

    let item = cache
        .find_by_identifier(identifier)
        .ok_or_else(|| anyhow!("no item found for '{}'", identifier))?
        .clone();
    let editable = EditableItem::from_data(&item.data);
    let toml_text = toml::to_string_pretty(&editable).context("serializing item for editing")?;

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
    let patch = edited.to_patch();

    let new_version = client.update_item(&item.key, item.version, patch.clone())?;

    if let Some(cached) = cache.items.get_mut(&item.key)
        && let Some(obj) = cached.data.as_object_mut()
        && let Some(patch_obj) = patch.as_object()
    {
        for (k, v) in patch_obj {
            obj.insert(k.clone(), v.clone());
        }
        cached.version = new_version;
    }
    cache.save()?;

    println!("Updated '{}'.", edited.title);

    Ok(())
}
