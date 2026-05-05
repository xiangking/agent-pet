// Message types for driving pet animations
// Maps external events to pet states

use serde::{Deserialize, Serialize};

/// Incoming message from external sources (WebSocket, HTTP, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PetMessage {
    /// Message type: new_message, mention, error, processing, etc.
    pub message_type: String,
    /// Optional payload data
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Source identifier (e.g., "slack", "discord", "webhook")
    #[serde(default)]
    pub source: String,
    /// Timestamp
    #[serde(default)]
    pub timestamp: Option<u64>,
}

/// Message type constants
pub const MSG_NEW_MESSAGE: &str = "new_message";
pub const MSG_MENTION: &str = "mention";
pub const MSG_ERROR: &str = "error";
pub const MSG_PROCESSING: &str = "processing";
pub const MSG_WAITING_INPUT: &str = "waiting_input";
pub const MSG_REVIEW_REQUIRED: &str = "review_required";
pub const MSG_SUCCESS: &str = "success";
pub const MSG_IDLE: &str = "idle";
pub const MSG_RUNNING: &str = "running";
pub const MSG_JUMPING: &str = "jumping";
pub const MSG_WAVING: &str = "waving";
pub const MSG_FAILED: &str = "failed";
pub const MSG_WAITING: &str = "waiting";
pub const MSG_REVIEW: &str = "review";

/// Default message map: message_type -> pet_state
///
/// This function creates a complete mapping from all supported message types
/// to their corresponding pet animation states.
pub fn default_message_map() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    map.insert(MSG_NEW_MESSAGE.to_string(), "waving".to_string());
    map.insert(MSG_MENTION.to_string(), "jumping".to_string());
    map.insert(MSG_ERROR.to_string(), "failed".to_string());
    map.insert(MSG_PROCESSING.to_string(), "running".to_string());
    map.insert(MSG_WAITING_INPUT.to_string(), "waiting".to_string());
    map.insert(MSG_REVIEW_REQUIRED.to_string(), "review".to_string());
    map.insert(MSG_SUCCESS.to_string(), "waving".to_string());
    map.insert(MSG_IDLE.to_string(), "idle".to_string());
    // Direct state triggers
    map.insert(MSG_RUNNING.to_string(), "running".to_string());
    map.insert(MSG_JUMPING.to_string(), "jumping".to_string());
    map.insert(MSG_WAVING.to_string(), "waving".to_string());
    map.insert(MSG_FAILED.to_string(), "failed".to_string());
    map.insert(MSG_WAITING.to_string(), "waiting".to_string());
    map.insert(MSG_REVIEW.to_string(), "review".to_string());
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PetMessage 序列化 / 反序列化 ──────────────────────────

    #[test]
    fn pet_message_roundtrip_full() {
        let msg = PetMessage {
            message_type: "new_message".to_string(),
            payload: serde_json::json!({"text": "hello"}),
            source: "slack".to_string(),
            timestamp: Some(1700000000),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: PetMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.message_type, "new_message");
        assert_eq!(parsed.source, "slack");
        assert_eq!(parsed.timestamp, Some(1700000000));
        assert_eq!(parsed.payload["text"], "hello");
    }

    #[test]
    fn pet_message_defaults_when_optional_fields_missing() {
        let json = r#"{"message_type": "error"}"#;
        let msg: PetMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, "error");
        assert_eq!(msg.source, "");
        assert_eq!(msg.payload, serde_json::Value::Null);
        assert_eq!(msg.timestamp, None);
    }

    #[test]
    fn pet_message_rejects_missing_message_type() {
        let json = r#"{"source": "webhook"}"#;
        let result = serde_json::from_str::<PetMessage>(json);
        assert!(result.is_err(), "message_type 是必填字段，缺失时应解析失败");
    }

    // ── 消息类型常量 ──────────────────────────────────────────

    #[test]
    fn message_type_constants_are_unique() {
        let constants = [
            MSG_NEW_MESSAGE,
            MSG_MENTION,
            MSG_ERROR,
            MSG_PROCESSING,
            MSG_WAITING_INPUT,
            MSG_REVIEW_REQUIRED,
            MSG_SUCCESS,
            MSG_IDLE,
            MSG_RUNNING,
            MSG_JUMPING,
            MSG_WAVING,
            MSG_FAILED,
            MSG_WAITING,
            MSG_REVIEW,
        ];
        let set: std::collections::HashSet<&str> = constants.iter().copied().collect();
        assert_eq!(set.len(), constants.len(), "存在重复的消息类型常量");
    }

    // ── default_message_map ───────────────────────────────────

    #[test]
    fn default_message_map_contains_all_constants() {
        let map = default_message_map();
        let expected_keys = [
            MSG_NEW_MESSAGE,
            MSG_MENTION,
            MSG_ERROR,
            MSG_PROCESSING,
            MSG_WAITING_INPUT,
            MSG_REVIEW_REQUIRED,
            MSG_SUCCESS,
            MSG_IDLE,
            MSG_RUNNING,
            MSG_JUMPING,
            MSG_WAVING,
            MSG_FAILED,
            MSG_WAITING,
            MSG_REVIEW,
        ];
        for key in &expected_keys {
            assert!(
                map.contains_key(*key),
                "default_message_map 缺少键: {}",
                key
            );
        }
        assert_eq!(
            map.len(),
            expected_keys.len(),
            "default_message_map 键数量不匹配"
        );
    }

    #[test]
    fn default_message_map_values_are_valid_states() {
        let map = default_message_map();
        let valid_states: std::collections::HashSet<&str> = [
            "idle",
            "running-right",
            "running-left",
            "waving",
            "jumping",
            "failed",
            "waiting",
            "running",
            "review",
        ]
        .iter()
        .copied()
        .collect();

        for (msg_type, state) in &map {
            assert!(
                valid_states.contains(state.as_str()),
                "消息 '{}' 映射到未知状态 '{}'",
                msg_type,
                state,
            );
        }
    }

    #[test]
    fn default_message_map_semantic_mappings() {
        let map = default_message_map();
        // 核心语义映射校验
        assert_eq!(map[MSG_NEW_MESSAGE], "waving");
        assert_eq!(map[MSG_MENTION], "jumping");
        assert_eq!(map[MSG_ERROR], "failed");
        assert_eq!(map[MSG_PROCESSING], "running");
        assert_eq!(map[MSG_WAITING_INPUT], "waiting");
        assert_eq!(map[MSG_REVIEW_REQUIRED], "review");
        assert_eq!(map[MSG_SUCCESS], "waving");
        assert_eq!(map[MSG_IDLE], "idle");
    }

    #[test]
    fn default_message_map_direct_triggers() {
        let map = default_message_map();
        // 直接触发：消息类型 == 状态名
        assert_eq!(map[MSG_RUNNING], "running");
        assert_eq!(map[MSG_JUMPING], "jumping");
        assert_eq!(map[MSG_WAVING], "waving");
        assert_eq!(map[MSG_FAILED], "failed");
        assert_eq!(map[MSG_WAITING], "waiting");
        assert_eq!(map[MSG_REVIEW], "review");
    }
}
