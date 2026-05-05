use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const APP_CONFIG_DIR: &str = "agent-pet";
const LEGACY_CONFIG_DIR: &str = "codex-pet";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub live_source_paths: HashMap<String, String>,
    #[serde(default)]
    pub live_source_focus_targets: HashMap<String, String>,
    #[serde(default)]
    pub live_source_enabled: HashMap<String, bool>,
    #[serde(default)]
    pub live_source_prefix_enabled: bool,
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            live_source_paths: HashMap::new(),
            live_source_focus_targets: HashMap::new(),
            live_source_enabled: HashMap::new(),
            live_source_prefix_enabled: false,
            language: default_language(),
        }
    }
}

fn default_language() -> String {
    "en".to_string()
}

pub fn load_settings() -> AppSettings {
    let Some(path) = readable_settings_path() else {
        return AppSettings::default();
    };

    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => AppSettings::default(),
    }
}

pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path().ok_or_else(|| "Config dir not found".to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Settings path has no parent".to_string())?;

    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|config| config.join(APP_CONFIG_DIR).join("settings.json"))
}

fn readable_settings_path() -> Option<PathBuf> {
    let config = dirs::config_dir()?;
    let current = config.join(APP_CONFIG_DIR).join("settings.json");
    if current.exists() {
        return Some(current);
    }

    let legacy = config.join(LEGACY_CONFIG_DIR).join("settings.json");
    if legacy.exists() {
        return Some(legacy);
    }

    Some(current)
}
