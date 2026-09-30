use anyhow::Result;
use std::collections::BTreeMap;

use crate::cache::Cache;
use crate::config::Config;
use crate::sync;
use crate::zotero_api::ZoteroClient;

pub fn run() -> Result<()> {
    let config = Config::load()?;
    let read_client = ZoteroClient::read_preferred(&config)?;
    let write_client = ZoteroClient::write_preferred(&config)?;
    let mut cache = Cache::load()?;
    sync::sync_library(&read_client, &write_client, &mut cache)?;

    let keys = cache
        .top_level_items(None, None)
        .into_iter()
        .filter_map(|item| item.citation_key())
        .collect::<Vec<_>>();
    let adjustments = required_adjustments(keys.iter().map(String::as_str));

    if adjustments.is_empty() {
        println!("All citation-key suffixes are compact.");
    } else {
        for (current, suggested) in adjustments {
            println!("{current} -> {suggested}");
        }
    }

    Ok(())
}

fn required_adjustments<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<(String, String)> {
    let mut groups: BTreeMap<String, Vec<(u8, String)>> = BTreeMap::new();

    for key in keys {
        let (base, rank) = match key.as_bytes().last().copied() {
            Some(suffix @ b'a'..=b'z') => (&key[..key.len() - 1], suffix - b'a' + 1),
            _ => (key, 0),
        };
        groups
            .entry(base.to_string())
            .or_default()
            .push((rank, key.to_string()));
    }

    let mut adjustments = Vec::new();
    for (base, mut members) in groups {
        members.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        for (expected_rank, (_, current)) in members.into_iter().enumerate() {
            let expected = if expected_rank == 0 {
                base.clone()
            } else {
                format!("{base}{}", (b'a' + expected_rank as u8 - 1) as char)
            };
            if current != expected {
                adjustments.push((current, expected));
            }
        }
    }
    adjustments
}

#[cfg(test)]
mod tests {
    use super::required_adjustments;

    #[test]
    fn reports_missing_base_and_compacts_following_suffixes() {
        let adjustments = required_adjustments(["claeys2018a", "claeys2018b"]);
        assert_eq!(
            adjustments,
            [
                ("claeys2018a".to_string(), "claeys2018".to_string()),
                ("claeys2018b".to_string(), "claeys2018a".to_string()),
            ]
        );
    }

    #[test]
    fn reports_an_internal_suffix_gap() {
        let adjustments = required_adjustments(["smith2020", "smith2020b"]);
        assert_eq!(
            adjustments,
            [("smith2020b".to_string(), "smith2020a".to_string())]
        );
    }

    #[test]
    fn accepts_a_compact_sequence() {
        assert!(required_adjustments(["smith2020", "smith2020a"]).is_empty());
    }
}
