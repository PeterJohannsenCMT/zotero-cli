use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditableAuthor {
    #[serde(default)]
    pub first: String,
    #[serde(default)]
    pub last: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditableAuthors {
    #[serde(default)]
    pub names: Vec<EditableAuthor>,
}

/// Only fields used by BibTeX or bibitem export, plus the requested abstract.
/// Hidden metadata is retained in the original item, never in the editor form.
#[derive(Debug, Serialize, Deserialize)]
pub struct EditableItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub authors: EditableAuthors,
    #[serde(flatten)]
    pub fields: BTreeMap<String, String>,
}

fn item_fields(item_type: &str) -> &'static [(&'static str, &'static str)] {
    match item_type {
        "journalArticle" => &[
            ("journal", "publicationTitle"),
            ("journal_abbreviation", "journalAbbreviation"),
            ("date", "date"),
            ("volume", "volume"),
            ("pages", "pages"),
            ("doi", "DOI"),
            ("url", "url"),
            ("abstract", "abstractNote"),
        ],
        "thesis" => &[
            ("thesis_type", "thesisType"),
            ("university", "university"),
            ("date", "date"),
            ("place", "place"),
            ("doi", "DOI"),
            ("url", "url"),
            ("arxiv_id", "extra"),
            ("abstract", "abstractNote"),
        ],
        "book" => &[
            ("publisher", "publisher"),
            ("place", "place"),
            ("date", "date"),
            ("edition", "edition"),
            ("volume", "volume"),
            ("pages", "pages"),
            ("doi", "DOI"),
            ("abstract", "abstractNote"),
        ],
        "bookSection" | "conferencePaper" => &[
            ("book_title", "bookTitle"),
            ("conference", "conferenceName"),
            ("publisher", "publisher"),
            ("place", "place"),
            ("date", "date"),
            ("volume", "volume"),
            ("pages", "pages"),
            ("doi", "DOI"),
            ("abstract", "abstractNote"),
        ],
        "report" => &[
            ("institution", "institution"),
            ("report_number", "reportNumber"),
            ("date", "date"),
            ("doi", "DOI"),
            ("arxiv_id", "extra"),
            ("abstract", "abstractNote"),
        ],
        _ => &[
            ("publication", "publicationTitle"),
            ("website", "websiteTitle"),
            ("date", "date"),
            ("volume", "volume"),
            ("pages", "pages"),
            ("doi", "DOI"),
            ("url", "url"),
            ("arxiv_id", "extra"),
            ("abstract", "abstractNote"),
        ],
    }
}

fn text<'a>(data: &'a Value, key: &str) -> &'a str {
    data.get(key).and_then(Value::as_str).unwrap_or("")
}

fn arxiv_id(data: &Value) -> String {
    crate::commands::export::arxiv_id(&crate::zotero_api::ApiItem {
        key: String::new(),
        version: 0,
        data: data.clone(),
    })
    .unwrap_or_default()
}

fn extra_value(data: &Value, key: &str) -> String {
    text(data, "extra")
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(key)
                .then(|| value.trim().to_string())
        })
        .unwrap_or_default()
}

/// Update one labelled Extra entry without discarding unrelated metadata.
fn set_extra(data: &mut Value, key: &str, value: &str) {
    let mut lines: Vec<_> = text(data, "extra")
        .lines()
        .filter(|line| {
            !line
                .split_once(':')
                .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key))
        })
        .map(str::to_string)
        .collect();
    if !value.trim().is_empty() {
        lines.push(format!("{key}: {}", value.trim()));
    }
    data["extra"] = json!(lines.join("\n"));
}

impl EditableItem {
    pub fn from_data(data: &Value) -> Self {
        let authors = data
            .get("creators")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|creator| {
                text(creator, "creatorType").is_empty() || text(creator, "creatorType") == "author"
            })
            .map(|creator| EditableAuthor {
                first: text(creator, "firstName").to_string(),
                last: text(creator, "lastName").to_string(),
                name: text(creator, "name").to_string(),
            })
            .collect();
        let fields = item_fields(text(data, "itemType"))
            .iter()
            .filter(|(name, key)| {
                data.get(*key).is_some() || (*name == "doi" && data.get("extra").is_some())
            })
            .map(|(name, key)| {
                let value = match *name {
                    "arxiv_id" => arxiv_id(data),
                    "doi" if text(data, key).is_empty() => extra_value(data, "DOI"),
                    _ => text(data, key).to_string(),
                };
                (name.to_string(), value)
            })
            .collect();
        Self {
            title: text(data, "title").to_string(),
            authors: EditableAuthors { names: authors },
            fields,
        }
    }

    /// Scalar fields precede one compact author list, matching both editor flows.
    pub fn to_editor_toml(&self) -> Result<String> {
        let quote = |value: &str| toml::Value::String(value.to_string()).to_string();
        let mut output = format!("title = {}\n", quote(&self.title));
        for name in [
            "journal",
            "journal_abbreviation",
            "thesis_type",
            "university",
            "book_title",
            "conference",
            "publication",
            "website",
            "institution",
            "publisher",
            "date",
            "place",
            "edition",
            "volume",
            "pages",
            "report_number",
            "doi",
            "url",
            "arxiv_id",
            "abstract",
        ] {
            if let Some(value) = self.fields.get(name) {
                output.push_str(&format!("{name} = {}\n", quote(value)));
            }
        }
        output.push_str("\n[authors]\n# One author per line, in publication order. Use name for an institution.\nnames = [\n");
        for author in &self.authors.names {
            if !author.name.trim().is_empty() {
                output.push_str(&format!("    {{ name = {} }},\n", quote(&author.name)));
            } else {
                output.push_str(&format!(
                    "    {{ first = {}, last = {} }},\n",
                    quote(&author.first),
                    quote(&author.last)
                ));
            }
        }
        output.push_str("]\n");
        Ok(output)
    }

    /// Apply edits to a copy, retaining hidden fields and non-author creators.
    pub fn fill_template(&self, mut data: Value) -> Result<Value> {
        let mapping = item_fields(text(&data, "itemType"));
        data["title"] = json!(self.title);
        for (name, value) in &self.fields {
            let Some((_, key)) = mapping.iter().find(|(field, _)| field == name) else {
                bail!("unknown field '{name}' for this item type");
            };
            match name.as_str() {
                "arxiv_id" => {
                    // Avoid modifying legacy storage when the ID is unchanged.
                    if value.trim() != arxiv_id(&data) {
                        if text(&data, "archive").eq_ignore_ascii_case("arXiv") {
                            data["archiveLocation"] = json!(value.trim());
                        } else {
                            set_extra(&mut data, "arXiv", value);
                        }
                    }
                }
                "doi" if data.get("DOI").is_none() => {
                    if value.trim() != extra_value(&data, "DOI") {
                        set_extra(&mut data, "DOI", value);
                    }
                }
                _ if data.get(*key).is_some() => data[*key] = json!(value),
                _ => bail!("field '{name}' is unsupported by this Zotero item template"),
            }
        }
        let mut authors = Vec::new();
        for author in &self.authors.names {
            if !author.name.trim().is_empty() {
                if !author.first.trim().is_empty() || !author.last.trim().is_empty() {
                    bail!("use either first/last or name for each author");
                }
                authors.push(json!({"creatorType": "author", "name": author.name}));
            } else if !author.first.trim().is_empty() || !author.last.trim().is_empty() {
                authors.push(json!({"creatorType": "author", "firstName": author.first, "lastName": author.last}));
            }
        }
        if let Some(original) = data.get("creators").and_then(Value::as_array) {
            let mut replacements = authors.into_iter();
            let mut creators = Vec::new();
            for creator in original {
                if text(creator, "creatorType").is_empty()
                    || text(creator, "creatorType") == "author"
                {
                    creators.extend(replacements.next());
                } else {
                    creators.push(creator.clone());
                }
            }
            creators.extend(replacements);
            data["creators"] = json!(creators);
        }
        Ok(data)
    }

    /// Send only changed keys so hidden metadata never gets reset by defaults.
    pub fn patch_for(&self, original: &Value) -> Result<Value> {
        let updated = self.fill_template(original.clone())?;
        let fields = updated
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, value)| original.get(*key) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<serde_json::Map<_, _>>();
        Ok(Value::Object(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article() -> Value {
        json!({
            "itemType": "journalArticle", "title": "Example article",
            "publicationTitle": "Physical Review B", "journalAbbreviation": "Phys. Rev. B",
            "date": "2026", "volume": "114", "pages": "012345", "DOI": "10.1103/example",
            "url": "", "abstractNote": "Summary", "issue": "2", "shortTitle": "Short",
            "rights": "CC BY", "series": "Series", "seriesTitle": "Section", "seriesNumber": "1",
            "archive": "", "archiveLocation": "", "callNumber": "Shelf 1", "libraryCatalog": "Catalog",
            "numPages": "174", "PMID": "123", "PMCID": "PMC123", "extra": "Keep this note",
            "tags": [{"tag": "Ada2026", "type": 1}], "collections": ["ABCD1234"],
            "creators": [
                {"creatorType": "author", "firstName": "Ada", "lastName": "Lovelace"},
                {"creatorType": "editor", "name": "Editorial Board"},
                {"creatorType": "author", "firstName": "Ludwig", "lastName": "van Beethoven"}
            ]
        })
    }

    #[test]
    fn article_form_uses_approved_fields_and_preserves_hidden_metadata() {
        let original = article();
        let rendered = EditableItem::from_data(&original).to_editor_toml().unwrap();
        let document: toml::Value = toml::from_str(&rendered).unwrap();
        let mut keys: Vec<_> = document
            .as_table()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "abstract",
                "authors",
                "date",
                "doi",
                "journal",
                "journal_abbreviation",
                "pages",
                "title",
                "url",
                "volume"
            ]
        );
        assert!(rendered.contains("{ first = \"Ada\", last = \"Lovelace\" },"));
        let mut edited: EditableItem = toml::from_str(&rendered).unwrap();
        assert_eq!(edited.patch_for(&original).unwrap(), json!({}));
        edited.fields.insert("journal".into(), "New journal".into());
        edited
            .fields
            .insert("abstract".into(), "New summary".into());
        edited.authors.names[1].last = "de la Cruz".into();
        let updated = edited.fill_template(original.clone()).unwrap();
        assert_eq!(updated["publicationTitle"], "New journal");
        assert_eq!(updated["abstractNote"], "New summary");
        assert_eq!(updated["creators"][2]["lastName"], "de la Cruz");
        for (key, value) in original.as_object().unwrap() {
            if !["publicationTitle", "abstractNote", "creators"].contains(&key.as_str()) {
                assert_eq!(&updated[key], value, "hidden field changed: {key}");
            }
        }
        assert_eq!(updated["creators"][1], original["creators"][1]);
    }

    #[test]
    fn thesis_form_supports_degree_university_doi_and_arxiv() {
        let template = json!({
            "itemType": "thesis", "title": "", "thesisType": "", "university": "", "date": "",
            "place": "", "url": "", "abstractNote": "", "extra": "Unrelated note",
            "archive": "", "archiveLocation": "", "numPages": "", "rights": "",
            "creators": [{"creatorType": "author", "firstName": "", "lastName": ""}],
            "tags": [], "collections": []
        });
        let rendered = EditableItem::from_data(&template).to_editor_toml().unwrap();
        let document: toml::Value = toml::from_str(&rendered).unwrap();
        let mut keys: Vec<_> = document
            .as_table()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "abstract",
                "arxiv_id",
                "authors",
                "date",
                "doi",
                "place",
                "thesis_type",
                "title",
                "university",
                "url"
            ]
        );
        let mut edited: EditableItem = toml::from_str(&rendered).unwrap();
        edited.title = "Dissertation".into();
        edited
            .fields
            .insert("thesis_type".into(), "Ph.D. thesis".into());
        edited
            .fields
            .insert("university".into(), "ETH Zürich".into());
        edited.fields.insert("doi".into(), "10.1234/thesis".into());
        edited.fields.insert("arxiv_id".into(), "2601.01234".into());
        let created = edited.fill_template(template).unwrap();
        assert_eq!(created["thesisType"], "Ph.D. thesis");
        assert_eq!(created["university"], "ETH Zürich");
        assert_eq!(created["creators"], json!([]));
        assert_eq!(arxiv_id(&created), "2601.01234");
        assert!(text(&created, "extra").contains("Unrelated note"));
        assert_eq!(extra_value(&created, "DOI"), "10.1234/thesis");
        let reloaded = EditableItem::from_data(&created);
        assert_eq!(reloaded.fields["doi"], "10.1234/thesis");
        assert_eq!(reloaded.patch_for(&created).unwrap(), json!({}));
    }

    #[test]
    fn author_removal_and_addition_preserve_other_roles() {
        let original = article();
        let mut edited = EditableItem::from_data(&original);
        edited.authors.names.clear();
        let updated = edited.fill_template(original.clone()).unwrap();
        assert_eq!(updated["creators"], json!([original["creators"][1]]));
        edited.authors.names.push(EditableAuthor {
            name: "Research Collaboration".into(),
            ..Default::default()
        });
        let updated = edited.fill_template(original).unwrap();
        assert_eq!(
            updated["creators"][0],
            json!({"creatorType": "author", "name": "Research Collaboration"})
        );
        assert_eq!(updated["creators"][1]["creatorType"], "editor");
    }

    #[test]
    fn arxiv_edits_preserve_legacy_storage_and_unrelated_extra() {
        let original = json!({"itemType": "thesis", "title": "Thesis", "extra": "Note\nDOI: 10.1234/example", "archive": "arXiv", "archiveLocation": "2401.01234", "creators": []});
        let mut edited = EditableItem::from_data(&original);
        assert_eq!(edited.patch_for(&original).unwrap(), json!({}));
        edited.fields.insert("arxiv_id".into(), "2601.01234".into());
        let updated = edited.fill_template(original.clone()).unwrap();
        assert_eq!(updated["archiveLocation"], "2601.01234");
        assert_eq!(updated["extra"], original["extra"]);
        edited.fields.insert("arxiv_id".into(), "".into());
        let updated = edited.fill_template(original).unwrap();
        assert!(arxiv_id(&updated).is_empty());
    }

    #[test]
    fn rejects_unknown_fields_and_ambiguous_author_names() {
        let original = article();
        let mut edited = EditableItem::from_data(&original);
        edited.fields.insert("seriesTitle".into(), "Hidden".into());
        assert!(edited.patch_for(&original).is_err());
        edited.fields.remove("seriesTitle");
        edited.authors.names[0].name = "Institution".into();
        assert!(edited.patch_for(&original).is_err());
    }

    #[test]
    fn editor_text_round_trips_quotes_unicode_and_newlines() {
        let original = json!({"itemType": "journalArticle", "title": "A \"quote\"\nB\\C", "abstractNote": "Résumé\nSecond line", "creators": [
            {"creatorType": "author", "firstName": "A\\B\nC", "lastName": "O’Neil \"D\""},
            {"creatorType": "author", "name": "Group \"X\"\\Y"}
        ]});
        let rendered = EditableItem::from_data(&original).to_editor_toml().unwrap();
        let edited: EditableItem = toml::from_str(&rendered).unwrap();
        assert_eq!(edited.patch_for(&original).unwrap(), json!({}));
    }

    #[test]
    fn books_expose_publisher_and_edition_but_hide_cataloguing_fields() {
        let original = json!({"itemType": "book", "title": "Book", "publisher": "Press", "edition": "2", "series": "Hidden", "ISBN": "Hidden", "numPages": "174", "creators": []});
        let rendered = EditableItem::from_data(&original).to_editor_toml().unwrap();
        assert!(rendered.contains("publisher = \"Press\""));
        assert!(rendered.contains("edition = \"2\""));
        assert!(!rendered.contains("series"));
        assert!(!rendered.contains("ISBN"));
        assert!(!rendered.contains("numPages"));
    }
}
