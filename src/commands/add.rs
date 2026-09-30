use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use crate::cache::Cache;
use crate::config::Config;
use crate::crossref;
use crate::editable_item::{EditableAuthor, EditableItem};
use crate::http;
use crate::sync;
use crate::tagging;
use crate::zotero_api::{ApiItem, ZoteroClient};

pub fn run(doi: &str, download_pdf: bool) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let preferred_write_client = ZoteroClient::write_preferred(&config)?;

    let mut cache = Cache::load()?;
    sync::force_sync(&read_client, &preferred_write_client, &mut cache)
        .context("checking whether this DOI is already in your Zotero library")?;
    if let Some(existing) = find_existing_doi(&cache, doi) {
        bail!(
            "DOI '{}' is already present in your Zotero library as '{}' (key {})",
            doi,
            existing.title(),
            existing.key
        );
    }

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
    let pdf_candidates = if download_pdf {
        pdf_candidates_for_work(canonical_doi, &work)
    } else {
        Vec::new()
    };

    let item_type = crossref::item_type_for_work(&work);
    let template = read_client.item_template(item_type)?;
    let mut item = crossref::fill_item_template(template, &work);

    let creators = crossref::creators(&work);
    let year = crossref::best_year(&work);
    let base = tagging::base_tag(&creators, year)?;
    let tag = tagging::unique_tag(&read_client, &base)?;
    item["tags"] = serde_json::json!([{ "tag": tag }]);

    // PDF uploads use Zotero's Web API storage authorization/register flow, so
    // if we'll attach one, create the parent item through the Web API too; that
    // guarantees the parent exists on the same side before creating its child.
    let web_write_client = if !pdf_candidates.is_empty() {
        Some(ZoteroClient::web(&config)?)
    } else {
        None
    };
    let create_client = web_write_client.as_ref().unwrap_or(&preferred_write_client);

    let created = create_client.create_item(item)?;

    if download_pdf {
        if pdf_candidates.is_empty() {
            eprintln!("warning: could not find a direct PDF URL for this DOI");
        } else {
            match download_and_attach_pdf(
                &read_client,
                web_write_client.as_ref().unwrap_or(&preferred_write_client),
                &created,
                &tag,
                &pdf_candidates,
            ) {
                Ok(attachment) => {
                    cache.items.insert(attachment.key.clone(), attachment);
                }
                Err(err) => eprintln!("warning: added item, but PDF attachment failed: {err:#}"),
            }
        }
    }

    // Insert directly into the cache so it shows up immediately in `list`/
    // `search`/`open`/`edit` even within the freshness window that would
    // otherwise skip talking to the server again.
    cache.items.insert(created.key.clone(), created);
    cache.save()?;

    Ok(())
}

pub(crate) fn download_and_attach_pdf(
    read_client: &ZoteroClient,
    write_client: &ZoteroClient,
    parent: &ApiItem,
    tag: &str,
    urls: &[String],
) -> Result<ApiItem> {
    let filename = pdf_filename(parent, tag);
    let tmp_path = std::env::temp_dir().join(format!(
        "zotero-cli-pdf-{}-{}",
        std::process::id(),
        filename
    ));

    let result = (|| {
        download_pdf_from_candidates(urls, &tmp_path)?;

        let template = read_client
            .attachment_template("imported_file")
            .context("fetching PDF attachment template")?;
        let title = filename.trim_end_matches(".pdf");
        let created = write_client
            .create_pdf_attachment(&parent.key, template, title, &filename)
            .context("creating Zotero attachment item")?;

        let reported_version =
            write_client.upload_new_attachment_file(&created.key, &tmp_path, &filename)?;
        let mut fresh = write_client.get_item(&created.key)?;
        if let Some(new_version) = reported_version {
            fresh.version = new_version;
        }
        Ok(fresh)
    })();

    let _ = std::fs::remove_file(&tmp_path);
    result
}

pub(crate) fn pdf_candidates_for_work(doi: &str, work: &Value) -> Vec<String> {
    let mut candidates = crossref::pdf_urls(work);
    if let Ok(discovered) = discover_pdf_urls_from_landing_page(doi) {
        candidates.extend(discovered);
    }
    dedup_urls(candidates)
}

fn download_pdf_from_candidates(urls: &[String], path: &Path) -> Result<()> {
    if urls.is_empty() {
        bail!("no PDF candidates found");
    }

    let mut failures = Vec::new();
    for url in urls {
        match download_pdf(url, path) {
            Ok(()) => return Ok(()),
            Err(err) => failures.push(format!("{}: {:#}", url, err)),
        }
    }

    bail!("all PDF candidates failed:\n{}", failures.join("\n"))
}

fn download_pdf(url: &str, path: &Path) -> Result<()> {
    const MAX_PDF_BYTES: u64 = 100 * 1024 * 1024;

    let client = http::HttpClient::new("Mozilla/5.0 (compatible; zotero-cli/0.1)", Vec::new());
    let resp = http::response(client.get(url).call()).context("requesting PDF")?;
    let status = resp.status();
    if !http::is_success(status) {
        bail!("PDF download failed: {} {}", status, resp.status_text());
    }

    let content_type = resp.header("content-type").unwrap_or("").to_string();
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(MAX_PDF_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading PDF download")?;
    if bytes.len() as u64 > MAX_PDF_BYTES {
        bail!("PDF download exceeded {} MB", MAX_PDF_BYTES / 1024 / 1024);
    }
    if !bytes.starts_with(b"%PDF-") {
        bail!(
            "downloaded file does not look like a PDF (Content-Type: {})",
            if content_type.is_empty() {
                "unknown"
            } else {
                content_type.as_str()
            }
        );
    }

    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn discover_pdf_urls_from_landing_page(doi: &str) -> Result<Vec<String>> {
    const MAX_HTML_BYTES: u64 = 5 * 1024 * 1024;

    let doi = normalize_doi(doi).unwrap_or_else(|| doi.trim().to_string());
    let url = format!("https://doi.org/{}", urlencoding::encode(&doi));
    let client = http::HttpClient::new("Mozilla/5.0 (compatible; zotero-cli/0.1)", Vec::new());
    let resp = http::response(
        client
            .get(&url)
            .set("Accept", "text/html,application/pdf;q=0.9,*/*;q=0.8")
            .call(),
    )
    .context("requesting DOI landing page")?;
    let status = resp.status();
    if !http::is_success(status) {
        bail!(
            "DOI landing page request failed: {} {}",
            status,
            resp.status_text()
        );
    }

    let final_url = resp.get_url().to_string();
    let content_type = resp.header("content-type").unwrap_or("").to_string();
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(MAX_HTML_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading DOI landing page")?;
    if bytes.len() as u64 > MAX_HTML_BYTES {
        bail!(
            "DOI landing page exceeded {} MB",
            MAX_HTML_BYTES / 1024 / 1024
        );
    }
    if bytes.starts_with(b"%PDF-")
        || content_type
            .to_ascii_lowercase()
            .contains("application/pdf")
    {
        return Ok(vec![final_url]);
    }

    let html = String::from_utf8_lossy(&bytes);
    let mut urls = extract_pdf_urls_from_html(&html, &final_url, &doi);
    if final_url.contains("journals.aps.org") && final_url.contains("/abstract/") {
        urls.push(final_url.replace("/abstract/", "/pdf/"));
    }
    Ok(dedup_urls(urls))
}

fn extract_pdf_urls_from_html(html: &str, base_url: &str, doi: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for tag in html.match_indices('<').filter_map(|(idx, _)| {
        let rest = &html[idx..];
        let end = rest.find('>')?;
        Some(&rest[..=end])
    }) {
        let lower = tag.to_ascii_lowercase();
        if lower.contains("citation_pdf_url") {
            if let Some(url) = attr_value(tag, "content") {
                urls.push(absolute_url(base_url, &decode_html_attr(&url)));
            }
        }
        if lower.contains("href=") {
            if let Some(href) = attr_value(tag, "href") {
                let href = decode_html_attr(&href);
                let lower_href = href.to_ascii_lowercase();
                if lower_href.contains("/pdf/") && lower_href.contains(doi) {
                    urls.push(absolute_url(base_url, &href));
                }
            }
        }
    }
    urls
}

fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search_from = 0;
    loop {
        let pos = lower[search_from..].find(attr)? + search_from;
        let after_attr = pos + attr.len();
        if !lower[..pos]
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            let mut rest = &tag[after_attr..];
            rest = rest.trim_start();
            if let Some(after_equals) = rest.strip_prefix('=') {
                rest = after_equals.trim_start();
                let quote = rest.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let rest = &rest[quote.len_utf8()..];
                    let end = rest.find(quote)?;
                    return Some(rest[..end].to_string());
                }
                let end = rest
                    .find(|ch: char| ch.is_whitespace() || ch == '>')
                    .unwrap_or(rest.len());
                return Some(rest[..end].to_string());
            }
        }
        search_from = after_attr;
    }
}

fn absolute_url(base_url: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    if url.starts_with("//") {
        let scheme = base_url
            .split_once(':')
            .map(|(scheme, _)| scheme)
            .unwrap_or("https");
        return format!("{}:{}", scheme, url);
    }
    if url.starts_with('/') {
        if let Some((scheme, rest)) = base_url.split_once("://") {
            let host = rest.split('/').next().unwrap_or(rest);
            return format!("{}://{}{}", scheme, host, url);
        }
    }
    let prefix = base_url
        .rsplit_once('/')
        .map(|(prefix, _)| prefix)
        .unwrap_or(base_url);
    format!("{}/{}", prefix, url)
}

fn decode_html_attr(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn dedup_urls(urls: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for url in urls {
        if !url.trim().is_empty() && !deduped.iter().any(|existing| existing == &url) {
            deduped.push(url);
        }
    }
    deduped
}

fn pdf_filename(parent: &ApiItem, tag: &str) -> String {
    let title = parent.title();
    let base = if title == "(untitled)" || title.trim().is_empty() {
        tag
    } else {
        title.as_str()
    };
    let sanitized = sanitize_filename(base);
    if sanitized.is_empty() {
        format!("{}.pdf", tag)
    } else {
        format!("{}.pdf", sanitized)
    }
}

fn sanitize_filename(value: &str) -> String {
    let mut out = String::new();
    let mut previous_was_separator = false;
    for ch in value.chars() {
        let replacement = if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            Some(ch)
        } else if ch.is_whitespace() || matches!(ch, '/' | '\\' | ':' | ';' | ',') {
            Some('_')
        } else {
            None
        };

        let Some(ch) = replacement else {
            continue;
        };
        if ch == '_' {
            if previous_was_separator {
                continue;
            }
            previous_was_separator = true;
        } else {
            previous_was_separator = false;
        }
        out.push(ch);
        if out.len() >= 120 {
            break;
        }
    }
    out.trim_matches(['_', '.']).to_string()
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
    let write_client = ZoteroClient::write_preferred(&config)?;

    let template = read_client
        .item_template(item_type)
        .with_context(|| format!("fetching item template for type '{}'", item_type))?;

    let mut editable = EditableItem::from_data(&template);
    if editable.authors.names.is_empty() {
        editable.authors.names.push(EditableAuthor::default());
    }
    let toml_text = editable
        .to_editor_toml()
        .context("serializing blank item for editing")?;

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

    let item = edited.fill_template(template)?;
    let mut created = write_client.create_item(item)?;

    match assign_citation_tag(&read_client, &write_client, &mut created) {
        Ok(Some(_)) => {}
        Ok(None) => eprintln!(
            "warning: added '{}' (key {}) but no author/date was given, so no AuthorYear tag was assigned; \
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
