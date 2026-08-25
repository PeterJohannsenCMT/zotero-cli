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

    let mut fields = Vec::new();
    for field in split_bibtex_fields(&entry[comma + 1..close]) {
        let Some(eq) = field.find('=') else {
            continue;
        };
        let name = field[..eq].trim().to_ascii_lowercase();
        if BIBTEX_EXPORT_FIELDS.contains(&name.as_str()) {
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
    for wanted in BIBTEX_EXPORT_FIELDS {
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
    if let Some(arxiv) = arxiv_id(item) {
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

    if let Some(journal) =
        field(item, "journalAbbreviation").or_else(|| field(item, "publicationTitle"))
    {
        chunks.push(latex_escape(&journal));
    } else if let Some(title) = field(item, "title") {
        chunks.push(format!("\\emph{{{}}}", latex_escape(&title)));
    }

    if let Some(volume) = field(item, "volume") {
        chunks.push(format!("\\textbf{{{}}}", latex_escape(&volume)));
    }
    if let Some(locator) = field(item, "pages").or_else(|| field(item, "articleNumber")) {
        chunks.push(latex_escape(&locator));
    }
    if let Some(year) = year(item) {
        if let Some(last) = chunks.last_mut() {
            last.push_str(&format!(" ({})", latex_escape(&year)));
        } else {
            chunks.push(format!("({})", latex_escape(&year)));
        }
    }

    finish_reference(chunks.join(", "))
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
    let thesis_type = field(item, "thesisType").unwrap_or_else(|| "Ph.D. thesis".to_string());
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
                (true, false) => Some(latex_escape(last)),
                (false, true) => Some(initials(first)),
                (false, false) => Some(format!("{} {}", initials(first), latex_escape(last))),
            }
        })
        .take(11)
        .collect();

    match names.len() {
        0 => None,
        1 => Some(names[0].clone()),
        2 => Some(format!("{} and {}", names[0], names[1])),
        11 => Some(format!("{}, et al.", names[..10].join(", "))),
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

fn initials(first_name: &str) -> String {
    first_name
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter_map(|part| part.chars().find(|c| c.is_alphabetic()))
        .map(|ch| format!("{}.", ch.to_uppercase()))
        .collect::<Vec<String>>()
        .join(" ")
}

fn arxiv_id(item: &ApiItem) -> Option<String> {
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
    use super::{format_bibitem, latex_escape, replace_citekey_and_filter_fields};
    use crate::zotero_api::ApiItem;

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
            "\\bibitem{Lovelace2020} A. M. Lovelace and A. Turing, J. Name, \\textbf{12}, 4-9 (2020)."
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
            "\\bibitem{Dirac1999} P. Dirac, \\emph{Quantum Mechanics}, (Academic Press, New York, 1999)."
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
            "\\bibitem{Smith2024} J. Smith, arXiv:2401.01234."
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
