use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::http::{self, HttpClient};

/// Fetch raw CrossRef work metadata (the "message" object) for a DOI.
pub fn fetch_work(doi: &str) -> Result<Value> {
    let doi = doi
        .trim()
        .trim_start_matches("https://doi.org/")
        .trim_start_matches("doi.org/");
    let url = format!(
        "https://api.crossref.org/works/{}",
        urlencoding::encode(doi)
    );
    let http = HttpClient::new("zotero-cli/0.1 (mailto:noreply@example.com)", Vec::new());
    let resp = http::response(http.get(&url).call()).context("request to CrossRef failed")?;
    if !http::is_success(resp.status()) {
        bail!(
            "CrossRef lookup for DOI '{}' failed: {}",
            doi,
            resp.status()
        );
    }
    let body: Value = resp.into_json().context("parsing CrossRef response")?;
    body.get("message")
        .cloned()
        .context("CrossRef response missing 'message' field")
}

/// Map a CrossRef work to the Zotero item type it should be created as.
pub fn item_type_for_work(work: &Value) -> &'static str {
    match work.get("type").and_then(Value::as_str).unwrap_or("") {
        "journal-article" => "journalArticle",
        "proceedings-article" => "conferencePaper",
        "book-chapter" => "bookSection",
        "book" | "monograph" | "edited-book" => "book",
        "posted-content" => "preprint",
        "report" => "report",
        "dissertation" => "thesis",
        "dataset" => "dataset",
        _ => "journalArticle",
    }
}

fn date_parts_to_string(date_obj: Option<&Value>) -> String {
    let Some(parts) = date_obj
        .and_then(|d| d.get("date-parts"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_array)
    else {
        return String::new();
    };
    let nums: Vec<i64> = parts.iter().filter_map(Value::as_i64).collect();
    match nums.len() {
        0 => String::new(),
        1 => format!("{}", nums[0]),
        2 => format!("{}-{:02}", nums[0], nums[1]),
        _ => format!("{}-{:02}-{:02}", nums[0], nums[1], nums[2]),
    }
}

pub fn best_year(work: &Value) -> Option<i64> {
    for key in ["published-print", "published-online", "issued", "created"] {
        if let Some(y) = work
            .get(key)
            .and_then(|d| d.get("date-parts"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_array)
            .and_then(|p| p.first())
            .and_then(Value::as_i64)
        {
            return Some(y);
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct CreatorName {
    pub last_name: String,
    pub first_name: String,
}

pub fn creators(work: &Value) -> Vec<CreatorName> {
    work.get("author")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|a| CreatorName {
                    last_name: a
                        .get("family")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    first_name: a
                        .get("given")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A direct PDF URL advertised by CrossRef, usually intended for text/data
/// mining. Many works do not expose one here; callers should treat `None` as
/// normal rather than an error.
pub fn pdf_urls(work: &Value) -> Vec<String> {
    work.get("link")
        .and_then(Value::as_array)
        .map(|links| {
            links
                .iter()
                .filter(|link| {
                    link.get("content-type")
                        .and_then(Value::as_str)
                        .is_some_and(|ct| ct.eq_ignore_ascii_case("application/pdf"))
                })
                .filter_map(|link| link.get("URL").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Fill a Zotero item template (from /items/new) with CrossRef metadata.
pub fn fill_item_template(mut template: Value, work: &Value) -> Value {
    let item_type = item_type_for_work(work);
    template["itemType"] = Value::String(item_type.to_string());

    if let Some(title) = work
        .get("title")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
    {
        template["title"] = Value::String(title.to_string());
    }

    let creator_list = creators(work);
    if template.get("creators").is_some() {
        let arr: Vec<Value> = creator_list
            .iter()
            .map(|c| {
                serde_json::json!({
                    "creatorType": "author",
                    "firstName": c.first_name,
                    "lastName": c.last_name,
                })
            })
            .collect();
        template["creators"] = Value::Array(arr);
    }

    let date_str = date_parts_to_string(
        work.get("published-print")
            .or_else(|| work.get("published-online"))
            .or_else(|| work.get("issued")),
    );
    if !date_str.is_empty() && template.get("date").is_some() {
        template["date"] = Value::String(date_str);
    }

    if let Some(container) = work
        .get("container-title")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        && template.get("publicationTitle").is_some()
    {
        template["publicationTitle"] = Value::String(container.to_string());
    }

    if let Some(vol) = work.get("volume").and_then(Value::as_str)
        && template.get("volume").is_some()
    {
        template["volume"] = Value::String(vol.to_string());
    }
    if let Some(issue) = work.get("issue").and_then(Value::as_str)
        && template.get("issue").is_some()
    {
        template["issue"] = Value::String(issue.to_string());
    }
    if let Some(page) = work
        .get("page")
        .or_else(|| work.get("article-number"))
        .and_then(Value::as_str)
        && template.get("pages").is_some()
    {
        template["pages"] = Value::String(page.to_string());
    }
    if let Some(doi) = work.get("DOI").and_then(Value::as_str)
        && template.get("DOI").is_some()
    {
        template["DOI"] = Value::String(doi.to_string());
    }
    if let Some(url) = work.get("URL").and_then(Value::as_str)
        && template.get("url").is_some()
    {
        template["url"] = Value::String(url.to_string());
    }
    if let Some(issn) = work
        .get("ISSN")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        && template.get("ISSN").is_some()
    {
        template["ISSN"] = Value::String(issn.to_string());
    }
    if let Some(publisher) = work.get("publisher").and_then(Value::as_str)
        && template.get("publisher").is_some()
    {
        template["publisher"] = Value::String(publisher.to_string());
    }

    template
}

#[cfg(test)]
mod tests {
    use super::{fill_item_template, pdf_urls};

    #[test]
    fn uses_article_number_as_pages_when_page_is_missing() {
        let template = serde_json::json!({
            "itemType": "journalArticle",
            "title": "",
            "pages": ""
        });
        let work = serde_json::json!({
            "type": "journal-article",
            "title": ["Test"],
            "article-number": "012345"
        });

        let item = fill_item_template(template, &work);
        assert_eq!(item["pages"], "012345");
    }

    #[test]
    fn prefers_page_over_article_number() {
        let template = serde_json::json!({
            "itemType": "journalArticle",
            "pages": ""
        });
        let work = serde_json::json!({
            "type": "journal-article",
            "page": "12-34",
            "article-number": "012345"
        });

        let item = fill_item_template(template, &work);
        assert_eq!(item["pages"], "12-34");
    }

    #[test]
    fn finds_crossref_pdf_link() {
        let work = serde_json::json!({
            "link": [
                {"content-type": "text/html", "URL": "https://example.test/page"},
                {"content-type": "application/pdf", "URL": "https://example.test/paper.pdf"}
            ]
        });

        assert_eq!(pdf_urls(&work), vec!["https://example.test/paper.pdf"]);
    }
}
