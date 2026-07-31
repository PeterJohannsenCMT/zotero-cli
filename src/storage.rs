use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

use crate::config::Config;

/// Locate the local Zotero data directory (the one containing `storage/`).
/// Checks a config override first, then common default locations, then
/// falls back to parsing `prefs.js` from the Zotero Firefox-style profile.
pub fn find_data_dir(config: &Config) -> Result<PathBuf> {
    if let Some(dir) = &config.zotero_data_dir {
        let p = PathBuf::from(dir);
        if p.join("storage").is_dir() {
            return Ok(p);
        }
        bail!(
            "configured zotero_data_dir '{}' does not contain a storage/ directory",
            dir
        );
    }

    if let Some(home) = dirs::home_dir() {
        for candidate in [home.join("Zotero"), home.join("Documents").join("Zotero")] {
            if candidate.join("storage").is_dir() {
                return Ok(candidate);
            }
        }
    }

    if let Some(dir) = find_data_dir_from_profile()
        && dir.join("storage").is_dir()
    {
        return Ok(dir);
    }

    bail!(
        "could not locate your Zotero data directory automatically. \
         Set `zotero_data_dir` in the config file (run `zotero config`) to the folder \
         containing your `storage/` subdirectory."
    )
}

/// Search Zotero's profile prefs.js for `extensions.zotero.dataDir`.
fn find_data_dir_from_profile() -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    let profile_roots: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec![home.join("Library/Application Support/Zotero/Profiles")]
    } else if cfg!(target_os = "windows") {
        vec![home.join("AppData/Roaming/Zotero/Zotero/Profiles")]
    } else {
        vec![home.join(".zotero/zotero"), home.join(".mozilla-zotero")]
    };

    for root in profile_roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let prefs = entry.path().join("prefs.js");
            if let Some(dir) = parse_data_dir_pref(&prefs) {
                return Some(dir);
            }
        }
    }
    None
}

fn parse_data_dir_pref(prefs_path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(prefs_path).ok()?;
    for line in text.lines() {
        if line.contains("extensions.zotero.dataDir") {
            // user_pref("extensions.zotero.dataDir", "/Users/foo/Zotero");
            if let Some(start) = line.find(", \"") {
                let rest = &line[start + 3..];
                if let Some(end) = rest.find('"') {
                    return Some(PathBuf::from(&rest[..end]));
                }
            }
        }
    }
    None
}

/// Resolve the on-disk path of an attachment item's file, given the parent
/// data directory. Handles imported files (stored under storage/<key>/) and
/// linked files (absolute path stored directly on the item).
pub fn resolve_attachment_path(data_dir: &Path, key: &str, filename: &str) -> PathBuf {
    data_dir.join("storage").join(key).join(filename)
}
