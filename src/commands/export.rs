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
    bibitem: bool,
    revtex_journals: bool,
) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let collection_key = cache.effective_collection_key(
        subcollection.as_deref(),
        config.current_collection_key.as_deref(),
    )?;
    let use_filters = collection_key.is_some() || tag.is_some();
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
        let items = cache.top_level_items(collection_key.as_deref(), tag.as_deref());
        if items.is_empty() {
            eprintln!("No items found.");
            return Ok(());
        }
        items.into_iter().cloned().collect()
    };

    let mut first = true;
    for item in &targets {
        let enriched = enrich_arxiv_thesis(item);
        let item = &enriched;
        let citekey = item.citation_key().unwrap_or_else(|| {
            // eprintln!(
            //     "warning: '{}' has no AuthorYear tag; using its raw key as the citekey",
            //     item.key
            // );
            item.key.clone()
        });

        let entry = if bibitem {
            Ok(format_bibitem(item, &citekey))
        } else {
            match read_client.item_bibtex(&item.key) {
                Ok(bibtex) => {
                    let bibtex = add_thesis_bibtex_metadata(&bibtex, item);
                    replace_citekey_and_filter_fields(&bibtex, &citekey, revtex_journals, item)
                }
                Err(err) => {
                    eprintln!("error: {:#} (for '{}')", err, item.key);
                    any_failed = true;
                    continue;
                }
            }
        };

        match entry {
            Ok(entry) if !entry.trim().is_empty() => {
                if !first {
                    println!();
                }
                first = false;
                println!("{}", entry.trim_end());
            }
            Ok(_) => {}
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

const BIBTEX_EXPORT_FIELDS: [&str; 7] = [
    "author", "title", "journal", "volume", "pages", "doi", "year",
];

// arXiv imports are often stored as preprints even when the comments identify
// a dissertation. Enrich only the export copy; never modify the user's library.
fn enrich_arxiv_thesis(item: &ApiItem) -> ApiItem {
    let mut enriched = item.clone();
    if !matches!(item.item_type(), "preprint" | "manuscript" | "report") {
        return enriched;
    }
    let Some(id) = arxiv_id(item) else {
        return enriched;
    };
    let url = format!("https://arxiv.org/abs/{}", id);
    let result = (|| -> Result<String> {
        let response = crate::http::response(
            ureq::get(&url)
                .timeout(std::time::Duration::from_secs(10))
                .call(),
        )?;
        if !crate::http::is_success(response.status()) {
            anyhow::bail!("arXiv returned HTTP {}", response.status());
        }
        Ok(response.into_string()?)
    })();
    match result {
        Ok(html) => {
            apply_arxiv_thesis_metadata(&mut enriched, &html);
        }
        Err(error) => eprintln!("warning: could not check thesis metadata at {url}: {error}"),
    }
    enriched
}

fn apply_arxiv_thesis_metadata(item: &mut ApiItem, html: &str) {
    if let Some((degree, university)) = thesis_from_arxiv_html(html) {
        item.data["itemType"] = serde_json::json!("thesis");
        item.data["thesisType"] = serde_json::json!(degree);
        if let Some(university) = university {
            item.data["university"] = serde_json::json!(university);
        }
    }
}

fn thesis_from_arxiv_html(html: &str) -> Option<(&'static str, Option<String>)> {
    let comments = html.split("<td").skip(1).find_map(|cell| {
        let (attributes, text) = cell.split_once('>')?;
        if !attributes.split_whitespace().any(|word| word == "comments") {
            return None;
        }
        Some(text.split_once("</td>")?.0)
    })?;
    let mut text = String::new();
    let mut in_tag = false;
    for ch in comments.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    let text = text
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ");
    let lower = text.to_ascii_lowercase();
    let degree = if [
        "phd thesis",
        "ph.d. thesis",
        "doctoral thesis",
        "doctoral dissertation",
        "phd dissertation",
        "ph.d. dissertation",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        "Ph.D. thesis"
    } else if [
        "master's thesis",
        "masters thesis",
        "master thesis",
        "msc thesis",
        "m.sc. thesis",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        "Master's thesis"
    } else {
        return None;
    };
    // Only accept an institution explicitly given as a separate comment field.
    // Free-form prose and absent institution names should not be guessed.
    let university = text
        .split([',', ';'])
        .skip(1)
        .map(str::trim)
        .find(|part| {
            let lower = part.to_ascii_lowercase();
            [
                "university",
                "universiteit",
                "université",
                "universität",
                "institute of technology",
            ]
            .iter()
            .any(|marker| lower.contains(marker))
                && !lower.contains(" thesis")
                && !lower.contains(" dissertation")
        })
        .map(str::to_string);
    Some((degree, university))
}

fn add_thesis_bibtex_metadata(entry: &str, item: &ApiItem) -> String {
    if item.item_type() != "thesis" {
        return entry.to_string();
    }
    let Some(open) = entry.find('{') else {
        return entry.to_string();
    };
    let Ok(close) = matching_entry_close(entry, open) else {
        return entry.to_string();
    };
    let Some(comma) = entry[open + 1..close].find(',').map(|i| open + 1 + i) else {
        return entry.to_string();
    };
    let existing = split_bibtex_fields(&entry[comma + 1..close]);
    let mut additions = String::new();
    for (name, value) in [
        ("school", field(item, "university")),
        ("type", field(item, "thesisType")),
        ("url", field(item, "url")),
        ("eprint", arxiv_id(item)),
        ("archiveprefix", arxiv_id(item).map(|_| "arXiv".to_string())),
    ] {
        if let Some(value) = value
            && !existing.iter().any(|f| {
                f.split_once('=')
                    .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            })
        {
            let value = if name == "url" {
                latex_escape_url(&value)
            } else {
                latex_escape(&value)
            };
            additions.push_str(&format!("\n  {name} = {{{value}}},"));
        }
    }
    let original_type = entry[..open].trim().to_ascii_lowercase();
    let entry_type = if matches!(original_type.as_str(), "@misc" | "@unpublished") {
        match field(item, "thesisType").as_deref() {
            Some("Ph.D. thesis") => "@phdthesis",
            Some("Master's thesis") => "@mastersthesis",
            _ => &entry[..open],
        }
    } else {
        &entry[..open]
    };
    let body = entry[open..close].trim_end();
    let separator = if body.ends_with(',') { "" } else { "," };
    format!(
        "{entry_type}{body}{separator}{additions}\n{}",
        &entry[close..]
    )
}

const BIBTEX_THESIS_FIELDS: [&str; 14] = [
    "author",
    "title",
    "school",
    "type",
    "address",
    "month",
    "year",
    "doi",
    "url",
    "urldate",
    "eprint",
    "archiveprefix",
    "primaryclass",
    "eprinttype",
];

/// Replace the citekey in a Zotero-rendered BibTeX entry (`@type{oldkey,
/// ...}`) with `new_key`, and keep only the fields requested for `zotero export`.
fn replace_citekey_and_filter_fields(
    entry: &str,
    new_key: &str,
    revtex_journals: bool,
    item: &ApiItem,
) -> Result<String> {
    let open = entry
        .find('{')
        .ok_or_else(|| anyhow!("unexpected bibtex format (no '{{' found)"))?;
    let comma = entry[open + 1..]
        .find(',')
        .map(|i| i + open + 1)
        .ok_or_else(|| anyhow!("unexpected bibtex format (no ',' found)"))?;
    let close = matching_entry_close(entry, open)?;
    let entry_type = entry[..open]
        .trim()
        .trim_start_matches('@')
        .to_ascii_lowercase();
    let export_fields: &[&str] = if item.item_type() == "thesis"
        || matches!(
            entry_type.as_str(),
            "phdthesis" | "mastersthesis" | "thesis"
        ) {
        &BIBTEX_THESIS_FIELDS
    } else {
        &BIBTEX_EXPORT_FIELDS
    };

    let mut fields = Vec::new();
    for field in split_bibtex_fields(&entry[comma + 1..close]) {
        let Some(eq) = field.find('=') else {
            continue;
        };
        let name = field[..eq].trim().to_ascii_lowercase();
        if export_fields.contains(&name.as_str()) {
            let mut value = field[eq + 1..]
                .trim()
                .trim_end_matches(',')
                .trim()
                .to_string();
            if revtex_journals
                && name == "journal"
                && let Some(macro_name) = revtex_journal_macro(item, &value)
            {
                value = macro_name.to_string();
            }
            fields.push((name, value));
        }
    }

    let mut out = format!("{}{},", &entry[..open + 1], new_key);
    for wanted in export_fields {
        if let Some((_, value)) = fields.iter().find(|(name, _)| name == wanted) {
            out.push_str(&format!("\n  {} = {},", wanted, value));
        }
    }
    out.push_str("\n}");
    Ok(out)
}

const REVTEX_JOURNAL_ALIASES: &[(&str, &[&str])] = &[
    (
        "prl",
        &["prl", "Physical Review Letters", "Phys. Rev. Lett."],
    ),
    ("pr", &["pr", "Physical Review", "Phys. Rev."]),
    ("pra", &["pra", "Physical Review A", "Phys. Rev. A"]),
    ("prb", &["prb", "Physical Review B", "Phys. Rev. B"]),
    ("prc", &["prc", "Physical Review C", "Phys. Rev. C"]),
    ("prd", &["prd", "Physical Review D", "Phys. Rev. D"]),
    ("pre", &["pre", "Physical Review E", "Phys. Rev. E"]),
    (
        "rmp",
        &["rmp", "Reviews of Modern Physics", "Rev. Mod. Phys."],
    ),
    (
        "apl",
        &["apl", "Applied Physics Letters", "Appl. Phys. Lett."],
    ),
    (
        "jap",
        &["jap", "Journal of Applied Physics", "J. Appl. Phys."],
    ),
    (
        "jmp",
        &["jmp", "Journal of Mathematical Physics", "J. Math. Phys."],
    ),
    (
        "{J. Chem. Phys.}",
        &[
            "Journal of Chemical Physics",
            "The Journal of Chemical Physics",
            "J. Chem. Phys.",
        ],
    ),
    ("pf", &["pf", "Physics of Fluids", "Phys. Fluids"]),
    ("pp", &["pp", "Physics of Plasmas", "Phys. Plasmas"]),
    (
        "rsi",
        &[
            "rsi",
            "Review of Scientific Instruments",
            "Rev. Sci. Instrum.",
            "Rev. Sci. Inst.",
        ],
    ),
    ("pl", &["pl", "Physics Letters", "Phys. Lett."]),
    ("pla", &["pla", "Physics Letters A", "Phys. Lett. A"]),
    ("plb", &["plb", "Physics Letters B", "Phys. Lett. B"]),
    ("nat", &["nat", "Nature"]),
    ("science", &["science", "Science"]),
    (
        "{SciPost Phys.}",
        &["SciPost Physics", "Sci Post Physics", "SciPost Phys."],
    ),
    (
        "{SciPost Phys. Core}",
        &[
            "SciPost Physics Core",
            "Sci Post Physics Core",
            "SciPost Phys. Core",
        ],
    ),
];

fn revtex_journal_macro(item: &ApiItem, bibtex_value: &str) -> Option<&'static str> {
    let mut candidates = vec![clean_bibtex_value(bibtex_value)];
    candidates.extend(field(item, "publicationTitle"));
    candidates.extend(field(item, "journalAbbreviation"));

    candidates
        .iter()
        .find_map(|candidate| revtex_journal_macro_for_text(candidate))
}

fn revtex_journal_macro_for_text(text: &str) -> Option<&'static str> {
    let wanted = normalize_journal_name(text);
    if wanted.is_empty() {
        return None;
    }

    REVTEX_JOURNAL_ALIASES
        .iter()
        .find_map(|(macro_name, aliases)| {
            aliases
                .iter()
                .any(|alias| normalize_journal_name(alias) == wanted)
                .then_some(*macro_name)
        })
}

fn clean_bibtex_value(value: &str) -> String {
    let mut text = value.trim();
    loop {
        let stripped = if let Some(inner) = text.strip_prefix('{').and_then(|s| s.strip_suffix('}'))
        {
            Some(inner)
        } else {
            text.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        };
        match stripped {
            Some(inner) => text = inner.trim(),
            None => break,
        }
    }
    text.to_string()
}

fn normalize_journal_name(text: &str) -> String {
    clean_bibtex_value(text)
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn matching_entry_close(entry: &str, open: usize) -> Result<usize> {
    let mut depth = 0usize;
    let mut in_quote = false;
    let mut escaped = false;

    for (idx, ch) in entry[open..].char_indices() {
        let absolute = open + idx;
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' && depth <= 1 {
            in_quote = !in_quote;
        }
        if in_quote {
            continue;
        }
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(absolute);
                }
            }
            _ => {}
        }
    }

    Err(anyhow!("unexpected bibtex format (entry was not closed)"))
}

fn split_bibtex_fields(body: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0usize;
    let mut brace_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut in_quote = false;
    let mut escaped = false;

    for (idx, ch) in body.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' && brace_depth == 0 {
            in_quote = !in_quote;
        }
        if in_quote {
            continue;
        }
        match ch {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            ',' if brace_depth == 0 && paren_depth == 0 => {
                let field = body[start..idx].trim();
                if !field.is_empty() {
                    fields.push(field);
                }
                start = idx + 1;
            }
            _ => {}
        }
    }

    let field = body[start..].trim();
    if !field.is_empty() {
        fields.push(field);
    }
    fields
}

fn format_bibitem(item: &ApiItem, citekey: &str) -> String {
    format!(
        "\\bibitem{{{}}} {}",
        latex_escape_citekey(citekey),
        aps_reference(item)
    )
}

fn aps_reference(item: &ApiItem) -> String {
    if !matches!(item.item_type(), "journalArticle" | "thesis")
        && let Some(arxiv) = arxiv_id(item)
    {
        return finish_reference(join_nonempty(vec![
            creators(item),
            Some(format!("arXiv:{}", arxiv)),
        ]));
    }

    match item.item_type() {
        "journalArticle" => aps_journal_article(item),
        "book" => aps_book(item),
        "bookSection" | "conferencePaper" => aps_chapter_or_proceeding(item),
        "thesis" => aps_thesis(item),
        "report" => aps_report(item),
        _ => aps_generic(item),
    }
}

fn aps_journal_article(item: &ApiItem) -> String {
    let mut chunks = Vec::new();
    chunks.extend(creators(item));

    if let Some(title) = field(item, "title") {
        let title = strip_trailing_periods(&title);
        if !title.is_empty() {
            chunks.push(latex_escape(&title));
        }
    }

    if let Some(citation) = aps_journal_citation(item) {
        chunks.push(hyperlink_if_available(item, &citation));
    } else if let Some(arxiv) = arxiv_id(item) {
        chunks.push(format!("arXiv:{}", latex_escape(&arxiv)));
    }

    finish_reference(chunks.join(", "))
}

fn aps_journal_citation(item: &ApiItem) -> Option<String> {
    let mut citation = aps_journal_name(item)?;

    if let Some(volume) = field(item, "volume") {
        citation.push_str(&format!(" \\textbf{{{}}}", latex_escape(&volume)));
    }
    if let Some(locator) = field(item, "pages").or_else(|| field(item, "articleNumber")) {
        citation.push_str(&format!(", {}", latex_escape(&locator)));
    }
    if let Some(year) = year(item) {
        citation.push_str(&format!(" ({})", latex_escape(&year)));
    }

    Some(citation)
}

fn aps_journal_name(item: &ApiItem) -> Option<String> {
    let journal_abbreviation = field(item, "journalAbbreviation");
    let publication_title = field(item, "publicationTitle");

    journal_abbreviation
        .iter()
        .chain(publication_title.iter())
        .find_map(|journal| aps_journal_abbreviation_for_text(journal))
        .map(str::to_string)
        .or(journal_abbreviation)
        .or(publication_title)
        .map(|journal| latex_escape(&journal))
}

fn aps_book(item: &ApiItem) -> String {
    let mut chunks = Vec::new();
    chunks.extend(creators(item));
    if let Some(title) = field(item, "title") {
        chunks.push(format!("\\emph{{{}}}", latex_escape(&title)));
    }
    if let Some(parenthetical) = publisher_parenthetical(item) {
        chunks.push(parenthetical);
    }
    if let Some(edition) = field(item, "edition") {
        chunks.push(format!("{} ed.", latex_escape(&edition)));
    }
    if let Some(volume) = field(item, "volume") {
        chunks.push(format!("Vol. {}", latex_escape(&volume)));
    }
    if let Some(pages) = field(item, "pages") {
        chunks.push(format!("p. {}", latex_escape(&pages)));
    }
    finish_reference(chunks.join(", "))
}

fn aps_chapter_or_proceeding(item: &ApiItem) -> String {
    let mut chunks = Vec::new();
    chunks.extend(creators(item));
    if let Some(title) = field(item, "title") {
        chunks.push(format!("\\emph{{{}}}", latex_escape(&title)));
    }
    if let Some(container) = field(item, "bookTitle").or_else(|| field(item, "conferenceName")) {
        chunks.push(format!("in \\emph{{{}}}", latex_escape(&container)));
    }
    if let Some(parenthetical) = publisher_parenthetical(item) {
        chunks.push(parenthetical);
    }
    if let Some(volume) = field(item, "volume") {
        chunks.push(format!("Vol. {}", latex_escape(&volume)));
    }
    if let Some(pages) = field(item, "pages") {
        chunks.push(format!("p. {}", latex_escape(&pages)));
    }
    finish_reference(chunks.join(", "))
}

fn aps_thesis(item: &ApiItem) -> String {
    let mut chunks = Vec::new();
    chunks.extend(creators(item));
    if let Some(title) = field(item, "title") {
        chunks.push(format!(
            "\\emph{{{}}}",
            latex_escape(&strip_trailing_periods(&title))
        ));
    }
    let thesis_type = field(item, "thesisType").unwrap_or_else(|| "Thesis".to_string());
    if let Some(university) = field(item, "university") {
        chunks.push(format!(
            "{}, {}",
            latex_escape(&thesis_type),
            latex_escape(&university)
        ));
    } else {
        chunks.push(latex_escape(&thesis_type));
    }
    if let Some(year) = year(item) {
        chunks.push(latex_escape(&year));
    }
    if let Some(arxiv) = arxiv_id(item) {
        chunks.push(format!(
            "\\href{{https://arxiv.org/abs/{}}}{{arXiv:{}}}",
            latex_escape_url(&arxiv),
            latex_escape(&arxiv)
        ));
    }
    finish_reference(chunks.join(", "))
}

fn aps_report(item: &ApiItem) -> String {
    let mut chunks = Vec::new();
    chunks.extend(creators(item));
    if let Some(institution) = field(item, "institution") {
        chunks.push(latex_escape(&institution));
    }
    if let Some(report_number) = field(item, "reportNumber") {
        chunks.push(latex_escape(&report_number));
    } else if let Some(title) = field(item, "title") {
        chunks.push(format!("\\emph{{{}}}", latex_escape(&title)));
    }
    if let Some(year) = year(item) {
        chunks.push(latex_escape(&year));
    }
    finish_reference(chunks.join(", "))
}

fn aps_generic(item: &ApiItem) -> String {
    let mut chunks = Vec::new();
    chunks.extend(creators(item));
    if let Some(title) = field(item, "title") {
        chunks.push(format!("\\emph{{{}}}", latex_escape(&title)));
    }
    if let Some(container) = field(item, "publicationTitle").or_else(|| field(item, "websiteTitle"))
    {
        chunks.push(latex_escape(&container));
    }
    if let Some(year) = year(item) {
        chunks.push(latex_escape(&year));
    }
    if let Some(url) = field(item, "url") {
        chunks.push(format!("\\url{{{}}}", latex_escape_url(&url)));
    }
    finish_reference(chunks.join(", "))
}

fn publisher_parenthetical(item: &ApiItem) -> Option<String> {
    let publisher = field(item, "publisher")?;
    let mut parts = vec![latex_escape(&publisher)];
    if let Some(place) = field(item, "place") {
        parts.push(latex_escape(&place));
    }
    if let Some(year) = year(item) {
        parts.push(latex_escape(&year));
    }
    Some(format!("({})", parts.join(", ")))
}

fn hyperlink_if_available(item: &ApiItem, text: &str) -> String {
    reference_url(item)
        .map(|url| format!("\\href{{{}}}{{{}}}", latex_escape_url(&url), text))
        .unwrap_or_else(|| text.to_string())
}

fn reference_url(item: &ApiItem) -> Option<String> {
    field(item, "DOI")
        .or_else(|| field(item, "doi"))
        .map(|doi| doi_url(&doi))
        .or_else(|| field(item, "url"))
}

fn doi_url(doi: &str) -> String {
    let doi = doi.trim();
    let lower = doi.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        doi.to_string()
    } else if lower.starts_with("doi:") {
        format!("https://doi.org/{}", doi[4..].trim())
    } else {
        format!("https://doi.org/{}", doi)
    }
}

fn aps_journal_abbreviation_for_text(text: &str) -> Option<&'static str> {
    match normalize_journal_name(text).as_str() {
        "prl" | "physicalreviewletters" | "physrevlett" => Some("Phys. Rev. Lett."),
        "pr" | "physicalreview" | "physrev" => Some("Phys. Rev."),
        "pra" | "physicalreviewa" | "physreva" => Some("Phys. Rev. A"),
        "prb" | "physicalreviewb" | "physrevb" => Some("Phys. Rev. B"),
        "prc" | "physicalreviewc" | "physrevc" => Some("Phys. Rev. C"),
        "prd" | "physicalreviewd" | "physrevd" => Some("Phys. Rev. D"),
        "pre" | "physicalreviewe" | "physreve" => Some("Phys. Rev. E"),
        "prab" | "physicalreviewacceleratorsandbeams" | "physrevaccelbeams" => {
            Some("Phys. Rev. Accel. Beams")
        }
        "prapplied" | "physicalreviewapplied" | "physrevapplied" | "physrevappl" => {
            Some("Phys. Rev. Applied")
        }
        "prfluids" | "physicalreviewfluids" | "physrevfluids" => Some("Phys. Rev. Fluids"),
        "prmaterials" | "physicalreviewmaterials" | "physrevmaterials" => {
            Some("Phys. Rev. Materials")
        }
        "prper" | "physicalreviewphysicseducationresearch" | "physrevphyseducres" => {
            Some("Phys. Rev. Phys. Educ. Res.")
        }
        "prresearch" | "physicalreviewresearch" | "physrevresearch" | "physrevres" => {
            Some("Phys. Rev. Research")
        }
        "prstper"
        | "physicalreviewspecialtopicsphysicseducationresearch"
        | "physrevstphyseducres" => Some("Phys. Rev. ST Phys. Educ. Res."),
        "prstab" | "physicalreviewspecialtopicsacceleratorsandbeams" | "physrevstaccelbeams" => {
            Some("Phys. Rev. ST Accel. Beams")
        }
        "prx" | "physicalreviewx" | "physrevx" => Some("Phys. Rev. X"),
        "prxenergy" | "physicalreviewxenergy" | "physrevxenergy" => Some("PRX Energy"),
        "prxlife" | "physicalreviewxlife" | "physrevxlife" => Some("PRX Life"),
        "prxquantum" | "physicalreviewxquantum" => Some("PRX Quantum"),
        "rmp" | "reviewsofmodernphysics" | "revmodphys" => Some("Rev. Mod. Phys."),
        "apl" | "appliedphysicsletters" | "applphyslett" => Some("Appl. Phys. Lett."),
        "jap" | "journalofappliedphysics" | "japplphys" => Some("J. Appl. Phys."),
        "jmp" | "journalofmathematicalphysics" | "jmathphys" => Some("J. Math. Phys."),
        "journalofchemicalphysics" | "thejournalofchemicalphysics" | "jchemphys" => {
            Some("J. Chem. Phys.")
        }
        "pf" | "physicsoffluids" | "physfluids" => Some("Phys. Fluids"),
        "pp" | "physicsofplasmas" | "physplasmas" => Some("Phys. Plasmas"),
        "rsi" | "reviewofscientificinstruments" | "revsciinstrum" | "revsciinst" => {
            Some("Rev. Sci. Instrum.")
        }
        "pl" | "physicsletters" | "physlett" => Some("Phys. Lett."),
        "pla" | "physicslettersa" | "physletta" => Some("Phys. Lett. A"),
        "plb" | "physicslettersb" | "physlettb" => Some("Phys. Lett. B"),
        "scipostphysics" | "scipostphys" => Some("SciPost Phys."),
        "scipostphysicscore" | "scipostphyscore" => Some("SciPost Phys. Core"),
        _ => None,
    }
}

fn strip_trailing_periods(input: &str) -> String {
    let mut text = input.trim_end();
    while text.ends_with('.') {
        text = text[..text.len() - 1].trim_end();
    }
    text.to_string()
}

fn field(item: &ApiItem, name: &str) -> Option<String> {
    item.data
        .get(name)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn year(item: &ApiItem) -> Option<String> {
    field(item, "date").and_then(|date| {
        let year: String = date
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take(4)
            .collect();
        (year.len() == 4).then_some(year)
    })
}

fn creators(item: &ApiItem) -> Option<String> {
    let creators = item.data.get("creators")?.as_array()?;
    let names: Vec<String> = creators
        .iter()
        .filter(|creator| {
            creator
                .get("creatorType")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|ty| ty == "author")
        })
        .filter_map(|creator| {
            if let Some(name) = creator.get("name").and_then(serde_json::Value::as_str) {
                return Some(latex_escape(name.trim()));
            }
            let first = creator
                .get("firstName")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            let last = creator
                .get("lastName")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            match (first.is_empty(), last.is_empty()) {
                (true, true) => None,
                (true, false) => Some(latex_surname(last)),
                (false, true) => Some(initials(first)),
                (false, false) => Some(format!("{}~{}", initials(first), latex_surname(last))),
            }
        })
        .collect();

    match names.len() {
        0 => None,
        1 => Some(names[0].clone()),
        2 => Some(format!("{} and {}", names[0], names[1])),
        _ => Some(format!(
            "{}, and {}",
            names[..names.len() - 1].join(", "),
            names[names.len() - 1]
        )),
    }
    .or_else(|| {
        let summary = item.creator_summary();
        (!summary.is_empty()).then(|| latex_escape(&summary))
    })
}

fn latex_surname(last_name: &str) -> String {
    last_name
        .split_whitespace()
        .map(latex_escape)
        .collect::<Vec<_>>()
        .join("~")
}

fn initials(first_name: &str) -> String {
    first_name
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter_map(|part| part.chars().find(|c| c.is_alphabetic()))
        .map(|ch| format!("{}.", ch.to_uppercase()))
        .collect::<Vec<String>>()
        .join("~")
}

pub(crate) fn arxiv_id(item: &ApiItem) -> Option<String> {
    let archive = field(item, "archive").unwrap_or_default();
    let archive_location = field(item, "archiveLocation");
    if archive.eq_ignore_ascii_case("arXiv") {
        return archive_location.map(|id| id.trim_start_matches("arXiv:").to_string());
    }

    for value in [archive_location, field(item, "extra"), field(item, "url")] {
        if let Some(value) = value
            && let Some(id) = extract_arxiv_id(&value)
        {
            return Some(id);
        }
    }
    None
}

fn extract_arxiv_id(input: &str) -> Option<String> {
    let marker = input
        .find("arXiv:")
        .map(|i| i + "arXiv:".len())
        .or_else(|| {
            input
                .find("arxiv.org/abs/")
                .map(|i| i + "arxiv.org/abs/".len())
        })?;
    let id: String = input[marker..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '/'))
        .collect();
    (!id.is_empty()).then_some(id)
}

fn join_nonempty(parts: Vec<Option<String>>) -> String {
    parts
        .into_iter()
        .flatten()
        .collect::<Vec<String>>()
        .join(", ")
}

fn finish_reference(mut reference: String) -> String {
    while reference.ends_with('.') {
        reference.pop();
    }
    format!("{}.", reference)
}

fn latex_escape(input: &str) -> String {
    input.chars().fold(String::new(), |mut out, ch| {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '$' => out.push_str("\\$"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            _ => out.push(ch),
        }
        out
    })
}

fn latex_escape_url(input: &str) -> String {
    input.replace('%', "\\%").replace('\\', "\\textbackslash{}")
}

fn latex_escape_citekey(input: &str) -> String {
    input.replace('\\', "").replace(['{', '}', ','], "")
}

#[cfg(test)]
mod tests {
    use super::{
        add_thesis_bibtex_metadata, apply_arxiv_thesis_metadata, format_bibitem, latex_escape,
        replace_citekey_and_filter_fields, thesis_from_arxiv_html,
    };
    use crate::zotero_api::ApiItem;

    #[test]
    fn exports_an_arxiv_thesis_stored_as_a_preprint_in_both_formats() {
        let mut item = ApiItem {
            key: "THESIS".to_string(),
            version: 1,
            data: serde_json::json!({
                "itemType": "preprint", "title": "Example dissertation", "date": "2020",
                "extra": "arXiv:2001.01234", "url": "https://arxiv.org/abs/2001.01234",
                "creators": [{"creatorType": "author", "firstName": "Jane", "lastName": "Doe"}]
            }),
        };
        apply_arxiv_thesis_metadata(
            &mut item,
            "<td class=\"tablecell comments mathjax\">PhD Thesis, Example University, 2020</td>",
        );
        assert_eq!(
            format_bibitem(&item, "doe2020"),
            "\\bibitem{doe2020} J.~Doe, \\emph{Example dissertation}, Ph.D. thesis, Example University, 2020, \\href{https://arxiv.org/abs/2001.01234}{arXiv:2001.01234}."
        );
        let bibtex = add_thesis_bibtex_metadata(
            "@misc{old, author = {Doe, Jane}, title = {Example dissertation}, year = {2020},}",
            &item,
        );
        let exported = replace_citekey_and_filter_fields(&bibtex, "doe2020", false, &item).unwrap();
        assert!(exported.starts_with("@phdthesis{doe2020,"));
        assert!(exported.contains("school = {Example University}"));
        assert!(exported.contains("type = {Ph.D. thesis}"));
    }

    #[test]
    fn recognizes_explicit_arxiv_thesis_comments() {
        for (comments, degree, university) in [
            (
                "PhD Thesis (174 pages), Ghent University, 2018, ISBN 978-9-4619761-4-7",
                "Ph.D. thesis",
                Some("Ghent University"),
            ),
            (
                "Master's thesis; Example Institute of Technology; 2022",
                "Master's thesis",
                Some("Example Institute of Technology"),
            ),
            (
                "Doctoral dissertation, Université A &amp; B, 2021",
                "Ph.D. thesis",
                Some("Université A & B"),
            ),
            ("Ph.D. thesis, 2023", "Ph.D. thesis", None),
        ] {
            let html = format!("<td class=\"tablecell comments mathjax\">{comments}</td>");
            assert_eq!(
                thesis_from_arxiv_html(&html),
                Some((degree, university.map(str::to_string)))
            );
        }
        assert_eq!(
            thesis_from_arxiv_html(
                "<p>This thesis presents...</p><td class=\"tablecell comments mathjax\">Published article, 12 pages</td>"
            ),
            None
        );
        assert_eq!(
            thesis_from_arxiv_html(
                "<td class=\"tablecell comments mathjax\">Discusses university research</td>"
            ),
            None
        );
    }

    #[test]
    fn enriches_misc_bibtex_without_overwriting_existing_fields() {
        let item = ApiItem {
            key: "THESIS".to_string(),
            version: 1,
            data: serde_json::json!({
                "itemType": "thesis", "thesisType": "Ph.D. thesis",
                "university": "A & B University", "extra": "arXiv:2401.01234",
                "url": "https://arxiv.org/abs/2401.01234"
            }),
        };
        let entry =
            "@misc{old, title = {Original {Title}}, type = {Custom doctorate}, year = {2024}}";
        let enriched = add_thesis_bibtex_metadata(entry, &item);
        let filtered = replace_citekey_and_filter_fields(&enriched, "new", false, &item).unwrap();
        assert!(filtered.starts_with("@phdthesis{new,"));
        assert!(filtered.contains("title = {Original {Title}}"));
        assert!(filtered.contains("school = {A \\& B University}"));
        assert!(filtered.contains("type = {Custom doctorate}"));
        assert!(filtered.contains("eprint = {2401.01234}"));
        assert!(filtered.contains("archiveprefix = {arXiv}"));
    }

    #[test]
    fn preserves_bibtex_thesis_fields_and_entry_types() {
        for (entry_type, item_type) in [
            ("phdthesis", ""),
            ("mastersthesis", ""),
            ("thesis", ""),
            ("misc", "thesis"),
        ] {
            let item = ApiItem {
                key: "THESIS".to_string(),
                version: 1,
                data: serde_json::json!({"itemType": item_type}),
            };
            let entry = format!(
                "@{entry_type}{{old,\n  title = {{A {{nested}} title}},\n  school = {{University, Department}},\n  type = {{Master's thesis}},\n  address = {{City}},\n  year = {{2020}},\n  url = {{https://example.test/thesis}},\n  eprint = {{2001.01234}},\n  archivePrefix = {{arXiv}},\n  abstract = {{Omit me}},\n}}"
            );
            assert_eq!(
                replace_citekey_and_filter_fields(&entry, "new", true, &item).unwrap(),
                format!(
                    "@{entry_type}{{new,\n  title = {{A {{nested}} title}},\n  school = {{University, Department}},\n  type = {{Master's thesis}},\n  address = {{City}},\n  year = {{2020}},\n  url = {{https://example.test/thesis}},\n  eprint = {{2001.01234}},\n  archiveprefix = {{arXiv}},\n}}"
                )
            );
        }
    }

    #[test]
    fn replaces_bibtex_citekey_and_keeps_only_requested_fields() {
        let item = ApiItem {
            key: "ABCD1234".to_string(),
            version: 1,
            data: serde_json::json!({}),
        };
        let entry = "@article{old,\n  title = {Title},\n  abstract = {Nope},\n  year = {2020},\n  doi = {10.1000/test},\n}";
        assert_eq!(
            replace_citekey_and_filter_fields(entry, "Smith2020", false, &item).unwrap(),
            "@article{Smith2020,\n  title = {Title},\n  doi = {10.1000/test},\n  year = {2020},\n}"
        );
    }

    #[test]
    fn can_rewrite_physical_review_journal_to_revtex_macro() {
        let item = ApiItem {
            key: "ABCD1234".to_string(),
            version: 1,
            data: serde_json::json!({
                "publicationTitle": "Physical Review B",
                "journalAbbreviation": "Phys. Rev. B"
            }),
        };
        let entry = "@article{old,\n  title = {Title},\n  journal = {Physical Review B},\n  year = {2020},\n}";
        assert_eq!(
            replace_citekey_and_filter_fields(entry, "Smith2020", true, &item).unwrap(),
            "@article{Smith2020,\n  title = {Title},\n  journal = prb,\n  year = {2020},\n}"
        );
    }

    #[test]
    fn can_use_zotero_metadata_to_pick_revtex_journal_macro() {
        let item = ApiItem {
            key: "ABCD1234".to_string(),
            version: 1,
            data: serde_json::json!({
                "publicationTitle": "Physical Review Letters"
            }),
        };
        let entry = "@article{old,\n  journal = {PhysRevLett},\n  year = {2020},\n}";
        assert_eq!(
            replace_citekey_and_filter_fields(entry, "Smith2020", true, &item).unwrap(),
            "@article{Smith2020,\n  journal = prl,\n  year = {2020},\n}"
        );
    }

    #[test]
    fn abbreviates_scipost_journal_names() {
        let item = ApiItem {
            key: "ABCD1234".to_string(),
            version: 1,
            data: serde_json::json!({}),
        };
        let entry = "@article{old,\n  journal = {SciPost Physics},\n  year = {2020},\n}";
        assert_eq!(
            replace_citekey_and_filter_fields(entry, "Smith2020", true, &item).unwrap(),
            "@article{Smith2020,\n  journal = {SciPost Phys.},\n  year = {2020},\n}"
        );
    }

    #[test]
    fn abbreviates_journal_of_chemical_physics_without_using_revtex_jcp_macro() {
        let item = ApiItem {
            key: "ABCD1234".to_string(),
            version: 1,
            data: serde_json::json!({}),
        };
        let entry =
            "@article{old,\n  journal = {Journal of Chemical Physics},\n  year = {2020},\n}";
        assert_eq!(
            replace_citekey_and_filter_fields(entry, "Smith2020", true, &item).unwrap(),
            "@article{Smith2020,\n  journal = {J. Chem. Phys.},\n  year = {2020},\n}"
        );
    }

    #[test]
    fn formats_aps_journal_bibitem_from_item_data() {
        let item = ApiItem {
            key: "ABCD1234".to_string(),
            version: 1,
            data: serde_json::json!({
                "itemType": "journalArticle",
                "title": "A & B",
                "date": "2020-05-01",
                "publicationTitle": "Journal_name",
                "journalAbbreviation": "J. Name",
                "volume": "12",
                "issue": "3",
                "pages": "4-9",
                "DOI": "10.1000/test_1",
                "creators": [
                    {"creatorType": "author", "firstName": "Ada Marie", "lastName": "Lovelace"},
                    {"creatorType": "author", "firstName": "Alan", "lastName": "Turing"}
                ]
            }),
        };

        assert_eq!(
            format_bibitem(&item, "Lovelace2020"),
            "\\bibitem{Lovelace2020} A.~M.~Lovelace and A.~Turing, A \\& B, \\href{https://doi.org/10.1000/test_1}{J. Name \\textbf{12}, 4-9 (2020)}."
        );
    }

    #[test]
    fn abbreviates_and_hyperlinks_physical_review_bibitem_journal_block() {
        let item = ApiItem {
            key: "PRL12345".to_string(),
            version: 1,
            data: serde_json::json!({
                "itemType": "journalArticle",
                "title": "A discovery.",
                "date": "2024",
                "publicationTitle": "Physical Review Letters",
                "volume": "132",
                "pages": "123456",
                "DOI": "10.1103/PhysRevLett.132.123456",
                "creators": [
                    {"creatorType": "author", "firstName": "Jane", "lastName": "Doe"}
                ]
            }),
        };

        assert_eq!(
            format_bibitem(&item, "Doe2024"),
            "\\bibitem{Doe2024} J.~Doe, A discovery, \\href{https://doi.org/10.1103/PhysRevLett.132.123456}{Phys. Rev. Lett. \\textbf{132}, 123456 (2024)}."
        );
    }

    #[test]
    fn abbreviates_current_aps_bibitem_journal_names() {
        let item = ApiItem {
            key: "PRX12345".to_string(),
            version: 1,
            data: serde_json::json!({
                "itemType": "journalArticle",
                "title": "A result",
                "date": "2024",
                "journalAbbreviation": "Physical Review X",
                "publicationTitle": "Physical Review X",
                "volume": "14",
                "pages": "011001",
                "creators": [
                    {"creatorType": "author", "firstName": "Jane", "lastName": "Doe"}
                ]
            }),
        };

        assert_eq!(
            format_bibitem(&item, "Doe2024"),
            "\\bibitem{Doe2024} J.~Doe, A result, Phys. Rev. X \\textbf{14}, 011001 (2024)."
        );
    }

    #[test]
    fn bibitem_surnames_use_nonbreaking_spaces() {
        let item = ApiItem {
            key: "NAMES123".into(),
            version: 1,
            data: serde_json::json!({
                "creators": [
                    {"creatorType": "author", "firstName": "Ludwig", "lastName": " van  Beethoven "},
                    {"creatorType": "author", "lastName": "de\tla\u{00a0}Cruz & Co"}
                ]
            }),
        };
        assert_eq!(
            super::creators(&item).unwrap(),
            "L.~van~Beethoven and de~la~Cruz~\\&~Co"
        );
        assert_eq!(item.data["creators"][0]["lastName"], " van  Beethoven ");
    }

    #[test]
    fn formats_all_bibitem_authors_without_et_al() {
        let item = ApiItem {
            key: "MANY1234".to_string(),
            version: 1,
            data: serde_json::json!({
                "itemType": "journalArticle",
                "title": "Many authors",
                "date": "2024",
                "publicationTitle": "Physical Review Research",
                "volume": "6",
                "pages": "013001",
                "creators": [
                    {"creatorType": "author", "firstName": "Author", "lastName": "One"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Two"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Three"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Four"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Five"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Six"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Seven"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Eight"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Nine"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Ten"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Eleven"},
                    {"creatorType": "author", "firstName": "Author", "lastName": "Twelve"}
                ]
            }),
        };

        assert_eq!(
            format_bibitem(&item, "One2024"),
            "\\bibitem{One2024} A.~One, A.~Two, A.~Three, A.~Four, A.~Five, A.~Six, A.~Seven, A.~Eight, A.~Nine, A.~Ten, A.~Eleven, and A.~Twelve, Many authors, Phys. Rev. Research \\textbf{6}, 013001 (2024)."
        );
    }

    #[test]
    fn formats_aps_book_bibitem() {
        let item = ApiItem {
            key: "BOOK1234".to_string(),
            version: 1,
            data: serde_json::json!({
                "itemType": "book",
                "title": "Quantum Mechanics",
                "date": "1999",
                "publisher": "Academic Press",
                "place": "New York",
                "creators": [
                    {"creatorType": "author", "firstName": "Paul", "lastName": "Dirac"}
                ]
            }),
        };

        assert_eq!(
            format_bibitem(&item, "Dirac1999"),
            "\\bibitem{Dirac1999} P.~Dirac, \\emph{Quantum Mechanics}, (Academic Press, New York, 1999)."
        );
    }

    #[test]
    fn preserves_thesis_metadata_with_arxiv_identifier() {
        let item = ApiItem {
            key: "THESIS".to_string(),
            version: 1,
            data: serde_json::json!({
                "itemType": "thesis",
                "title": "Richardson-Gaudin models and broken integrability",
                "thesisType": "Ph.D. thesis",
                "university": "Ghent University",
                "date": "2018",
                "url": "https://arxiv.org/abs/1809.04447",
                "creators": [{"creatorType": "author", "firstName": "Pieter W.", "lastName": "Claeys"}]
            }),
        };
        assert_eq!(
            format_bibitem(&item, "claeys2018"),
            "\\bibitem{claeys2018} P.~W.~Claeys, \\emph{Richardson-Gaudin models and broken integrability}, Ph.D. thesis, Ghent University, 2018, \\href{https://arxiv.org/abs/1809.04447}{arXiv:1809.04447}."
        );
    }

    #[test]
    fn does_not_assume_a_thesis_is_a_doctorate() {
        let item = ApiItem {
            key: "THESIS".to_string(),
            version: 1,
            data: serde_json::json!({"itemType": "thesis", "date": "2020"}),
        };
        assert_eq!(
            format_bibitem(&item, "thesis2020"),
            "\\bibitem{thesis2020} Thesis, 2020."
        );
    }

    #[test]
    fn formats_arxiv_bibitem() {
        let item = ApiItem {
            key: "ARXIV123".to_string(),
            version: 1,
            data: serde_json::json!({
                "archive": "arXiv",
                "archiveLocation": "2401.01234",
                "creators": [
                    {"creatorType": "author", "firstName": "Jane", "lastName": "Smith"}
                ]
            }),
        };

        assert_eq!(
            format_bibitem(&item, "Smith2024"),
            "\\bibitem{Smith2024} J.~Smith, arXiv:2401.01234."
        );
    }

    #[test]
    fn escapes_latex_specials() {
        assert_eq!(
            latex_escape(r"\ & % $ # _ { } ~ ^"),
            r"\textbackslash{} \& \% \$ \# \_ \{ \} \textasciitilde{} \textasciicircum{}"
        );
    }
}
