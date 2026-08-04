use anyhow::{Context, Result};
use std::io::{self, Write};

use crate::config::Config;

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    if let Some(d) = default {
        print!("{} [{}]: ", label, d);
    } else {
        print!("{}: ", label);
    }
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line).context("reading input")?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.unwrap_or("").to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

pub fn run() -> Result<()> {
    let existing = Config::load().unwrap_or_default();

    println!("Configure zotero-cli. Get an API key at https://www.zotero.org/settings/keys");
    println!("Your numeric user ID is shown on that same page.\n");

    let library_type = loop {
        let v = prompt(
            "Library type (user/group)",
            Some(if existing.library_type.is_empty() {
                "user"
            } else {
                &existing.library_type
            }),
        )?;
        if v == "user" || v == "group" {
            break v;
        }
        println!("Please enter 'user' or 'group'.");
    };

    let library_id = prompt(
        "Library ID (numeric)",
        Some(existing.library_id.as_str()).filter(|s| !s.is_empty()),
    )?;

    let api_key = prompt(
        "API key",
        Some(existing.api_key.as_str()).filter(|s| !s.is_empty()),
    )?;

    let data_dir_default = existing.zotero_data_dir.clone();
    let data_dir_input = prompt(
        "Local Zotero data directory (blank = auto-detect)",
        data_dir_default.as_deref(),
    )?;
    let zotero_data_dir = if data_dir_input.is_empty() {
        None
    } else {
        Some(data_dir_input)
    };

    let config = Config {
        library_type,
        library_id,
        api_key,
        zotero_data_dir,
        current_collection_key: existing.current_collection_key,
    };
    config.save()?;
    println!("\nSaved config to {}", Config::path()?.display());

    Ok(())
}
