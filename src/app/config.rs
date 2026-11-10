use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_explorer_width")]
    pub explorer_width: u16,
    #[serde(default = "default_tab_width")]
    pub tab_width: usize,
    #[serde(default = "default_true")]
    pub explorer_visible: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub claude_enabled: bool,
    #[serde(default)]
    pub types_note_dismissed: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            explorer_width: default_explorer_width(),
            tab_width: default_tab_width(),
            explorer_visible: true,
            theme: default_theme(),
            claude_enabled: false,
            types_note_dismissed: false,
        }
    }
}

pub const EXPLORER_MIN_WIDTH: u16 = 28;

fn default_explorer_width() -> u16 {
    EXPLORER_MIN_WIDTH
}
fn default_tab_width() -> usize {
    4
}
fn default_true() -> bool {
    true
}
fn default_theme() -> String {
    "p0".into()
}

impl Config {
    pub fn load() -> Self {
        match fs::read_to_string(config_path()) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => {
                let Ok(text) = fs::read_to_string(legacy_config_path()) else {
                    return Self::default();
                };
                toml::from_str(&text).unwrap_or_default()
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        fs::write(path, text)
    }
}

pub fn config_path() -> PathBuf {
    app_dir("r3vu").join("config.toml")
}

fn legacy_config_path() -> PathBuf {
    app_dir("p0").join("config.toml")
}

pub fn log_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("r3vu")
        .join("r3vu.log")
}

fn app_dir(name: &str) -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(name)
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn claude_is_off_by_default() {
        assert!(!Config::default().claude_enabled);
    }
}
