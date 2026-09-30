use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::config::Config;
use crate::http::{self, HttpClient};
use crate::tagging;

const PAGE_LIMIT: usize = 100;
const WEB_API_ROOT: &str = "https://api.zotero.org";
const LOCAL_API_ROOT: &str = "http://localhost:23119/api";

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
    api_root: String,
    base_url: String,
    source: ApiSource,
    fallback: Option<Box<ZoteroClient>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiSource {
    Local,
    Web,
}

fn version_header(resp: &ureq::Response) -> Option<i64> {
    resp.header("last-modified-version")
        .or_else(|| resp.header("zotero-library-version"))
        .and_then(|v| v.parse::<i64>().ok())
}

impl ZoteroClient {
    pub fn web(config: &Config) -> Result<Self> {
        let http = HttpClient::new(
            "zotero-cli/0.1",
            vec![
                ("Zotero-API-Version".to_string(), "3".to_string()),
                ("Zotero-API-Key".to_string(), config.api_key.clone()),
            ],
        );
        Ok(Self {
            http,
            api_root: WEB_API_ROOT.to_string(),
            base_url: config.library_base_url(),
            source: ApiSource::Web,
            fallback: None,
        })
    }

    pub fn local(config: &Config) -> Result<Self> {
        let http = HttpClient::new(
            "zotero-cli/0.1",
            vec![("Zotero-API-Version".to_string(), "3".to_string())],
        );
        Ok(Self {
            http,
            api_root: LOCAL_API_ROOT.to_string(),
            base_url: config.local_library_base_url(),
            source: ApiSource::Local,
            fallback: None,
        })
    }

    /// Prefer Zotero Desktop's local API for reads, but fall back to the Web
    /// API if the local server is unavailable or disabled.
    pub fn read_preferred(config: &Config) -> Result<Self> {
        let mut local = Self::local(config)?;
        local.fallback = Some(Box::new(Self::web(config)?));
        Ok(local)
    }

    /// Prefer Zotero Desktop's local API for writes too, falling back to the
    /// Web API if the local server is unavailable or rejects the operation.
    /// This avoids rate-limited Web API calls when Zotero Desktop is running,
    /// while preserving the old behavior when it is not.
    pub fn write_preferred(config: &Config) -> Result<Self> {
        let mut local = Self::local(config)?;
        local.fallback = Some(Box::new(Self::web(config)?));
        Ok(local)
    }

    fn is_local(&self) -> bool {
        self.source == ApiSource::Local
    }

    fn with_fallback<T>(
        &self,
        action: impl Fn(&ZoteroClient) -> Result<T>,
        fallback_context: &str,
    ) -> Result<T> {
        match action(self) {
            Ok(value) => Ok(value),
            Err(err) if self.is_local() => {
                if let Some(fallback) = &self.fallback {
                    action(fallback)
                        .with_context(|| format!("falling back to Zotero Web API after local API failed while {fallback_context}: {err:#}"))
                } else {
                    Err(err)
                }
            }
            Err(err) => Err(err),
        }
    }

    fn with_read_fallback<T>(
        &self,
        action: impl Fn(&ZoteroClient) -> Result<T>,
        fallback_context: &str,
    ) -> Result<T> {
        self.with_fallback(action, fallback_context)
    }

    fn with_write_fallback<T>(
        &self,
        action: impl Fn(&ZoteroClient) -> Result<T>,
        fallback_context: &str,
    ) -> Result<T> {
        self.with_fallback(action, fallback_context)
    }

    /// GETs every page of `path`, returning the collected JSON values plus the
    /// library version reported on the first response (if any).
    fn paginated_get(&self, path: &str, extra_query: &str) -> Result<(Vec<Value>, Option<i64>)> {
        self.with_read_fallback(
            |client| client.paginated_get_once(path, extra_query),
            &format!("GET {}", path),
        )
    }

    fn paginated_get_once(
        &self,
        path: &str,
        extra_query: &str,
    ) -> Result<(Vec<Value>, Option<i64>)> {
        let mut results = Vec::new();
        let mut start = 0usize;
        let mut version = None;
        loop {
            let url = self.paginated_url(path, extra_query, start);
            let resp = http::response(self.http.get(&url).call()).context("request failed")?;
            let status = resp.status();
            if !http::is_success(status) {
                let body = resp.into_string().unwrap_or_default();
                bail!("GET {} failed: {} {}", path, status, body);
            }
            if version.is_none() {
                version = version_header(&resp);
            }
            let batch: Vec<Value> = resp.into_json().context("parsing JSON response")?;
            let got = batch.len();
            results.extend(batch);
            if self.is_local() || got < PAGE_LIMIT {
                break;
            }
            start += PAGE_LIMIT;
        }
        Ok((results, version))
    }

    fn paginated_url(&self, path: &str, extra_query: &str, start: usize) -> String {
        let mut params = vec!["format=json".to_string()];
        if !self.is_local() {
            params.push(format!("limit={}", PAGE_LIMIT));
            params.push(format!("start={}", start));
        }
        if !extra_query.is_empty() {
            params.push(extra_query.to_string());
        }
        format!("{}{}?{}", self.base_url, path, params.join("&"))
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
        self.with_read_fallback(
            |client| client.get_collection_once(key),
            "fetching a collection",
        )
    }

    fn get_collection_once(&self, key: &str) -> Result<Collection> {
        let url = format!("{}/collections/{}?format=json", self.base_url, key);
        let resp = http::response(self.http.get(&url).call()).context("request failed")?;
        let status = resp.status();
        if !http::is_success(status) {
            let body = resp.into_string().unwrap_or_default();
            bail!("fetching collection failed: {} {}", status, body);
        }
        resp.into_json().context("parsing collection response")
    }

    /// Fetch a single item's current state directly from the server.
    pub fn get_item(&self, key: &str) -> Result<ApiItem> {
        self.with_read_fallback(|client| client.get_item_once(key), "fetching an item")
    }

    fn get_item_once(&self, key: &str) -> Result<ApiItem> {
        let url = format!("{}/items/{}?format=json", self.base_url, key);
        let resp = http::response(self.http.get(&url).call()).context("request failed")?;
        let status = resp.status();
        if !http::is_success(status) {
            let body = resp.into_string().unwrap_or_default();
            bail!("fetching item failed: {} {}", status, body);
        }
        resp.into_json().context("parsing item response")
    }

    /// Keys of items/collections/tags permanently deleted since `since`.
    pub fn deleted_since(&self, since: i64) -> Result<Value> {
        self.with_read_fallback(
            |client| client.deleted_since_once(since),
            "fetching deleted keys",
        )
    }

    fn deleted_since_once(&self, since: i64) -> Result<Value> {
        let url = format!("{}/deleted?since={}", self.base_url, since);
        let resp = http::response(self.http.get(&url).call()).context("request failed")?;
        if !http::is_success(resp.status()) {
            bail!("GET /deleted failed: {}", resp.status());
        }
        resp.into_json().context("parsing deleted response")
    }

    pub fn item_template(&self, item_type: &str) -> Result<Value> {
        self.with_read_fallback(
            |client| client.item_template_once(item_type),
            "fetching an item template",
        )
    }

    fn item_template_once(&self, item_type: &str) -> Result<Value> {
        let url = format!(
            "{}/items/new?itemType={}",
            self.api_root,
            urlencoding::encode(item_type)
        );
        let resp = http::response(self.http.get(&url).call()).context("request failed")?;
        if !http::is_success(resp.status()) {
            bail!("failed to fetch item template for type '{}'", item_type);
        }
        resp.into_json().context("parsing item template")
    }

    pub fn attachment_template(&self, link_mode: &str) -> Result<Value> {
        self.with_read_fallback(
            |client| client.attachment_template_once(link_mode),
            "fetching an attachment template",
        )
    }

    fn attachment_template_once(&self, link_mode: &str) -> Result<Value> {
        let url = format!(
            "{}/items/new?itemType=attachment&linkMode={}",
            self.api_root,
            urlencoding::encode(link_mode)
        );
        let resp = http::response(self.http.get(&url).call()).context("request failed")?;
        if !http::is_success(resp.status()) {
            bail!(
                "failed to fetch attachment template for linkMode '{}'",
                link_mode
            );
        }
        resp.into_json().context("parsing attachment template")
    }

    /// Create one item; returns the created ApiItem (with key/version).
    pub fn create_item(&self, item: Value) -> Result<ApiItem> {
        self.with_write_fallback(
            |client| client.create_item_once(item.clone()),
            "creating an item",
        )
    }

    fn create_item_once(&self, item: Value) -> Result<ApiItem> {
        let url = format!("{}/items", self.base_url);
        let body = serde_json::json!([item]);
        let resp =
            http::response(self.http.post(&url).send_json(body)).context("request failed")?;
        let status = resp.status();
        let text = resp.into_string().context("reading response body")?;
        if !http::is_success(status) {
            bail!("creating item failed: {} {}", status, text);
        }
        let parsed: Value = serde_json::from_str(&text).context("parsing create response")?;
        let successful = parsed
            .get("successful")
            .and_then(|v| v.get("0"))
            .ok_or_else(|| anyhow!("item creation did not report success: {}", text))?;
        serde_json::from_value(successful.clone()).context("parsing created item")
    }

    pub fn create_pdf_attachment(
        &self,
        parent_key: &str,
        mut template: Value,
        title: &str,
        filename: &str,
    ) -> Result<ApiItem> {
        template["parentItem"] = Value::String(parent_key.to_string());
        template["title"] = Value::String(title.to_string());
        template["contentType"] = Value::String("application/pdf".to_string());
        template["filename"] = Value::String(filename.to_string());
        self.create_item(template)
    }

    /// Upload a file for a just-created attachment. Returns the updated
    /// attachment/library version when Zotero reports one. If Zotero already
    /// has the file bytes, it associates them during authorization and no
    /// upload/register step is needed.
    pub fn upload_new_attachment_file(
        &self,
        attachment_key: &str,
        path: &Path,
        filename: &str,
    ) -> Result<Option<i64>> {
        let bytes =
            std::fs::read(path).with_context(|| format!("reading PDF {}", path.display()))?;
        let metadata =
            std::fs::metadata(path).with_context(|| format!("stat PDF {}", path.display()))?;
        let md5 = md5_hex(&bytes);
        let mtime = metadata
            .modified()
            .context("reading PDF modification time")?
            .duration_since(UNIX_EPOCH)
            .context("PDF modification time is before Unix epoch")?
            .as_millis()
            .to_string();
        let filesize = bytes.len().to_string();

        let auth_url = format!("{}/items/{}/file", self.base_url, attachment_key);
        let form = [
            ("md5", md5.as_str()),
            ("filename", filename),
            ("filesize", filesize.as_str()),
            ("mtime", mtime.as_str()),
        ];
        let resp = http::response(
            self.http
                .post(&auth_url)
                .set("If-None-Match", "*")
                .send_form(&form),
        )
        .context("requesting Zotero upload authorization")?;
        let status = resp.status();
        let text = resp
            .into_string()
            .context("reading upload authorization body")?;
        if !http::is_success(status) {
            bail!("upload authorization failed: {} {}", status, text);
        }
        let auth: Value = serde_json::from_str(&text).context("parsing upload authorization")?;
        if auth.get("exists").and_then(Value::as_i64) == Some(1) {
            return Ok(None);
        }

        let upload_url = auth
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("upload authorization response has no url"))?;
        let content_type = auth
            .get("contentType")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("upload authorization response has no contentType"))?;
        let prefix = auth
            .get("prefix")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("upload authorization response has no prefix"))?;
        let suffix = auth
            .get("suffix")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("upload authorization response has no suffix"))?;
        let upload_key = auth
            .get("uploadKey")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("upload authorization response has no uploadKey"))?;

        let mut upload_body = Vec::with_capacity(prefix.len() + bytes.len() + suffix.len());
        upload_body.extend_from_slice(prefix.as_bytes());
        upload_body.extend_from_slice(&bytes);
        upload_body.extend_from_slice(suffix.as_bytes());

        let upload_http = HttpClient::new("zotero-cli/0.1", Vec::new());
        let upload_resp = http::response(
            upload_http
                .post(upload_url)
                .set("Content-Type", content_type)
                .send_bytes(&upload_body),
        )
        .context("uploading PDF bytes")?;
        let upload_status = upload_resp.status();
        if upload_status != 201 {
            let body = upload_resp.into_string().unwrap_or_default();
            bail!("PDF upload failed: {} {}", upload_status, body);
        }

        let register_resp = http::response(
            self.http
                .post(&auth_url)
                .set("If-None-Match", "*")
                .send_form(&[("upload", upload_key)]),
        )
        .context("registering Zotero file upload")?;
        let register_status = register_resp.status();
        if register_status != 204 {
            let body = register_resp.into_string().unwrap_or_default();
            bail!(
                "registering file upload failed: {} {}",
                register_status,
                body
            );
        }
        Ok(version_header(&register_resp))
    }

    pub fn download_attachment_file(&self, attachment_key: &str) -> Result<Vec<u8>> {
        self.with_read_fallback(
            |client| client.download_attachment_file_once(attachment_key),
            "downloading an attachment file",
        )
    }

    fn download_attachment_file_once(&self, attachment_key: &str) -> Result<Vec<u8>> {
        const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

        let url = format!("{}/items/{}/file", self.base_url, attachment_key);
        let resp = http::response(self.http.get(&url).call()).context("request failed")?;
        let status = resp.status();
        if !http::is_success(status) {
            bail!(
                "downloading attachment file failed: {} {}",
                status,
                resp.status_text()
            );
        }

        let mut bytes = Vec::new();
        resp.into_reader()
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("reading attachment file")?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            bail!(
                "attachment file exceeded {} MB",
                MAX_FILE_BYTES / 1024 / 1024
            );
        }
        Ok(bytes)
    }

    /// Partially update an item. `patch` should contain only the changed
    /// fields. Returns the item's new library version.
    pub fn update_item(&self, key: &str, version: i64, patch: Value) -> Result<i64> {
        self.with_write_fallback(
            |client| client.update_item_once(key, version, patch.clone()),
            "updating an item",
        )
    }

    fn update_item_once(&self, key: &str, version: i64, patch: Value) -> Result<i64> {
        let url = format!("{}/items/{}", self.base_url, key);
        let resp = http::response(
            self.http
                .patch(&url)
                .set("If-Unmodified-Since-Version", &version.to_string())
                .send_json(patch),
        )
        .context("request failed")?;
        let status = resp.status();
        if !http::is_success(status) {
            let body = resp.into_string().unwrap_or_default();
            bail!("updating item failed: {} {}", status, body);
        }
        Ok(version_header(&resp).unwrap_or(version + 1))
    }

    /// Delete an item. Returns the new library version.
    pub fn delete_item(&self, key: &str, version: i64) -> Result<i64> {
        self.with_write_fallback(
            |client| client.delete_item_once(key, version),
            "deleting an item",
        )
    }

    fn delete_item_once(&self, key: &str, version: i64) -> Result<i64> {
        let url = format!("{}/items/{}", self.base_url, key);
        let resp = http::response(
            self.http
                .delete(&url)
                .set("If-Unmodified-Since-Version", &version.to_string())
                .call(),
        )
        .context("request failed")?;
        let status = resp.status();
        if !http::is_success(status) {
            let body = resp.into_string().unwrap_or_default();
            bail!("deleting item failed: {} {}", status, body);
        }
        Ok(version_header(&resp).unwrap_or(version + 1))
    }

    /// Partially update a collection. `patch` should contain only the
    /// changed fields (e.g. `{"name": "New Name"}`). Returns the
    /// collection's new library version.
    pub fn update_collection(&self, key: &str, version: i64, patch: Value) -> Result<i64> {
        self.with_write_fallback(
            |client| client.update_collection_once(key, version, patch.clone()),
            "updating a collection",
        )
    }

    fn update_collection_once(&self, key: &str, version: i64, patch: Value) -> Result<i64> {
        let url = format!("{}/collections/{}", self.base_url, key);
        let resp = http::response(
            self.http
                .patch(&url)
                .set("If-Unmodified-Since-Version", &version.to_string())
                .send_json(patch),
        )
        .context("request failed")?;
        let status = resp.status();
        if !http::is_success(status) {
            let body = resp.into_string().unwrap_or_default();
            bail!("updating collection failed: {} {}", status, body);
        }
        Ok(version_header(&resp).unwrap_or(version + 1))
    }

    /// Fetch a single item's BibTeX export, as rendered by Zotero (plain
    /// text, one entry). The citekey Zotero generates is not touched here —
    /// callers that want the item's AuthorYear tag as the citekey need to
    /// rewrite it themselves.
    pub fn item_bibtex(&self, key: &str) -> Result<String> {
        self.with_read_fallback(
            |client| client.item_bibtex_once(key),
            "fetching item BibTeX",
        )
    }

    fn item_bibtex_once(&self, key: &str) -> Result<String> {
        let url = format!("{}/items/{}?format=bibtex", self.base_url, key);
        let resp = http::response(self.http.get(&url).call()).context("request failed")?;
        let status = resp.status();
        let text = resp.into_string().context("reading response body")?;
        if !http::is_success(status) {
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

fn md5_hex(input: &[u8]) -> String {
    let mut msg = input.to_vec();
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let j = i * 4;
            *word = u32::from_le_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | (!b & d), i)
            } else if i < 32 {
                ((d & b) | (!d & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | !d), (7 * i) % 16)
            };
            let tmp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = tmp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut digest = Vec::with_capacity(16);
    digest.extend_from_slice(&a0.to_le_bytes());
    digest.extend_from_slice(&b0.to_le_bytes());
    digest.extend_from_slice(&c0.to_le_bytes());
    digest.extend_from_slice(&d0.to_le_bytes());
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::{ZoteroClient, md5_hex};
    use crate::config::Config;

    fn test_config() -> Config {
        Config {
            library_type: "user".to_string(),
            library_id: "12345".to_string(),
            api_key: "abc".to_string(),
            zotero_data_dir: None,
            current_collection_key: None,
        }
    }

    #[test]
    fn md5_hex_matches_known_answer() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }

    #[test]
    fn local_and_web_clients_use_expected_library_urls() {
        let config = test_config();
        let local = ZoteroClient::local(&config).unwrap();
        let web = ZoteroClient::web(&config).unwrap();

        assert_eq!(local.base_url, "http://localhost:23119/api/users/12345");
        assert_eq!(web.base_url, "https://api.zotero.org/users/12345");
    }

    #[test]
    fn web_paginated_urls_include_limit_and_start() {
        let client = ZoteroClient::web(&test_config()).unwrap();

        assert_eq!(
            client.paginated_url("/items", "since=10", 100),
            "https://api.zotero.org/users/12345/items?format=json&limit=100&start=100&since=10"
        );
    }

    #[test]
    fn local_paginated_urls_omit_limit_and_start() {
        let client = ZoteroClient::local(&test_config()).unwrap();

        assert_eq!(
            client.paginated_url("/items", "since=10", 100),
            "http://localhost:23119/api/users/12345/items?format=json&since=10"
        );
    }
}
