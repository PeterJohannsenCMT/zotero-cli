use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A Zotero item's metadata, reshaped into a flat, hand-editable form used
/// for both `zotero edit` (pre-filled from an existing item) and
/// `zotero add --manual` (blank, filled into a freshly fetched item
/// template). Round-tripped through TOML in the user's `$EDITOR`.
#[derive(Debug, Serialize, Deserialize)]
pub struct EditableCreator {
    #[serde(default = "default_creator_type")]
    pub creator_type: String,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: String,
}

fn default_creator_type() -> String {
    "author".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EditableItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub publication_title: String,
    #[serde(default)]
    pub doi: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub abstract_note: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Local-only keywords, not synced to Zotero (it has no such field).
    /// Stored separately and merged back in after editing.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub creators: Vec<EditableCreator>,
}

impl EditableItem {
    pub fn from_data(data: &Value) -> Self {
        let get_str = |key: &str| {
            data.get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let tags = data
            .get("tags")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.get("tag").and_then(Value::as_str))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let creators = data
            .get("creators")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .map(|c| EditableCreator {
                        creator_type: c
                            .get("creatorType")
                            .and_then(Value::as_str)
                            .unwrap_or("author")
                            .to_string(),
                        first_name: c
                            .get("firstName")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        last_name: c
                            .get("lastName")
                            .and_then(Value::as_str)
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
            keywords: Vec::new(),
            creators,
        }
    }

    /// A blank item, ready to hand to the user's `$EDITOR` for `zotero add
    /// --manual`. Includes one empty creator row so the expected shape (an
    /// array of tables) is obvious without needing to consult docs.
    pub fn blank() -> Self {
        EditableItem {
            title: String::new(),
            date: String::new(),
            publication_title: String::new(),
            doi: String::new(),
            url: String::new(),
            abstract_note: String::new(),
            tags: Vec::new(),
            keywords: Vec::new(),
            creators: vec![EditableCreator {
                creator_type: default_creator_type(),
                first_name: String::new(),
                last_name: String::new(),
            }],
        }
    }

    pub fn to_patch(&self) -> Value {
        let tags: Vec<Value> = self
            .tags
            .iter()
            .map(|t| serde_json::json!({ "tag": t }))
            .collect();
        let creators: Vec<Value> = self
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

    /// Fill a freshly fetched item template (from `/items/new`) with these
    /// fields, only touching keys the template actually has — item types
    /// vary (a `webpage` has no `DOI`, a `book` has no `publicationTitle`,
    /// ...), and sending a field the type doesn't support gets the create
    /// request rejected outright.
    pub fn fill_template(&self, mut template: Value) -> Value {
        if template.get("title").is_some() {
            template["title"] = Value::String(self.title.clone());
        }
        if template.get("date").is_some() {
            template["date"] = Value::String(self.date.clone());
        }
        if template.get("publicationTitle").is_some() {
            template["publicationTitle"] = Value::String(self.publication_title.clone());
        }
        if template.get("DOI").is_some() {
            template["DOI"] = Value::String(self.doi.clone());
        }
        if template.get("url").is_some() {
            template["url"] = Value::String(self.url.clone());
        }
        if template.get("abstractNote").is_some() {
            template["abstractNote"] = Value::String(self.abstract_note.clone());
        }
        if template.get("creators").is_some() {
            let arr: Vec<Value> = self
                .creators
                .iter()
                .filter(|c| !c.first_name.is_empty() || !c.last_name.is_empty())
                .map(|c| {
                    serde_json::json!({
                        "creatorType": c.creator_type,
                        "firstName": c.first_name,
                        "lastName": c.last_name,
                    })
                })
                .collect();
            template["creators"] = Value::Array(arr);
        }
        if template.get("tags").is_some() {
            let arr: Vec<Value> = self
                .tags
                .iter()
                .map(|t| serde_json::json!({ "tag": t }))
                .collect();
            template["tags"] = Value::Array(arr);
        }
        template
    }
}
