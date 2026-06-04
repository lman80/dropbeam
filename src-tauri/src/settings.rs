//! Settings persistence (a small JSON file in the app config dir).

use std::fs;
use std::path::{Path, PathBuf};

use crate::models::Settings;

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// Load settings, filling in runtime-derived defaults (download dir, name) when
/// they're blank.
pub fn load(config_dir: &Path, default_download: &str, default_name: &str) -> Settings {
    let mut s = match fs::read_to_string(settings_path(config_dir)) {
        Ok(txt) => serde_json::from_str::<Settings>(&txt).unwrap_or_default(),
        Err(_) => Settings::default(),
    };
    if s.download_dir.trim().is_empty() {
        s.download_dir = default_download.to_string();
    }
    if s.display_name.trim().is_empty() {
        s.display_name = default_name.to_string();
    }
    s
}

pub fn save(config_dir: &Path, settings: &Settings) -> Result<(), String> {
    let _ = fs::create_dir_all(config_dir);
    let txt = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    write_atomic(&settings_path(config_dir), txt.as_bytes()).map_err(|e| e.to_string())
}

/// Write a file by writing to a temp sibling then renaming, so a crash mid-write
/// never corrupts the real file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
