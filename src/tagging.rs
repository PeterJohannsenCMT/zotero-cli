use anyhow::{Result, anyhow};
use std::collections::HashSet;

use crate::crossref::CreatorName;
use crate::zotero_api::{ApiItem, ZoteroClient};

/// Transliterate to plain ASCII (so e.g. "Šamaj" -> "Samaj", "Rodríguez" ->
/// "Rodriguez") and lowercase, so every citation key is typeable on any
/// keyboard and consistently cased.
fn clean_name_component(name: &str) -> String {
    deunicode::deunicode(name)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Build the base "authoryear" tag, e.g. "smith2020", from the first author's
/// last name and the publication year, before collision suffixes are applied.
pub fn base_tag(creators: &[CreatorName], year: Option<i64>) -> Result<String> {
    let last = creators
        .first()
        .map(|c| c.last_name.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("no author name available to build an AuthorYear tag"))?;
    let cleaned = clean_name_component(last);
    let year =
        year.ok_or_else(|| anyhow!("no publication year available to build an AuthorYear tag"))?;
    Ok(format!("{}{}", cleaned, year))
}

/// Given a base tag like "Smith2020", find the first unused variant in the
/// library (via a live tag search), trying "Smith2020", then "Smith2020a",
/// "Smith2020b", ...
pub fn unique_tag(client: &ZoteroClient, base: &str) -> Result<String> {
    let existing = client.tags_starting_with(base)?;
    let existing_lower: HashSet<String> = existing.iter().map(|s| s.to_lowercase()).collect();
    unique_tag_local(base, &existing_lower)
        .ok_or_else(|| anyhow!("exhausted a-z suffixes for tag base '{}'", base))
}

/// Same as `unique_tag`, but checks against an already-known set of tags
/// (lowercased) instead of making a network request. Used when backfilling
/// many items at once, where we already hold every tag in the library.
pub fn unique_tag_local(base: &str, existing_lower: &HashSet<String>) -> Option<String> {
    if !existing_lower.contains(&base.to_lowercase()) {
        return Some(base.to_string());
    }
    for c in 'a'..='z' {
        let candidate = format!("{}{}", base, c);
        if !existing_lower.contains(&candidate.to_lowercase()) {
            return Some(candidate);
        }
    }
    None
}

/// Does `tag` look like an AuthorYear citation key, i.e. a name followed by a
/// 4-digit year and an optional single-letter collision suffix (`Smith2020`,
/// `Smith2020a`, `Smith2020b`, ...)?
fn looks_like_author_year(tag: &str) -> bool {
    let chars: Vec<char> = tag.chars().collect();
    let mut end = chars.len();
    if end > 0 && chars[end - 1].is_ascii_lowercase() {
        end -= 1;
    }
    end > 4 && chars[end - 4..end].iter().all(|c| c.is_ascii_digit())
}

/// All AuthorYear-shaped tags on an item (normally there should be at most one).
pub fn author_year_candidates(tags: &[String]) -> Vec<String> {
    tags.iter()
        .filter(|t| looks_like_author_year(t))
        .cloned()
        .collect()
}

/// Pick the AuthorYear-shaped tag out of an item's tag list, if it has one.
/// If there's more than one (e.g. leftover collision-suffix tags from a past
/// bug), prefer the shortest / least-suffixed one.
pub fn find_author_year_tag(tags: &[String]) -> Option<String> {
    let mut candidates = author_year_candidates(tags);
    candidates.sort_by_key(|t| t.len());
    candidates.into_iter().next()
}

/// Is this citation key already in our canonical form: plain ASCII, no
/// uppercase letters? (Older/backfilled keys may have kept accented
/// characters or a capitalized surname; those get migrated to this form.)
pub fn is_clean_citation_key(tag: &str) -> bool {
    tag.chars().all(|c| c.is_ascii() && !c.is_ascii_uppercase())
}

/// Find the first 4 consecutive digits in a Zotero date string (which may be
/// a full ISO date, just a year, or something looser).
fn extract_year(date: &str) -> Option<String> {
    let chars: Vec<char> = date.chars().collect();
    for i in 0..chars.len() {
        if i + 4 <= chars.len() && chars[i..i + 4].iter().all(|c| c.is_ascii_digit()) {
            return Some(chars[i..i + 4].iter().collect());
        }
    }
    None
}

/// Derive an AuthorYear base tag directly from an existing library item's own
/// creators/date fields (as opposed to `base_tag`, which works from freshly
/// fetched CrossRef metadata during `zotero add`).
pub fn derive_base_tag_from_item(item: &ApiItem) -> Option<String> {
    let creators = item.data.get("creators")?.as_array()?;
    let last = creators.iter().find_map(|c| {
        c.get("lastName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                c.get("name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            })
    })?;
    let cleaned = clean_name_component(last);
    if cleaned.is_empty() {
        return None;
    }
    let year = extract_year(item.date())?;
    Some(format!("{}{}", cleaned, year))
}
