use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::{Client as HttpClient, Response};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::tagging;

const PAGE_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiItem {
    pub key: String,
    pub version: i64,
    #[serde(default)]
    pub data: Value,
}

impl ApiItem {
    pub fn title(&self) -> String {
        self.data
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)")
            .to_string()
    }

    pub fn item_type(&self) -> &str {
        self.data
            .get("itemType")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    pub fn tags(&self) -> Vec<String> {
        self.data
            .get("tags")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.get("tag").and_then(Value::as_str))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The AuthorYear-shaped tag used as this item's stable citation key, if any.
    pub fn citation_key(&self) -> Option<String> {
        tagging::find_author_year_tag(&self.tags())
    }

    pub fn date(&self) -> &str {
        self.data.get("date").and_then(Value::as_str).unwrap_or("")
    }

    pub fn parent_item(&self) -> Option<&str> {
        self.data.get("parentItem").and_then(Value::as_str)
    }

    pub fn in_collection(&self, collection_key: &str) -> bool {
        self.data
            .get("collections")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().any(|v| v.as_str() == Some(collection_key)))
            .unwrap_or(false)
    }

    pub fn creator_summary(&self) -> String {
        let creators = self
            .data
            .get("creators")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if creators.is_empty() {
            return String::new();
        }
        let last_names: Vec<String> = creators
            .iter()
            .filter_map(|c| {
                c.get("lastName")
                    .and_then(Value::as_str)
                    .or_else(|| c.get("name").and_then(Value::as_str))
            })
            .map(|s| s.to_string())
            .collect();
        match last_names.len() {
            0 => String::new(),
            1 => last_names[0].clone(),
            2 => format!("{} & {}", last_names[0], last_names[1]),
            _ => format!("{} et al.", last_names[0]),
        }
    }

    pub fn url_or_doi(&self) -> Option<String> {
        if let Some(doi) = self.data.get("DOI").and_then(Value::as_str)
            && !doi.is_empty()
        {
            return Some(format!("https://doi.org/{}", doi));
        }
        self.data
            .get("url")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub key: String,
    #[serde(default)]
    pub version: i64,
    #[serde(default)]
    pub data: Value,
}

impl Collection {
    pub fn name(&self) -> &str {
        self.data.get("name").and_then(Value::as_str).unwrap_or("")
    }

    /// The key of the collection this one is nested under, if any.
    pub fn parent_key(&self) -> Option<&str> {
        self.data.get("parentCollection").and_then(Value::as_str)
    }
}

pub struct ZoteroClient {
    http: HttpClient,
    base_url: String,
}

fn version_header(resp: &Response) -> Option<i64> {
    resp.headers()
        .get("last-modified-version")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
}

impl ZoteroClient {
    pub fn from_config(config: &Config) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("Zotero-API-Version", HeaderValue::from_static("3"));
        headers.insert(
            "Zotero-API-Key",
            HeaderValue::from_str(&config.api_key).context("invalid API key")?,
        );
        let http = HttpClient::builder()
            .default_headers(headers)
            .user_agent("zotero-cli/0.1")
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            base_url: config.library_base_url(),
        })
    }

    /// GETs every page of `path`, returning the collected JSON values plus the
    /// library version reported on the first response (if any).
    fn paginated_get(&self, path: &str, extra_query: &str) -> Result<(Vec<Value>, Option<i64>)> {
        let mut results = Vec::new();
        let mut start = 0usize;
        let mut version = None;
        loop {
            let sep = if extra_query.is_empty() { "" } else { "&" };
            let url = format!(
                "{}{}?format=json&limit={}&start={}{}{}",
                self.base_url, path, PAGE_LIMIT, start, sep, extra_query
            );
            let resp = self.http.get(&url).send().context("request failed")?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().unwrap_or_default();
                bail!("GET {} failed: {} {}", path, status, body);
            }
            if version.is_none() {
                version = version_header(&resp);
            }
            let batch: Vec<Value> = resp.json().context("parsing JSON response")?;
            let got = batch.len();
            results.extend(batch);
            if got < PAGE_LIMIT {
                break;
            }
            start += PAGE_LIMIT;
        }
        Ok((results, version))
    }

    /// Items (of every type, including attachments/notes) created or changed
    /// since `since`. Pass 0 to fetch the entire library. Returns the items
    /// plus the library version as of this fetch.
    pub fn items_since(&self, since: i64) -> Result<(Vec<Value>, i64)> {
        let query = format!("since={}", since);
        let (values, version) = self.paginated_get("/items", &query)?;
        Ok((values, version.unwrap_or(since)))
    }

    pub fn collections_since(&self, since: i64) -> Result<Vec<Value>> {
        let query = format!("since={}", since);
        let (values, _) = self.paginated_get("/collections", &query)?;
        Ok(values)
    }

    /// Fetch a single collection's current state directly from the server
    /// (bypassing the local cache), so callers can get its up-to-date
    /// version before a conditional update.
    pub fn get_collection(&self, key: &str) -> Result<Collection> {
        let url = format!("{}/collections/{}?format=json", self.base_url, key);
        let resp = self.http.get(&url).send().context("request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("fetching collection failed: {} {}", status, body);
        }
        resp.json().context("parsing collection response")
    }

    /// Keys of items/collections/tags permanently deleted since `since`.
    pub fn deleted_since(&self, since: i64) -> Result<Value> {
        let url = format!("{}/deleted?since={}", self.base_url, since);
        let resp = self.http.get(&url).send().context("request failed")?;
        if !resp.status().is_success() {
            bail!("GET /deleted failed: {}", resp.status());
        }
        resp.json().context("parsing deleted response")
    }

    pub fn item_template(&self, item_type: &str) -> Result<Value> {
        let url = format!(
            "https://api.zotero.org/items/new?itemType={}",
            urlencoding::encode(item_type)
        );
        let resp = self.http.get(&url).send().context("request failed")?;
        if !resp.status().is_success() {
            bail!("failed to fetch item template for type '{}'", item_type);
        }
        resp.json().context("parsing item template")
    }

    /// Create one item; returns the created ApiItem (with key/version).
    pub fn create_item(&self, item: Value) -> Result<ApiItem> {
        let url = format!("{}/items", self.base_url);
        let body = serde_json::json!([item]);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .context("request failed")?;
        let status = resp.status();
        let text = resp.text().context("reading response body")?;
        if !status.is_success() {
            bail!("creating item failed: {} {}", status, text);
        }
        let parsed: Value = serde_json::from_str(&text).context("parsing create response")?;
        let successful = parsed
            .get("successful")
            .and_then(|v| v.get("0"))
            .ok_or_else(|| anyhow!("item creation did not report success: {}", text))?;
        serde_json::from_value(successful.clone()).context("parsing created item")
    }

    /// Partially update an item. `patch` should contain only the changed
    /// fields. Returns the item's new library version.
    pub fn update_item(&self, key: &str, version: i64, patch: Value) -> Result<i64> {
        let url = format!("{}/items/{}", self.base_url, key);
        let resp = self
            .http
            .patch(&url)
            .header("If-Unmodified-Since-Version", version.to_string())
            .json(&patch)
            .send()
            .context("request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("updating item failed: {} {}", status, body);
        }
        Ok(version_header(&resp).unwrap_or(version + 1))
    }

    /// Partially update a collection. `patch` should contain only the
    /// changed fields (e.g. `{"name": "New Name"}`). Returns the
    /// collection's new library version.
    pub fn update_collection(&self, key: &str, version: i64, patch: Value) -> Result<i64> {
        let url = format!("{}/collections/{}", self.base_url, key);
        let resp = self
            .http
            .patch(&url)
            .header("If-Unmodified-Since-Version", version.to_string())
            .json(&patch)
            .send()
            .context("request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("updating collection failed: {} {}", status, body);
        }
        Ok(version_header(&resp).unwrap_or(version + 1))
    }

    /// Fetch a single item's BibTeX export, as rendered by Zotero (plain
    /// text, one entry). The citekey Zotero generates is not touched here —
    /// callers that want the item's AuthorYear tag as the citekey need to
    /// rewrite it themselves.
    pub fn item_bibtex(&self, key: &str) -> Result<String> {
        let url = format!("{}/items/{}?format=bibtex", self.base_url, key);
        let resp = self.http.get(&url).send().context("request failed")?;
        let status = resp.status();
        let text = resp.text().context("reading response body")?;
        if !status.is_success() {
            bail!(
                "fetching bibtex for item {} failed: {} {}",
                key,
                status,
                text
            );
        }
        Ok(text)
    }

    /// Search existing tags starting with `prefix` (case-insensitive).
    pub fn tags_starting_with(&self, prefix: &str) -> Result<Vec<String>> {
        let query = format!("q={}", urlencoding::encode(prefix));
        let (values, _) = self.paginated_get("/tags", &query)?;
        Ok(values
            .into_iter()
            .filter_map(|v| v.get("tag").and_then(Value::as_str).map(|s| s.to_string()))
            .collect())
    }
}
