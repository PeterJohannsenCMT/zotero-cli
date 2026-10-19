use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "zotero-offline-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let config = if cfg!(target_os = "macos") {
            root.join("Library/Application Support")
        } else {
            root.join(".config")
        }
        .join("zotero-cli");
        let cache = if cfg!(target_os = "macos") {
            root.join("Library/Caches")
        } else {
            root.join(".cache")
        }
        .join("zotero-cli");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "library_type = 'user'\nlibrary_id = '12345'\napi_key = 'unused'\n",
        )
        .unwrap();
        let snapshot = serde_json::json!({
            "last_synced_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            "library_version": 1,
            "items": {"ABCDEFGH": {"key": "ABCDEFGH", "version": 1, "data": {
                "itemType": "journalArticle", "title": "Offline quantum optics", "date": "2020",
                "publicationTitle": "Physical Review B", "tags": [{"tag": "smith2020"}],
                "creators": [{"creatorType": "author", "lastName": "Smith", "firstName": "Jane"}]
            }}}, "collections": {}
        });
        std::fs::write(cache.join("cache.json"), snapshot.to_string()).unwrap();
        Self(root)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_zotero"))
            .env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join(".config"))
            .env("XDG_CACHE_HOME", self.0.join(".cache"))
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn offline_cached_reads_and_exports() {
    let fixture = Fixture::new();
    for args in [
        vec!["--offline", "list"],
        vec!["search", "--offline", "quantum"],
        vec!["--offline", "show", "smith2020"],
        vec!["--offline", "export", "smith2020"],
        vec!["--offline", "export", "--bib", "smith2020"],
    ] {
        let output = fixture.run(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("smith2020"));
        if args.contains(&"--bib") {
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(text.contains("@article{smith2020,"));
            assert!(text.contains("journal = prb,"));
            assert!(text.contains("Smith, Jane"));
        }
    }
}

#[test]
fn offline_rejects_writes_before_api_or_editor_access() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--offline", "add", "10.1234/example"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unavailable offline"));
}
