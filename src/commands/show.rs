use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::cache::Cache;
use crate::config::Config;
use crate::display::{self, Highlight};
use crate::keywords::Keywords;
use crate::sync;
use crate::zotero_api::{ApiItem, ZoteroClient};

pub fn run(identifier: &str) -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let item = cache
        .find_by_identifier(identifier)
        .ok_or_else(|| anyhow!("no item found for '{}'", identifier))?;
    let keyword_store = Keywords::load()?;
    let keywords = keyword_store.for_item(&item.key);

    print_line("Title", &item.title(), Highlight::Title);
    print_line("Authors", &format_creators(item), Highlight::Creator);
    print_line("Date", &format_date(item.date()), Highlight::Date);
    print_line(
        "Journal",
        string_field(&item.data, "publicationTitle"),
        Highlight::Venue,
    );
    if let Some(citation_key) = item.citation_key()
        && citation_key != item.key
    {
        print_line("Citation key", &citation_key, Highlight::Key);
    }
    print_line("Version", &item.version.to_string(), Highlight::Muted);
    print_line(
        "Book title",
        string_field(&item.data, "bookTitle"),
        Highlight::Venue,
    );
    print_line(
        "Proceedings",
        string_field(&item.data, "proceedingsTitle"),
        Highlight::Venue,
    );
    print_line(
        "Website",
        string_field(&item.data, "websiteTitle"),
        Highlight::Venue,
    );
    print_line(
        "Publisher",
        string_field(&item.data, "publisher"),
        Highlight::Muted,
    );
    print_line("Place", string_field(&item.data, "place"), Highlight::Muted);
    print_line(
        "Volume",
        string_field(&item.data, "volume"),
        Highlight::Muted,
    );
    print_line("Issue", string_field(&item.data, "issue"), Highlight::Muted);
    print_line("Pages", string_field(&item.data, "pages"), Highlight::Muted);
    print_line(
        "DOI",
        string_field(&item.data, "DOI"),
        Highlight::Identifier,
    );
    print_line(
        "ISBN",
        string_field(&item.data, "ISBN"),
        Highlight::Identifier,
    );
    print_line(
        "Accessed",
        string_field(&item.data, "accessDate"),
        Highlight::Date,
    );
    print_line(
        "Catalog",
        string_field(&item.data, "libraryCatalog"),
        Highlight::Muted,
    );
    print_line(
        "Archive",
        string_field(&item.data, "archive"),
        Highlight::Muted,
    );
    print_line(
        "Archive loc.",
        string_field(&item.data, "archiveLocation"),
        Highlight::Muted,
    );
    print_line(
        "Rights",
        string_field(&item.data, "rights"),
        Highlight::Muted,
    );
    print_line("Keywords", &keywords.join(", "), Highlight::Keyword);
    print_line(
        "Collections",
        &format_collections(item, &cache),
        Highlight::Collection,
    );
    print_multiline(
        "Abstract",
        string_field(&item.data, "abstractNote"),
        Highlight::Body,
    );
    print_multiline("Extra", string_field(&item.data, "extra"), Highlight::Body);

    Ok(())
}

fn print_line(label: &str, value: &str, group: Highlight) {
    let value = value.trim();
    if !value.is_empty() {
        let label = format!("{:<13}", format!("{label}:"));
        println!(
            "{} {}",
            display::highlight(&label, Highlight::Label),
            display::highlight(value, group)
        );
    }
}

fn print_multiline(label: &str, value: &str, group: Highlight) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }

    println!(
        "{}",
        display::highlight(&format!("{label}:"), Highlight::Section)
    );
    for line in value.lines() {
        println!("  {}", display::highlight(line.trim(), group));
    }
}

fn string_field<'a>(data: &'a Value, key: &str) -> &'a str {
    data.get(key).and_then(Value::as_str).unwrap_or("")
}

fn format_creators(item: &ApiItem) -> String {
    item.data
        .get("creators")
        .and_then(Value::as_array)
        .map(|creators| {
            creators
                .iter()
                .filter_map(|creator| {
                    let name = creator
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| {
                            let first = creator
                                .get("firstName")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let last = creator
                                .get("lastName")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let full = format_personal_name(first, last);
                            (!full.is_empty()).then_some(full)
                        })?;
                    Some(name)
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default()
}

fn format_personal_name(first: &str, last: &str) -> String {
    let initials = first
        .split_whitespace()
        .filter_map(|name| name.chars().next())
        .map(|initial| format!("{}.", initial))
        .collect::<Vec<_>>()
        .join(" ");

    [initials.as_str(), last]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_date(date: &str) -> String {
    let date = date.trim();
    let mut parts = date.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return date.to_string();
    };

    let Ok(month) = month.parse::<usize>() else {
        return date.to_string();
    };
    let Ok(day) = day.parse::<u32>() else {
        return date.to_string();
    };

    let Some(month_name) = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .get(month.saturating_sub(1)) else {
        return date.to_string();
    };

    if day == 0 {
        return date.to_string();
    }

    format!("{day}. {month_name} {year}")
}

fn format_collections(item: &ApiItem, cache: &Cache) -> String {
    item.data
        .get("collections")
        .and_then(Value::as_array)
        .map(|collections| {
            collections
                .iter()
                .filter_map(Value::as_str)
                .map(|key| {
                    cache
                        .collections
                        .get(key)
                        .map(|collection| collection.name().to_string())
                        .unwrap_or_else(|| key.to_string())
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}
