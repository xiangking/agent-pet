// Pet state machine
// Manages state transitions based on messages (animation is handled by the frontend)

use crate::message::{default_message_map, PetNotice, PetUsageMetric};
use crate::pet::{load_pet_config, PetConfig, PetState};
use crate::settings::{load_settings, save_settings, AppSettings};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{Emitter, LogicalSize, Manager};

const CELL_WIDTH: f64 = 192.0;
const CELL_HEIGHT: f64 = 208.0;
const MIN_WINDOW_WIDTH: f64 = 240.0;
const BUBBLE_SPACE_HEIGHT: f64 = 92.0;
const CODEX_ORIGINAL_SCALE: f64 = 0.45;
const DEFAULT_PET_SCALE: f64 = CODEX_ORIGINAL_SCALE;
const MIN_PET_SCALE: f64 = CODEX_ORIGINAL_SCALE * 0.75;
const MAX_PET_SCALE: f64 = CODEX_ORIGINAL_SCALE * 1.5;
const MAX_USAGE_METRICS: usize = 24;

pub struct PetStateMachine {
    app_handle: tauri::AppHandle,
    current_state: PetState,
    loaded_pet: Option<PetConfig>,
    message_map: HashMap<String, String>,
    websocket_enabled: bool,
    websocket_port: u16,
    codex_monitor_enabled: bool,
    claude_monitor_enabled: bool,
    opencode_monitor_enabled: bool,
    openclaw_monitor_enabled: bool,
    hermes_monitor_enabled: bool,
    antigravity_monitor_enabled: bool,
    live_source_paths: HashMap<String, String>,
    live_source_focus_targets: HashMap<String, String>,
    live_source_prefix_enabled: bool,
    usage_metrics: Vec<PetUsageMetric>,
    pet_scale: f64,
    language: String,
}

impl PetStateMachine {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        let settings = load_settings();

        Self {
            app_handle,
            current_state: PetState::Idle,
            loaded_pet: None,
            message_map: default_message_map(),
            websocket_enabled: true,
            websocket_port: 8765,
            codex_monitor_enabled: live_source_enabled_from_settings(&settings, "codex"),
            claude_monitor_enabled: live_source_enabled_from_settings(&settings, "claude"),
            opencode_monitor_enabled: live_source_enabled_from_settings(&settings, "opencode"),
            openclaw_monitor_enabled: live_source_enabled_from_settings(&settings, "openclaw"),
            hermes_monitor_enabled: live_source_enabled_from_settings(&settings, "hermes"),
            antigravity_monitor_enabled: live_source_enabled_from_settings(
                &settings,
                "antigravity",
            ),
            live_source_paths: live_source_paths_from_settings(&settings),
            live_source_focus_targets: live_source_focus_targets_from_settings(&settings),
            live_source_prefix_enabled: settings.live_source_prefix_enabled,
            usage_metrics: Vec::new(),
            pet_scale: DEFAULT_PET_SCALE,
            language: normalize_language(&settings.language).to_string(),
        }
    }

    pub fn current_state(&self) -> PetState {
        self.current_state
    }

    pub fn message_map(&self) -> &HashMap<String, String> {
        &self.message_map
    }

    pub fn set_message_map(&mut self, map: HashMap<String, String>) {
        self.message_map = map;
    }

    pub fn websocket_enabled(&self) -> bool {
        self.websocket_enabled
    }

    pub fn websocket_port(&self) -> u16 {
        self.websocket_port
    }

    pub fn set_websocket_enabled(&mut self, enabled: bool) {
        self.websocket_enabled = enabled;
    }

    pub fn codex_monitor_enabled(&self) -> bool {
        self.codex_monitor_enabled
    }

    pub fn set_codex_monitor_enabled(&mut self, enabled: bool) {
        self.codex_monitor_enabled = enabled;
    }

    pub fn claude_monitor_enabled(&self) -> bool {
        self.claude_monitor_enabled
    }

    pub fn opencode_monitor_enabled(&self) -> bool {
        self.opencode_monitor_enabled
    }

    pub fn openclaw_monitor_enabled(&self) -> bool {
        self.openclaw_monitor_enabled
    }

    pub fn hermes_monitor_enabled(&self) -> bool {
        self.hermes_monitor_enabled
    }

    pub fn antigravity_monitor_enabled(&self) -> bool {
        self.antigravity_monitor_enabled
    }

    pub fn live_source_enabled(&self, source: &str) -> bool {
        match source {
            "codex" => self.codex_monitor_enabled,
            "claude" => self.claude_monitor_enabled,
            "opencode" => self.opencode_monitor_enabled,
            "openclaw" => self.openclaw_monitor_enabled,
            "hermes" => self.hermes_monitor_enabled,
            "antigravity" => self.antigravity_monitor_enabled,
            _ => false,
        }
    }

    pub fn set_live_source_enabled(&mut self, source: &str, enabled: bool) -> Result<(), String> {
        let updated = match source {
            "codex" => {
                self.codex_monitor_enabled = enabled;
                true
            }
            "claude" => {
                self.claude_monitor_enabled = enabled;
                true
            }
            "opencode" => {
                self.opencode_monitor_enabled = enabled;
                true
            }
            "openclaw" => {
                self.openclaw_monitor_enabled = enabled;
                true
            }
            "hermes" => {
                self.hermes_monitor_enabled = enabled;
                true
            }
            "antigravity" => {
                self.antigravity_monitor_enabled = enabled;
                true
            }
            _ => false,
        };

        if updated {
            self.persist_settings()
        } else {
            Err(format!("Unknown live source: {}", source))
        }
    }

    pub fn live_source_path(&self, source: &str) -> Option<String> {
        self.live_source_paths.get(source).cloned()
    }

    pub fn live_source_paths(&self) -> HashMap<String, String> {
        self.live_source_paths.clone()
    }

    pub fn live_source_focus_targets(&self) -> HashMap<String, String> {
        self.live_source_focus_targets.clone()
    }

    pub fn set_live_source_path(&mut self, source: &str, path: String) -> Result<(), String> {
        if !is_known_live_source(source) {
            return Err(format!("Unknown live source: {}", source));
        }

        let defaults = default_live_source_paths();
        let fallback = defaults
            .get(source)
            .ok_or_else(|| format!("Unknown live source: {}", source))?;
        let path = match path.trim() {
            "" => fallback.clone(),
            trimmed => trimmed.to_string(),
        };

        self.live_source_paths.insert(source.to_string(), path);
        self.persist_settings()
    }

    pub fn live_source_focus_target(&self, source: &str) -> Result<String, String> {
        if !is_known_live_source(source) {
            return Err(format!("Unknown live source: {}", source));
        }

        self.live_source_focus_targets
            .get(source)
            .cloned()
            .ok_or_else(|| format!("Unknown live source: {}", source))
    }

    pub fn set_live_source_focus_target(
        &mut self,
        source: &str,
        target: String,
    ) -> Result<(), String> {
        if !is_known_live_source(source) {
            return Err(format!("Unknown live source: {}", source));
        }

        let defaults = default_live_source_focus_targets();
        let fallback = defaults
            .get(source)
            .ok_or_else(|| format!("Unknown live source: {}", source))?;
        let target = match target.trim() {
            "" => fallback.clone(),
            trimmed => trimmed.to_string(),
        };

        validate_focus_target(&target)?;
        self.live_source_focus_targets
            .insert(source.to_string(), target);
        self.persist_settings()
    }

    pub fn live_source_prefix_enabled(&self) -> bool {
        self.live_source_prefix_enabled
    }

    pub fn set_live_source_prefix_enabled(&mut self, enabled: bool) -> Result<(), String> {
        self.live_source_prefix_enabled = enabled;
        self.persist_settings()
    }

    pub fn pet_scale(&self) -> f64 {
        self.pet_scale
    }

    pub fn set_pet_scale(&mut self, scale: f64) -> f64 {
        let scale = scale.clamp(MIN_PET_SCALE, MAX_PET_SCALE);
        self.pet_scale = scale;

        if let Some(window) = self.app_handle.get_webview_window("pet") {
            let width = (CELL_WIDTH * scale).max(MIN_WINDOW_WIDTH);
            let height = CELL_HEIGHT * scale + BUBBLE_SPACE_HEIGHT;
            let _ = window.set_size(LogicalSize::new(width, height));
        }

        let _ = self.app_handle.emit("pet-scale-changed", scale);
        scale
    }

    pub fn show_codex_bubble(&self, text: &str, source: &str) {
        if let Some(window) = self.app_handle.get_webview_window("pet") {
            let _ = window.emit("codex-bubble", codex_bubble_payload(text, source));
        }
    }

    pub fn show_notice(&self, notice: &PetNotice) {
        if let Some(window) = self.app_handle.get_webview_window("notices") {
            let _ = window.show();
            let _ = window.emit("pet-notice", notice_payload(notice));
        }
    }

    pub fn show_usage_metric(&self, metric: &PetUsageMetric) {
        if let Some(window) = self.app_handle.get_webview_window("pet") {
            let _ = window.emit("pet-usage", usage_metric_payload(metric));
        }
    }

    pub fn upsert_usage_metric(&mut self, metric: PetUsageMetric) {
        let key = limit_metric_id(&metric.id);
        self.usage_metrics
            .retain(|item| limit_metric_id(&item.id) != key);
        self.usage_metrics.insert(0, metric.clone());
        self.usage_metrics.truncate(MAX_USAGE_METRICS);
        self.show_usage_metric(&metric);
    }

    pub fn usage_metrics_payload(&self) -> Vec<serde_json::Value> {
        self.usage_metrics
            .iter()
            .map(usage_metric_payload)
            .collect()
    }

    pub async fn load_pet(&mut self, pet_id: &str) -> Result<PetConfig, crate::pet::PetError> {
        let config = load_pet_config(pet_id).await?;

        // Load custom message map if present
        if !config.message_map.is_empty() {
            self.message_map = config.message_map.clone();
        }

        self.loaded_pet = Some(config.clone());
        self.current_state = PetState::Idle;

        // Notify frontend of pet change
        if let Some(window) = self.app_handle.get_webview_window("pet") {
            let _ = window.emit(
                "pet-loaded",
                serde_json::to_value(&config).unwrap_or_default(),
            );
        }
        // Also emit state reset
        if let Some(window) = self.app_handle.get_webview_window("pet") {
            let _ = window.emit("state-changed", "idle");
        }

        Ok(config)
    }

    pub fn handle_message(&mut self, message_type: &str) -> PetState {
        // Look up state mapping
        let target_state = self
            .message_map
            .get(message_type)
            .cloned()
            .unwrap_or_else(|| "idle".to_string());

        match target_state.parse::<PetState>() {
            Ok(new_state) => {
                if new_state != self.current_state {
                    self.current_state = new_state;

                    // Notify frontend of state change
                    if let Some(window) = self.app_handle.get_webview_window("pet") {
                        let _ = window.emit("state-changed", self.current_state.to_string());
                    }
                }
                new_state
            }
            Err(_) => {
                log::warn!("Unknown state mapping for message: {}", message_type);
                self.current_state
            }
        }
    }

    pub fn handle_websocket_message(&mut self, message_type: &str) -> Option<PetState> {
        if !self.websocket_enabled {
            return None;
        }

        Some(self.handle_message(message_type))
    }

    pub fn get_spritesheet_path(&self) -> Option<String> {
        self.loaded_pet.as_ref().map(|p| p.spritesheet_path.clone())
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn set_language(&mut self, language: String) -> Result<(), String> {
        self.language = normalize_language(&language).to_string();
        self.persist_settings()
    }

    fn current_settings(&self) -> AppSettings {
        AppSettings {
            live_source_paths: self.live_source_paths.clone(),
            live_source_focus_targets: self.live_source_focus_targets.clone(),
            live_source_enabled: HashMap::from([
                ("codex".to_string(), self.codex_monitor_enabled),
                ("claude".to_string(), self.claude_monitor_enabled),
                ("opencode".to_string(), self.opencode_monitor_enabled),
                ("openclaw".to_string(), self.openclaw_monitor_enabled),
                ("hermes".to_string(), self.hermes_monitor_enabled),
                ("antigravity".to_string(), self.antigravity_monitor_enabled),
            ]),
            live_source_prefix_enabled: self.live_source_prefix_enabled,
            language: self.language.clone(),
        }
    }

    fn persist_settings(&self) -> Result<(), String> {
        save_settings(&self.current_settings())
    }
}

fn normalize_language(language: &str) -> &str {
    match language {
        "zh-CN" => "zh-CN",
        _ => "en",
    }
}

pub fn default_live_source_paths() -> HashMap<String, String> {
    HashMap::from([
        ("codex".to_string(), home_path([".codex", "sessions"])),
        ("claude".to_string(), home_path([".claude", "projects"])),
        ("opencode".to_string(), default_opencode_path()),
        ("openclaw".to_string(), home_path([".openclaw", "agents"])),
        ("hermes".to_string(), home_path([".hermes", "sessions"])),
        (
            "antigravity".to_string(),
            home_path([".gemini", "antigravity"]),
        ),
    ])
}

pub fn default_live_source_focus_targets() -> HashMap<String, String> {
    HashMap::from([
        ("codex".to_string(), "app:Codex".to_string()),
        ("claude".to_string(), "app:Claude".to_string()),
        ("opencode".to_string(), "app:Terminal".to_string()),
        (
            "openclaw".to_string(),
            "url:http://127.0.0.1:18789/".to_string(),
        ),
        ("hermes".to_string(), "app:Terminal".to_string()),
        ("antigravity".to_string(), "app:Antigravity".to_string()),
    ])
}

fn default_opencode_path() -> String {
    #[cfg(windows)]
    {
        return platform_data_path(["opencode", "opencode.db"]);
    }

    #[cfg(not(windows))]
    {
        home_path([".local", "share", "opencode", "opencode.db"])
    }
}

fn home_path<const N: usize>(parts: [&str; N]) -> String {
    path_from_base(dirs::home_dir(), parts)
}

#[cfg(windows)]
fn platform_data_path<const N: usize>(parts: [&str; N]) -> String {
    path_from_base(dirs::data_local_dir().or_else(dirs::home_dir), parts)
}

fn path_from_base<const N: usize>(base: Option<PathBuf>, parts: [&str; N]) -> String {
    let Some(mut path) = base else {
        return parts.join("/");
    };

    for part in parts {
        path.push(part);
    }

    path.to_string_lossy().to_string()
}

fn live_source_paths_from_settings(settings: &AppSettings) -> HashMap<String, String> {
    let mut paths = default_live_source_paths();

    for source in known_live_sources() {
        if let Some(path) = settings.live_source_paths.get(source) {
            let path = path.trim();
            if !path.is_empty() {
                paths.insert(source.to_string(), path.to_string());
            }
        }
    }

    paths
}

fn live_source_focus_targets_from_settings(settings: &AppSettings) -> HashMap<String, String> {
    let mut targets = default_live_source_focus_targets();

    for source in known_live_sources() {
        if let Some(target) = settings.live_source_focus_targets.get(source) {
            let target = target.trim();
            if !target.is_empty() && validate_focus_target(target).is_ok() {
                targets.insert(source.to_string(), target.to_string());
            }
        }
    }

    targets
}

fn live_source_enabled_from_settings(settings: &AppSettings, source: &str) -> bool {
    settings
        .live_source_enabled
        .get(source)
        .copied()
        .unwrap_or(source == "codex")
}

fn is_known_live_source(source: &str) -> bool {
    known_live_sources().contains(&source)
}

fn known_live_sources() -> [&'static str; 6] {
    [
        "codex",
        "claude",
        "opencode",
        "openclaw",
        "hermes",
        "antigravity",
    ]
}

fn validate_focus_target(target: &str) -> Result<(), String> {
    let target = target.trim();
    if let Some(name) = target.strip_prefix("app:") {
        if !name.trim().is_empty() {
            return Ok(());
        }
    }

    if let Some(url) = target.strip_prefix("url:") {
        let url = url.trim();
        if url.starts_with("http://") || url.starts_with("https://") {
            return Ok(());
        }
    }

    Err("Focus target must be app:<name> or url:<http(s)-url>".to_string())
}

fn source_label(source: &str) -> &str {
    match source {
        "codex" => "Codex",
        "claude" => "Claude Code",
        "opencode" => "opencode",
        "openclaw" => "OpenClaw",
        "hermes" => "Hermes Agent",
        "antigravity" => "Antigravity",
        _ => source,
    }
}

fn codex_bubble_payload(text: &str, source: &str) -> serde_json::Value {
    serde_json::json!({
        "text": text,
        "source": source,
        "sourceLabel": source_label(source),
    })
}

fn notice_payload(notice: &PetNotice) -> serde_json::Value {
    let source_label = notice
        .source_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .map(str::trim)
        .map(limit_text)
        .unwrap_or_else(|| source_label(&notice.source).to_string());
    let group_key = limit_notice_key(&notice.group_key);
    let id = limit_notice_key(&notice.id);
    let fallback_id = if !group_key.is_empty() {
        group_key.clone()
    } else if !id.is_empty() {
        id.clone()
    } else {
        format!(
            "{}:{}",
            notice.source.trim(),
            limit_text(&notice.title).to_ascii_lowercase()
        )
    };

    serde_json::json!({
        "id": if id.is_empty() { fallback_id } else { id },
        "groupKey": group_key,
        "level": normalize_notice_level(&notice.level),
        "title": limit_text(&notice.title),
        "body": limit_notice_body(&notice.body),
        "source": notice.source.trim(),
        "sourceLabel": source_label,
        "noticeType": normalize_notice_type(&notice.notice_type),
        "actionHint": notice.action_hint.as_deref().map(limit_text),
        "actionLabel": notice.action_label.as_deref().map(limit_text),
        "focusSource": notice.focus_source,
        "actionKind": notice.action_kind.as_deref().map(normalize_notice_action_kind),
        "automationSafe": notice.automation_safe,
        "ttlSeconds": notice.ttl_seconds.clamp(15, 3600),
        "timestamp": notice.timestamp,
    })
}

fn usage_metric_payload(metric: &PetUsageMetric) -> serde_json::Value {
    let source_label = metric
        .source_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .map(str::trim)
        .map(limit_text)
        .unwrap_or_else(|| source_label(&metric.source).to_string());

    serde_json::json!({
        "id": limit_metric_id(&metric.id),
        "source": metric.source.trim(),
        "sourceLabel": source_label,
        "label": limit_text(&metric.label),
        "value": limit_text(&metric.value),
        "detail": limit_text(&metric.detail),
        "percent": metric.percent.map(|percent| percent.clamp(0.0, 100.0)),
        "status": normalize_notice_level(&metric.status),
        "meta": metric.meta,
        "timestamp": metric.timestamp,
    })
}

fn limit_metric_id(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "usage".to_string()
    } else {
        limit_text(trimmed)
    }
}

fn limit_notice_key(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        limit_text(trimmed)
    }
}

fn normalize_notice_level(level: &str) -> &str {
    match level.trim().to_ascii_lowercase().as_str() {
        "success" => "success",
        "warning" | "warn" => "warning",
        "error" | "danger" => "error",
        _ => "info",
    }
}

fn normalize_notice_type(notice_type: &str) -> &str {
    match notice_type.trim().to_ascii_lowercase().as_str() {
        "approval_required" => "approval_required",
        "confirm_required" => "confirm_required",
        "press_enter_required" => "press_enter_required",
        "context_compacting" => "context_compacting",
        "task_failed" => "task_failed",
        "info" => "info",
        _ => "info",
    }
}

fn normalize_notice_action_kind(action_kind: &str) -> &str {
    match action_kind.trim().to_ascii_lowercase().as_str() {
        "focus" => "focus",
        "press_enter" => "press_enter",
        "type_yes_enter" => "type_yes_enter",
        "select_allow" => "select_allow",
        _ => "focus",
    }
}

fn limit_text(text: &str) -> String {
    const MAX_CHARS: usize = 56;
    let text = text.trim();
    let mut chars = text.chars();
    let shortened: String = chars.by_ref().take(MAX_CHARS).collect();

    if chars.next().is_some() {
        format!("{}...", shortened.trim_end())
    } else if shortened.is_empty() {
        "Notice".to_string()
    } else {
        shortened
    }
}

fn limit_notice_body(text: &str) -> String {
    const MAX_CHARS: usize = 140;
    let text = text.trim();
    let mut chars = text.chars();
    let shortened: String = chars.by_ref().take(MAX_CHARS).collect();

    if chars.next().is_some() {
        format!("{}...", shortened.trim_end())
    } else {
        shortened
    }
}

#[cfg(test)]
mod focus_target_tests {
    use super::*;

    #[test]
    fn default_focus_targets_cover_known_sources() {
        let targets = default_live_source_focus_targets();
        for source in known_live_sources() {
            assert!(targets.contains_key(source), "missing target for {source}");
            assert!(validate_focus_target(targets.get(source).unwrap()).is_ok());
        }
    }

    #[test]
    fn live_source_focus_targets_from_settings_uses_defaults() {
        let settings = AppSettings::default();
        assert_eq!(
            live_source_focus_targets_from_settings(&settings),
            default_live_source_focus_targets()
        );
    }

    #[test]
    fn live_source_focus_targets_from_settings_accepts_override() {
        let mut settings = AppSettings::default();
        settings
            .live_source_focus_targets
            .insert("codex".to_string(), "app:Warp".to_string());
        let targets = live_source_focus_targets_from_settings(&settings);
        assert_eq!(targets.get("codex"), Some(&"app:Warp".to_string()));
    }

    #[test]
    fn live_source_focus_targets_from_settings_ignores_invalid_override() {
        let mut settings = AppSettings::default();
        settings
            .live_source_focus_targets
            .insert("codex".to_string(), "file:/tmp/nope".to_string());
        let targets = live_source_focus_targets_from_settings(&settings);
        assert_eq!(targets.get("codex"), Some(&"app:Codex".to_string()));
    }

    #[test]
    fn codex_bubble_payload_includes_source() {
        let payload = codex_bubble_payload("hello", "codex");
        assert_eq!(payload["text"], "hello");
        assert_eq!(payload["source"], "codex");
        assert_eq!(payload["sourceLabel"], "Codex");
    }

    #[test]
    fn notice_payload_normalizes_level_and_limits_ttl() {
        let notice = PetNotice {
            id: "claude-quota-low".to_string(),
            group_key: "claude-quota".to_string(),
            level: "warn".to_string(),
            title: "Claude quota low".to_string(),
            body: "18% remaining".to_string(),
            source: "claude".to_string(),
            source_label: None,
            notice_type: "approval_required".to_string(),
            action_hint: Some("Allow / Deny".to_string()),
            action_label: Some("Review".to_string()),
            focus_source: true,
            action_kind: Some("focus".to_string()),
            automation_safe: false,
            ttl_seconds: 2,
            timestamp: None,
        };
        let payload = notice_payload(&notice);
        assert_eq!(payload["id"], "claude-quota-low");
        assert_eq!(payload["groupKey"], "claude-quota");
        assert_eq!(payload["level"], "warning");
        assert_eq!(payload["sourceLabel"], "Claude Code");
        assert_eq!(payload["noticeType"], "approval_required");
        assert_eq!(payload["actionHint"], "Allow / Deny");
        assert_eq!(payload["actionLabel"], "Review");
        assert_eq!(payload["focusSource"], true);
        assert_eq!(payload["actionKind"], "focus");
        assert_eq!(payload["automationSafe"], false);
        assert_eq!(payload["ttlSeconds"], 15);
    }

    #[test]
    fn usage_metric_payload_clamps_percent() {
        let metric = PetUsageMetric {
            id: "claude-quota".to_string(),
            source: "claude".to_string(),
            source_label: Some("Claude".to_string()),
            label: "Quota".to_string(),
            value: "118%".to_string(),
            detail: "reset 2h".to_string(),
            percent: Some(118.0),
            status: "warn".to_string(),
            meta: serde_json::json!({"kind": "short_quota"}),
            timestamp: None,
        };
        let payload = usage_metric_payload(&metric);
        assert_eq!(payload["id"], "claude-quota");
        assert_eq!(payload["sourceLabel"], "Claude");
        assert_eq!(payload["percent"], 100.0);
        assert_eq!(payload["status"], "warning");
        assert_eq!(payload["meta"]["kind"], "short_quota");
    }

    #[test]
    fn live_source_enabled_defaults_to_only_codex() {
        let settings = AppSettings::default();
        assert!(live_source_enabled_from_settings(&settings, "codex"));
        for source in ["claude", "opencode", "openclaw", "hermes", "antigravity"] {
            assert!(!live_source_enabled_from_settings(&settings, source));
        }
    }

    #[test]
    fn default_live_source_paths_include_antigravity() {
        let paths = default_live_source_paths();
        assert!(paths
            .get("antigravity")
            .is_some_and(|path| path.ends_with(".gemini/antigravity")));
    }

    #[test]
    fn live_source_enabled_uses_saved_setting() {
        let mut settings = AppSettings::default();
        settings
            .live_source_enabled
            .insert("claude".to_string(), true);
        settings
            .live_source_enabled
            .insert("codex".to_string(), false);

        assert!(!live_source_enabled_from_settings(&settings, "codex"));
        assert!(live_source_enabled_from_settings(&settings, "claude"));
    }
}
