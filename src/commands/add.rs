use anyhow::Result;

use crate::config::Config;
use crate::crossref;
use crate::tagging;
use crate::zotero_api::ZoteroClient;

pub fn run(doi: &str) -> Result<()> {
    let config = Config::load()?;
    let client = ZoteroClient::from_config(&config)?;

    println!("Looking up {} on CrossRef...", doi);
    let work = crossref::fetch_work(doi)?;

    let item_type = crossref::item_type_for_work(&work);
    let template = client.item_template(item_type)?;
    let mut item = crossref::fill_item_template(template, &work);

    let creators = crossref::creators(&work);
    let year = crossref::best_year(&work);
    let base = tagging::base_tag(&creators, year)?;
    let tag = tagging::unique_tag(&client, &base)?;
    item["tags"] = serde_json::json!([{ "tag": tag }]);

    let created = client.create_item(item)?;
    println!(
        "Added '{}' as [{}] (key {})",
        created.title(),
        tag,
        created.key
    );

    Ok(())
}
