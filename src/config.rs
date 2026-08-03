use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Config {
    /// "user" or "group"
    pub library_type: String,
    /// Numeric Zotero user ID or group ID
    pub library_id: String,
    /// Zotero API key (from https://www.zotero.org/settings/keys)
    pub api_key: String,
    /// Optional override for the local Zotero data directory (contains storage/)
    #[serde(default)]
    pub zotero_data_dir: Option<String>,
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("could not determine config directory")?
            .join("zotero-cli");
        Ok(dir.join("config.toml"))
    }

    pub fn load() -> Result<Config> {
        let path = Self::path()?;
        if !path.exists() {
            bail!(
                "no config found at {}. Run `zotero config` first.",
                path.display()
            );
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).context("parsing config.toml")?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating config directory {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        let mut f = fs::File::create(&path)
            .with_context(|| format!("creating config file {}", path.display()))?;
        f.write_all(text.as_bytes())?;
        Ok(())
    }

    fn library_path(&self) -> String {
        let segment = match self.library_type.as_str() {
            "group" => "groups",
            _ => "users",
        };
        format!("{}/{}", segment, self.library_id)
    }

    /// e.g. https://api.zotero.org/users/12345
    pub fn library_base_url(&self) -> String {
        format!("https://api.zotero.org/{}", self.library_path())
    }

    /// e.g. http://localhost:23119/api/users/12345
    pub fn local_library_base_url(&self) -> String {
        format!("http://localhost:23119/api/{}", self.library_path())
    }
}
