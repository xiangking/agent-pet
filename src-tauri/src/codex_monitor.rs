// Watches Codex session JSONL files and maps live Codex activity to pet messages.

use crate::message::{
    MSG_ERROR, MSG_MENTION, MSG_NEW_MESSAGE, MSG_PROCESSING, MSG_SUCCESS, MSG_WAITING_INPUT,
};
use crate::state_machine::PetStateMachine;
use serde_json::Value;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::async_runtime::Mutex;
use tokio_util::sync::CancellationToken;

struct CodexActivity {
    message_type: &'static str,
    bubble_text: Option<String>,
    origin: ActivityOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivityOrigin {
    User,
    Assistant,
}

pub struct CodexMonitor {
    state_machine: Arc<Mutex<PetStateMachine>>,
    cancellation_token: CancellationToken,
}

impl CodexMonitor {
    pub fn new(
        state_machine: Arc<Mutex<PetStateMachine>>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            state_machine,
            cancellation_token,
        }
    }

    pub async fn run(&self) {
        let mut codex_cursor = FileCursor::default();
        let mut claude_cursor = FileCursor::default();
        let mut opencode_cursor = OpencodeCursor::default();
        let mut openclaw_cursor = FileCursor::default();
        let mut hermes_cursor = FileCursor::default();
        let mut antigravity_conversation_cursor = BinaryFileCursor::default();
        let mut antigravity_brain_cursor = MetadataCursor::default();

        while !self.cancellation_token.is_cancelled() {
            let (
                codex_path,
                claude_path,
                opencode_path,
                openclaw_path,
                hermes_path,
                antigravity_path,
            ) = {
                let sm = self.state_machine.lock().await;
                (
                    enabled_source_path(&sm, "codex"),
                    enabled_source_path(&sm, "claude"),
                    enabled_source_path(&sm, "opencode"),
                    enabled_source_path(&sm, "openclaw"),
                    enabled_source_path(&sm, "hermes"),
                    enabled_source_path(&sm, "antigravity"),
                )
            };

            if let Some(sessions_dir) = codex_path.as_deref() {
                if let Err(e) = poll_jsonl_source(
                    sessions_dir,
                    &mut codex_cursor,
                    &self.state_machine,
                    "codex",
                    is_codex_session_file,
                    codex_line_to_activity,
                )
                .await
                {
                    log::warn!("Codex monitor poll failed: {}", e);
                }
            }

            if let Some(projects_dir) = claude_path.as_deref() {
                if let Err(e) = poll_jsonl_source(
                    projects_dir,
                    &mut claude_cursor,
                    &self.state_machine,
                    "claude",
                    is_jsonl_file,
                    claude_line_to_activity,
                )
                .await
                {
                    log::warn!("Claude monitor poll failed: {}", e);
                }
            }

            if let Some(db_path) = opencode_path.as_deref().filter(|path| path.exists()) {
                if let Err(e) =
                    poll_opencode_source(db_path, &mut opencode_cursor, &self.state_machine).await
                {
                    log::warn!("opencode monitor poll failed: {}", e);
                }
            }

            if let Some(sessions_root) = openclaw_path.as_deref() {
                if let Err(e) = poll_jsonl_source(
                    sessions_root,
                    &mut openclaw_cursor,
                    &self.state_machine,
                    "openclaw",
                    is_openclaw_session_file,
                    openclaw_line_to_activity,
                )
                .await
                {
                    log::warn!("OpenClaw monitor poll failed: {}", e);
                }
            }

            if let Some(sessions_root) = hermes_path.as_deref() {
                if let Err(e) = poll_jsonl_source(
                    sessions_root,
                    &mut hermes_cursor,
                    &self.state_machine,
                    "hermes",
                    is_hermes_session_file,
                    hermes_line_to_activity,
                )
                .await
                {
                    log::warn!("Hermes monitor poll failed: {}", e);
                }
            }

            if let Some(root) = antigravity_path.as_deref() {
                if let Err(e) = poll_antigravity_source(
                    root,
                    &mut antigravity_conversation_cursor,
                    &mut antigravity_brain_cursor,
                    &self.state_machine,
                )
                .await
                {
                    log::warn!("Antigravity monitor poll failed: {}", e);
                }
            }

            tokio::select! {
                _ = self.cancellation_token.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(700)) => {}
            }
        }
    }
}

fn enabled_source_path(sm: &PetStateMachine, source: &str) -> Option<PathBuf> {
    if sm.live_source_enabled(source) {
        expand_source_path(sm.live_source_path(source))
    } else {
        None
    }
}

#[derive(Default)]
struct FileCursor {
    path: Option<PathBuf>,
    offset: u64,
}

#[derive(Default)]
struct OpencodeCursor {
    last_time_created: i64,
    initialized: bool,
}

#[derive(Default)]
struct BinaryFileCursor {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    initialized: bool,
}

#[derive(Default)]
struct MetadataCursor {
    newest_modified: Option<SystemTime>,
    initialized: bool,
}

async fn poll_jsonl_source(
    root: &Path,
    cursor: &mut FileCursor,
    state_machine: &Arc<Mutex<PetStateMachine>>,
    source: &'static str,
    file_filter: fn(&Path) -> bool,
    parser: fn(&str) -> Option<CodexActivity>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = newest_jsonl_file(root, file_filter)? else {
        return Ok(());
    };

    if cursor.path.as_ref() != Some(&path) {
        let len = fs::metadata(&path)?.len();
        cursor.path = Some(path);
        cursor.offset = len;
        if let Some(activity) = latest_activity_in_file(cursor.path.as_ref().unwrap(), parser)? {
            emit_activity(state_machine, source, activity).await;
        }
        return Ok(());
    }

    let len = fs::metadata(&path)?.len();
    if len < cursor.offset {
        cursor.offset = 0;
    }
    if len == cursor.offset {
        return Ok(());
    }

    let mut file = File::open(&path)?;
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut appended = String::new();
    file.read_to_string(&mut appended)?;
    cursor.offset = len;

    for line in appended.lines() {
        if let Some(activity) = parser(line) {
            emit_activity(state_machine, source, activity).await;
        }
    }

    Ok(())
}

async fn emit_activity(
    state_machine: &Arc<Mutex<PetStateMachine>>,
    source: &str,
    activity: CodexActivity,
) {
    if activity.origin == ActivityOrigin::User {
        return;
    }

    let mut sm = state_machine.lock().await;
    if sm.live_source_enabled(source) {
        let prefix_enabled = sm.live_source_prefix_enabled();
        sm.handle_message(activity.message_type);
        if let Some(text) = activity.bubble_text {
            let bubble_text = if prefix_enabled {
                format_source_bubble_text(source, &text)
            } else {
                text
            };
            sm.show_codex_bubble(&bubble_text, source);
        }
    }
}

fn expand_source_path(path: Option<String>) -> Option<PathBuf> {
    let path = path?;
    let expanded = expand_env_vars(path.trim());
    let trimmed = expanded.trim();

    if trimmed.is_empty() {
        return None;
    }

    if trimmed == "~" {
        dirs::home_dir()
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        dirs::home_dir().map(|home| home.join(rest))
    } else {
        Some(PathBuf::from(trimmed))
    }
}

fn expand_env_vars(path: &str) -> String {
    let mut output = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '%' {
            let mut name = String::new();
            while let Some(&next) = chars.peek() {
                chars.next();
                if next == '%' {
                    break;
                }
                name.push(next);
            }

            if name.is_empty() {
                output.push(ch);
            } else if let Ok(value) = std::env::var(&name) {
                output.push_str(&value);
            } else {
                output.push('%');
                output.push_str(&name);
                output.push('%');
            }
            continue;
        }

        if ch == '$' {
            if chars.peek() == Some(&'{') {
                chars.next();
                let mut name = String::new();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == '}' {
                        break;
                    }
                    name.push(next);
                }

                if name.is_empty() {
                    output.push_str("${}");
                } else if let Ok(value) = std::env::var(&name) {
                    output.push_str(&value);
                } else {
                    output.push_str("${");
                    output.push_str(&name);
                    output.push('}');
                }
                continue;
            }

            let mut name = String::new();
            while let Some(&next) = chars.peek() {
                if !(next == '_' || next.is_ascii_alphanumeric()) {
                    break;
                }
                chars.next();
                name.push(next);
            }

            if name.is_empty() {
                output.push(ch);
            } else if let Ok(value) = std::env::var(&name) {
                output.push_str(&value);
            } else {
                output.push(ch);
                output.push_str(&name);
            }
            continue;
        }

        output.push(ch);
    }

    output
}

fn newest_jsonl_file(
    root: &Path,
    file_filter: fn(&Path) -> bool,
) -> Result<Option<PathBuf>, std::io::Error> {
    Ok(newest_file_with_modified(root, file_filter)?.map(|(path, _)| path))
}

fn newest_file(
    root: &Path,
    file_filter: fn(&Path) -> bool,
) -> Result<Option<PathBuf>, std::io::Error> {
    Ok(newest_file_with_modified(root, file_filter)?.map(|(path, _)| path))
}

fn newest_file_with_modified(
    root: &Path,
    file_filter: fn(&Path) -> bool,
) -> Result<Option<(PathBuf, SystemTime)>, std::io::Error> {
    if root.is_file() {
        return Ok(if file_filter(root) {
            Some((
                root.to_path_buf(),
                fs::metadata(root)?
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH),
            ))
        } else {
            None
        });
    }

    let mut newest: Option<(PathBuf, SystemTime)> = None;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            if !file_filter(&path) {
                continue;
            }

            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);

            if newest
                .as_ref()
                .map(|(_, current)| modified > *current)
                .unwrap_or(true)
            {
                newest = Some((path, modified));
            }
        }
    }

    Ok(newest)
}

fn is_codex_session_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with("rollout-"))
            .unwrap_or(false)
}

fn is_jsonl_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
}

fn is_openclaw_session_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        && path
            .components()
            .any(|component| component.as_os_str() == "sessions")
}

fn is_hermes_session_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
}

fn is_antigravity_conversation_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("pb")
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("conversations")
}

fn is_antigravity_brain_metadata_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    matches!(
        name,
        "task.md.metadata.json"
            | "implementation_plan.md.metadata.json"
            | "audit_report.md.metadata.json"
            | "code_review.md.metadata.json"
            | "changes_diff.md.metadata.json"
    ) && path
        .components()
        .any(|component| component.as_os_str() == "brain")
}

#[cfg(test)]
fn codex_line_to_message_type(line: &str) -> Option<&'static str> {
    codex_line_to_activity(line).map(|activity| activity.message_type)
}

fn codex_line_to_activity(line: &str) -> Option<CodexActivity> {
    let value: Value = serde_json::from_str(line).ok()?;
    let record_type = value.get("type")?.as_str()?;
    let payload = value.get("payload");

    match record_type {
        "event_msg" => event_payload_to_activity(payload?),
        "response_item" => response_payload_to_activity(payload?),
        _ => None,
    }
}

fn latest_activity_in_file(
    path: &Path,
    parser: fn(&str) -> Option<CodexActivity>,
) -> Result<Option<CodexActivity>, std::io::Error> {
    const TAIL_BYTES: u64 = 64 * 1024;

    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;

    let mut tail = String::new();
    file.read_to_string(&mut tail)?;
    let lines = if start > 0 {
        tail.lines().skip(1).collect::<Vec<_>>()
    } else {
        tail.lines().collect::<Vec<_>>()
    };

    Ok(lines.into_iter().rev().find_map(parser))
}

fn event_payload_to_activity(payload: &Value) -> Option<CodexActivity> {
    let event_type = payload.get("type")?.as_str()?;

    if event_type.contains("approval")
        || event_type.contains("permission")
        || event_type.contains("input")
    {
        return Some(activity(MSG_WAITING_INPUT, "Needs your input"));
    }

    match event_type {
        "user_message" => Some(CodexActivity {
            message_type: MSG_MENTION,
            bubble_text: payload_message_text(payload).or_else(|| Some("New request".to_string())),
            origin: ActivityOrigin::User,
        }),
        "task_started" => Some(activity(MSG_PROCESSING, "Working...")),
        "agent_message" => Some(CodexActivity {
            message_type: MSG_NEW_MESSAGE,
            bubble_text: payload_message_text(payload)
                .or_else(|| payload_last_agent_message(payload))
                .or_else(|| Some("Codex replied".to_string())),
            origin: ActivityOrigin::Assistant,
        }),
        "task_complete" => Some(CodexActivity {
            message_type: MSG_SUCCESS,
            bubble_text: payload_last_agent_message(payload).or_else(|| Some("Done".to_string())),
            origin: ActivityOrigin::Assistant,
        }),
        "exec_command_end" | "patch_apply_end" => {
            match payload.get("status").and_then(Value::as_str) {
                Some("failed") | Some("error") => Some(activity(MSG_ERROR, "Something failed")),
                _ => Some(activity(MSG_PROCESSING, "Updated")),
            }
        }
        _ => None,
    }
}

fn response_payload_to_activity(payload: &Value) -> Option<CodexActivity> {
    match payload.get("type")?.as_str()? {
        "reasoning"
        | "function_call"
        | "function_call_output"
        | "custom_tool_call"
        | "custom_tool_call_output" => Some(activity(MSG_PROCESSING, "Working...")),
        "message" => match payload.get("role").and_then(Value::as_str) {
            Some("assistant") => Some(CodexActivity {
                message_type: MSG_NEW_MESSAGE,
                bubble_text: response_message_text(payload)
                    .or_else(|| payload_last_agent_message(payload))
                    .or_else(|| Some("Codex replied".to_string())),
                origin: ActivityOrigin::Assistant,
            }),
            Some("user") => Some(CodexActivity {
                message_type: MSG_MENTION,
                bubble_text: response_message_text(payload)
                    .or_else(|| Some("New request".to_string())),
                origin: ActivityOrigin::User,
            }),
            _ => None,
        },
        _ => None,
    }
}

fn claude_line_to_activity(line: &str) -> Option<CodexActivity> {
    let value: Value = serde_json::from_str(line).ok()?;
    let event_type = value.get("type")?.as_str()?;

    match event_type {
        "user" => Some(CodexActivity {
            message_type: MSG_MENTION,
            bubble_text: claude_message_text(&value).or_else(|| Some("New request".to_string())),
            origin: ActivityOrigin::User,
        }),
        "assistant" => {
            if value.get("error").is_some()
                || value.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true)
            {
                return Some(CodexActivity {
                    message_type: MSG_ERROR,
                    bubble_text: claude_message_text(&value)
                        .or_else(|| Some("Something failed".to_string())),
                    origin: ActivityOrigin::Assistant,
                });
            }

            let message = value.get("message")?;
            if message_has_tool_use(message) {
                Some(activity(MSG_PROCESSING, "Working..."))
            } else {
                Some(CodexActivity {
                    message_type: MSG_NEW_MESSAGE,
                    bubble_text: claude_message_text(&value)
                        .or_else(|| Some("Replied".to_string())),
                    origin: ActivityOrigin::Assistant,
                })
            }
        }
        "progress" => Some(activity(MSG_PROCESSING, "Working...")),
        "system" => match value.get("subtype").and_then(Value::as_str) {
            Some("turn_duration") | Some("stop_hook_summary") => {
                Some(activity(MSG_SUCCESS, "Done"))
            }
            _ => None,
        },
        _ => None,
    }
}

fn openclaw_line_to_activity(line: &str) -> Option<CodexActivity> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "message" {
        return None;
    }

    let message = value.get("message")?;
    match message.get("role").and_then(Value::as_str) {
        Some("user") => Some(CodexActivity {
            message_type: MSG_MENTION,
            bubble_text: openclaw_message_text(message).or_else(|| Some("New request".to_string())),
            origin: ActivityOrigin::User,
        }),
        Some("assistant") => {
            let text = openclaw_message_text(message);
            let has_tool_call = message_has_openclaw_tool_call(message);

            if let Some(text) = text {
                Some(CodexActivity {
                    message_type: MSG_NEW_MESSAGE,
                    bubble_text: Some(text),
                    origin: ActivityOrigin::Assistant,
                })
            } else if message.get("stopReason").and_then(Value::as_str) == Some("error") {
                Some(activity(MSG_ERROR, "Something failed"))
            } else if has_tool_call {
                Some(activity(MSG_PROCESSING, "Working..."))
            } else {
                Some(activity(MSG_SUCCESS, "Done"))
            }
        }
        _ => None,
    }
}

fn hermes_line_to_activity(line: &str) -> Option<CodexActivity> {
    let value: Value = serde_json::from_str(line).ok()?;
    let record_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message");

    match record_type {
        "session" | "model_change" | "thinking_level_change" | "custom" => None,
        "message" => {
            let message = value.get("message").unwrap_or(&value);
            match message.get("role").and_then(Value::as_str) {
                Some("user") => Some(CodexActivity {
                    message_type: MSG_MENTION,
                    bubble_text: hermes_message_text(message)
                        .or_else(|| Some("New request".to_string())),
                    origin: ActivityOrigin::User,
                }),
                Some("assistant") => {
                    if message.get("model").and_then(Value::as_str) == Some("delivery-mirror")
                        || message.get("provider").and_then(Value::as_str) == Some("openclaw")
                            && message.get("model").and_then(Value::as_str)
                                == Some("delivery-mirror")
                    {
                        return None;
                    }

                    let text = hermes_message_text(message);
                    let has_tool_call = message_has_openclaw_tool_call(message);

                    if let Some(text) = text {
                        Some(CodexActivity {
                            message_type: MSG_NEW_MESSAGE,
                            bubble_text: Some(text),
                            origin: ActivityOrigin::Assistant,
                        })
                    } else if message.get("stopReason").and_then(Value::as_str) == Some("error") {
                        Some(activity(MSG_ERROR, "Something failed"))
                    } else if has_tool_call {
                        Some(activity(MSG_PROCESSING, "Working..."))
                    } else {
                        Some(activity(MSG_SUCCESS, "Done"))
                    }
                }
                Some("tool") => Some(activity(MSG_PROCESSING, "Using tool...")),
                _ => None,
            }
        }
        _ => None,
    }
}

async fn poll_antigravity_source(
    root: &Path,
    conversation_cursor: &mut BinaryFileCursor,
    brain_cursor: &mut MetadataCursor,
    state_machine: &Arc<Mutex<PetStateMachine>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let conversations_root = root.join("conversations");
    if conversations_root.exists() {
        poll_antigravity_conversation(&conversations_root, conversation_cursor, state_machine)
            .await?;
    }

    let brain_root = root.join("brain");
    if brain_root.exists() {
        poll_antigravity_brain_metadata(&brain_root, brain_cursor, state_machine).await?;
    }

    Ok(())
}

async fn poll_antigravity_conversation(
    root: &Path,
    cursor: &mut BinaryFileCursor,
    state_machine: &Arc<Mutex<PetStateMachine>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = newest_file(root, is_antigravity_conversation_file)? else {
        return Ok(());
    };
    let modified = fs::metadata(&path)?
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH);

    if !cursor.initialized {
        cursor.path = Some(path);
        cursor.modified = Some(modified);
        cursor.initialized = true;
        return Ok(());
    }

    if cursor.path.as_ref() == Some(&path) && cursor.modified == Some(modified) {
        return Ok(());
    }

    cursor.path = Some(path.clone());
    cursor.modified = Some(modified);

    let data = fs::read(&path)?;
    let activity = antigravity_blob_to_activity(&data)
        .or_else(|| Some(activity(MSG_PROCESSING, "Working...")));
    if let Some(activity) = activity {
        emit_activity(state_machine, "antigravity", activity).await;
    }

    Ok(())
}

async fn poll_antigravity_brain_metadata(
    root: &Path,
    cursor: &mut MetadataCursor,
    state_machine: &Arc<Mutex<PetStateMachine>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some((path, modified)) =
        newest_file_with_modified(root, is_antigravity_brain_metadata_file)?
    else {
        return Ok(());
    };

    if !cursor.initialized {
        cursor.newest_modified = Some(modified);
        cursor.initialized = true;
        return Ok(());
    }

    if cursor
        .newest_modified
        .map(|current| modified <= current)
        .unwrap_or(false)
    {
        return Ok(());
    }

    cursor.newest_modified = Some(modified);

    if let Some(activity) = antigravity_metadata_to_activity(&path) {
        emit_activity(state_machine, "antigravity", activity).await;
    }

    Ok(())
}

async fn poll_opencode_source(
    db_path: &Path,
    cursor: &mut OpencodeCursor,
    state_machine: &Arc<Mutex<PetStateMachine>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let rows = if cursor.initialized {
        query_opencode_rows(db_path, Some(cursor.last_time_created))?
    } else {
        let rows = query_opencode_latest_row(db_path)?;
        cursor.initialized = true;
        rows
    };

    for row in rows {
        cursor.last_time_created = cursor.last_time_created.max(row.time_created);
        if let Some(activity) = opencode_row_to_activity(&row) {
            emit_activity(state_machine, "opencode", activity).await;
        }
    }

    Ok(())
}

struct OpencodeRow {
    time_created: i64,
    role: Option<String>,
    part_type: Option<String>,
    text: Option<String>,
    data: Value,
}

fn query_opencode_rows(
    db_path: &Path,
    after_time_created: Option<i64>,
) -> Result<Vec<OpencodeRow>, Box<dyn std::error::Error>> {
    let condition = after_time_created
        .map(|time| format!("where p.time_created > {}", time))
        .unwrap_or_default();
    let query = format!(
        "select p.time_created as time_created, json_extract(m.data,'$.role') as role, json_extract(p.data,'$.type') as part_type, json_extract(p.data,'$.text') as text, p.data as data from part p join message m on m.id = p.message_id {} order by p.time_created asc limit 50;",
        condition
    );

    run_opencode_query(db_path, &query)
}

fn query_opencode_latest_row(
    db_path: &Path,
) -> Result<Vec<OpencodeRow>, Box<dyn std::error::Error>> {
    run_opencode_query(
        db_path,
        "select p.time_created as time_created, json_extract(m.data,'$.role') as role, json_extract(p.data,'$.type') as part_type, json_extract(p.data,'$.text') as text, p.data as data from part p join message m on m.id = p.message_id where json_extract(m.data,'$.role') = 'assistant' and json_extract(p.data,'$.type') = 'text' and coalesce(json_extract(p.data,'$.text'),'') <> '' order by p.time_created desc limit 1;",
    )
}

fn run_opencode_query(
    db_path: &Path,
    query: &str,
) -> Result<Vec<OpencodeRow>, Box<dyn std::error::Error>> {
    let output = Command::new("/usr/bin/sqlite3")
        .arg("-json")
        .arg(db_path)
        .arg(query)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("sqlite3 failed: {}", stderr).into());
    }

    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }

    let value: Value = serde_json::from_slice(&output.stdout)?;
    let rows = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let data = row
                .get("data")
                .and_then(Value::as_str)
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())?;
            Some(OpencodeRow {
                time_created: row.get("time_created")?.as_i64()?,
                role: row.get("role").and_then(Value::as_str).map(str::to_string),
                part_type: row
                    .get("part_type")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                text: row.get("text").and_then(Value::as_str).map(str::to_string),
                data,
            })
        })
        .collect();

    Ok(rows)
}

fn opencode_row_to_activity(row: &OpencodeRow) -> Option<CodexActivity> {
    let part_type = row.part_type.as_deref()?;

    match part_type {
        "text" => {
            let text = row.text.as_deref()?.trim();
            if text.is_empty() {
                return None;
            }

            match row.role.as_deref() {
                Some("user") => Some(CodexActivity {
                    message_type: MSG_MENTION,
                    bubble_text: Some(text.to_string()),
                    origin: ActivityOrigin::User,
                }),
                Some("assistant") => Some(CodexActivity {
                    message_type: MSG_NEW_MESSAGE,
                    bubble_text: Some(text.to_string()),
                    origin: ActivityOrigin::Assistant,
                }),
                _ => None,
            }
        }
        "reasoning" | "step-start" => Some(activity(MSG_PROCESSING, "Working...")),
        "step-finish" => match row.data.get("reason").and_then(Value::as_str) {
            Some("stop") => Some(activity(MSG_SUCCESS, "Done")),
            _ => Some(activity(MSG_PROCESSING, "Working...")),
        },
        "tool" => {
            let state = row.data.get("state")?;
            match state.get("status").and_then(Value::as_str) {
                Some("error") | Some("failed") => Some(activity(MSG_ERROR, "Tool failed")),
                _ => Some(activity(MSG_PROCESSING, "Using tool...")),
            }
        }
        _ => None,
    }
}

fn antigravity_blob_to_activity(data: &[u8]) -> Option<CodexActivity> {
    let fragments = printable_fragments(data);
    antigravity_fragments_to_activity(&fragments)
}

fn antigravity_fragments_to_activity(fragments: &[String]) -> Option<CodexActivity> {
    let lowered = fragments
        .iter()
        .map(|fragment| fragment.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let has_user_marker = lowered.iter().any(|fragment| {
        fragment.contains("user")
            || fragment.contains("human")
            || fragment.contains("prompt")
            || fragment.contains("request")
    });
    let has_assistant_marker = lowered.iter().any(|fragment| {
        fragment.contains("assistant") || fragment.contains("agent") || fragment.contains("model")
    });

    if lowered.iter().any(|fragment| {
        fragment.contains("error")
            || fragment.contains("failed")
            || fragment.contains("failure")
            || fragment.contains("exception")
    }) {
        return Some(CodexActivity {
            message_type: MSG_ERROR,
            bubble_text: Some("Something failed".to_string()),
            origin: ActivityOrigin::Assistant,
        });
    }

    if has_user_marker && !has_assistant_marker {
        return Some(CodexActivity {
            message_type: MSG_MENTION,
            bubble_text: antigravity_candidate_text(fragments)
                .or_else(|| Some("New request".to_string())),
            origin: ActivityOrigin::User,
        });
    }

    if has_assistant_marker
        || lowered.iter().any(|fragment| {
            fragment.contains("tool")
                || fragment.contains("thinking")
                || fragment.contains("running")
        })
    {
        return Some(activity(MSG_PROCESSING, "Working..."));
    }

    None
}

fn antigravity_candidate_text(fragments: &[String]) -> Option<String> {
    fragments
        .iter()
        .filter_map(|fragment| {
            let text = clean_antigravity_fragment(fragment);
            if is_antigravity_text_candidate(&text) {
                Some(text)
            } else {
                None
            }
        })
        .max_by_key(|text| text.len())
        .map(|text| truncate_bubble_text(text.trim()))
}

fn clean_antigravity_fragment(fragment: &str) -> String {
    let text = fragment
        .replace("\\n", " ")
        .replace("\\t", " ")
        .replace('\n', " ");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_antigravity_text_candidate(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 12 {
        return false;
    }

    let lowered = trimmed.to_ascii_lowercase();
    if lowered.ends_with(".md.metadata.json")
        || lowered.ends_with(".pb")
        || lowered.starts_with("uuid")
        || lowered.starts_with("file://")
        || lowered.contains("/.gemini/antigravity/")
    {
        return false;
    }

    trimmed.chars().any(|ch| ch.is_whitespace()) || trimmed.contains(['.', ',', ':', ';'])
}

fn printable_fragments(data: &[u8]) -> Vec<String> {
    let mut fragments = Vec::new();
    let mut current = Vec::new();

    for &byte in data {
        if byte.is_ascii_graphic() || byte == b' ' || byte == b'\n' || byte == b'\t' {
            current.push(byte);
            continue;
        }

        push_printable_fragment(&mut fragments, &mut current);
    }

    push_printable_fragment(&mut fragments, &mut current);
    fragments
}

fn push_printable_fragment(fragments: &mut Vec<String>, current: &mut Vec<u8>) {
    if current.len() >= 4 {
        if let Ok(text) = String::from_utf8(std::mem::take(current)) {
            fragments.push(text);
            return;
        }
    }

    current.clear();
}

fn antigravity_metadata_to_activity(path: &Path) -> Option<CodexActivity> {
    match path.file_name().and_then(|name| name.to_str())? {
        "implementation_plan.md.metadata.json"
        | "audit_report.md.metadata.json"
        | "code_review.md.metadata.json"
        | "changes_diff.md.metadata.json" => Some(activity(MSG_SUCCESS, "Updated")),
        "task.md.metadata.json" => Some(activity(MSG_PROCESSING, "Working...")),
        _ => None,
    }
}

fn activity(message_type: &'static str, bubble_text: &str) -> CodexActivity {
    CodexActivity {
        message_type,
        bubble_text: Some(bubble_text.to_string()),
        origin: ActivityOrigin::Assistant,
    }
}

fn claude_message_text(value: &Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    content_value_to_text(content).map(|text| truncate_bubble_text(text.trim()))
}

fn content_value_to_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("content").and_then(Value::as_str))
                })
                .collect::<Vec<_>>()
                .join(" ");
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    }
}

fn message_has_tool_use(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        })
        .unwrap_or(false)
}

fn message_has_openclaw_tool_call(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("toolCall" | "toolResult")
                )
            })
        })
        .unwrap_or(false)
}

fn format_source_bubble_text(source: &str, text: &str) -> String {
    truncate_bubble_text(&format!("{}: {}", source_label(source), text.trim()))
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

fn payload_message_text(payload: &Value) -> Option<String> {
    payload
        .get("message")
        .and_then(message_value_to_text)
        .map(|text| truncate_bubble_text(text.trim()))
}

fn payload_last_agent_message(payload: &Value) -> Option<String> {
    payload
        .get("last_agent_message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(truncate_bubble_text)
}

fn response_message_text(payload: &Value) -> Option<String> {
    let content = payload.get("content")?;
    message_value_to_text(content).map(|text| truncate_bubble_text(text.trim()))
}

fn openclaw_message_text(message: &Value) -> Option<String> {
    message
        .get("content")
        .and_then(|content| content_value_to_text(content))
        .map(|text| truncate_bubble_text(text.trim()))
}

fn hermes_message_text(message: &Value) -> Option<String> {
    message
        .get("content")
        .and_then(|content| content_value_to_text(content))
        .map(|text| truncate_bubble_text(text.trim()))
}

fn message_value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("content").and_then(Value::as_str))
                })
                .collect::<Vec<_>>()
                .join(" ");
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        }
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| map.get("content").and_then(message_value_to_text)),
        _ => None,
    }
}

fn truncate_bubble_text(text: &str) -> String {
    const MAX_CHARS: usize = 92;
    let mut chars = text.chars();
    let shortened: String = chars.by_ref().take(MAX_CHARS).collect();

    if chars.next().is_some() {
        format!("{}...", shortened.trim_end())
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_codex_task_started_to_processing() {
        let line = r#"{"type":"event_msg","payload":{"type":"task_started"}}"#;
        assert_eq!(codex_line_to_message_type(line), Some(MSG_PROCESSING));
    }

    #[test]
    fn maps_codex_failed_command_to_error() {
        let line =
            r#"{"type":"event_msg","payload":{"type":"exec_command_end","status":"failed"}}"#;
        assert_eq!(codex_line_to_message_type(line), Some(MSG_ERROR));
    }

    #[test]
    fn maps_codex_task_complete_to_success() {
        let line = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        assert_eq!(codex_line_to_message_type(line), Some(MSG_SUCCESS));
    }

    #[test]
    fn extracts_assistant_message_text_for_bubble() {
        let line = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello from Codex"}]}}"#;
        let activity = codex_line_to_activity(line).unwrap();
        assert_eq!(activity.message_type, MSG_NEW_MESSAGE);
        assert_eq!(activity.bubble_text.as_deref(), Some("Hello from Codex"));
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
    }

    #[test]
    fn extracts_user_message_text_for_bubble() {
        let line = r#"{"type":"event_msg","payload":{"type":"user_message","message":"Use the current message"}}"#;
        let activity = codex_line_to_activity(line).unwrap();
        assert_eq!(activity.message_type, MSG_MENTION);
        assert_eq!(
            activity.bubble_text.as_deref(),
            Some("Use the current message")
        );
        assert_eq!(activity.origin, ActivityOrigin::User);
    }

    #[test]
    fn ignores_unrelated_codex_events() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count"}}"#;
        assert_eq!(codex_line_to_message_type(line), None);
    }

    #[test]
    fn extracts_openclaw_assistant_message_text_for_bubble() {
        let line = r#"{"type":"message","id":"7d33653f","parentId":"4541d9a1","timestamp":"2026-03-13T12:47:12.949Z","message":{"role":"assistant","content":[{"type":"text","text":"It is a football player. Let me see if there are any other relevant results:"},{"type":"toolCall","id":"ollama_call_fc556e19-48ea-4ece-a09b-e4a3591a828c","name":"web_search","arguments":{"count":10,"query":"Lionell Messi"}}],"stopReason":"toolUse","api":"ollama","provider":"ollama","model":"glm-5:cloud","usage":{"input":23501,"output":220,"cacheRead":0,"cacheWrite":0,"totalTokens":23721,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":1773406032947}}"#;
        let activity = openclaw_line_to_activity(line).unwrap();
        assert_eq!(activity.message_type, MSG_NEW_MESSAGE);
        assert_eq!(
            activity.bubble_text.as_deref(),
            Some("It is a football player. Let me see if there are any other relevant results:")
        );
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
    }

    #[test]
    fn maps_openclaw_tool_call_only_to_processing() {
        let line = r#"{"type":"message","id":"de807fc0","timestamp":"2026-02-27T05:45:19.968Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call_auto_1","name":"search","arguments":{"query":"x"}}],"stopReason":"toolUse","api":"openai-completions","provider":"vllm","model":"vllm/gemma-3","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":1772659796512}}"#;
        let activity = openclaw_line_to_activity(line).unwrap();
        assert_eq!(activity.message_type, MSG_PROCESSING);
        assert_eq!(activity.bubble_text.as_deref(), Some("Working..."));
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
    }

    #[test]
    fn extracts_hermes_assistant_message_text_for_bubble() {
        let line = r#"{"role":"assistant","content":"I found the issue and will patch it now.","reasoning":"Need to inspect the local config first","finish_reason":"stop","timestamp":"2026-05-04T23:25:09.892976"}"#;
        let activity = hermes_line_to_activity(line).unwrap();
        assert_eq!(activity.message_type, MSG_NEW_MESSAGE);
        assert_eq!(
            activity.bubble_text.as_deref(),
            Some("I found the issue and will patch it now.")
        );
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
    }

    #[test]
    fn maps_hermes_tool_call_only_to_processing_and_skips_delivery_mirror() {
        let tool_line = r#"{"type":"message","id":"20260504_211412_ab12cd34","message":{"role":"assistant","content":[{"type":"toolCall","id":"call_auto_1","name":"search","arguments":{"query":"x"}}],"provider":"anthropic","model":"claude-sonnet-4-6","stopReason":"toolUse"}}"#;
        let mirror_line = r#"{"type":"message","id":"20260504_211412_ab12cd34","message":{"role":"assistant","content":[{"type":"text","text":"sent elsewhere"}],"provider":"openclaw","model":"delivery-mirror","stopReason":"stop"}}"#;
        let activity = hermes_line_to_activity(tool_line).unwrap();
        assert_eq!(activity.message_type, MSG_PROCESSING);
        assert_eq!(activity.bubble_text.as_deref(), Some("Working..."));
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
        assert!(hermes_line_to_activity(mirror_line).is_none());
    }

    #[test]
    fn antigravity_conversation_filter_requires_conversations_pb() {
        assert!(is_antigravity_conversation_file(Path::new(
            "/tmp/antigravity/conversations/session.pb"
        )));
        assert!(!is_antigravity_conversation_file(Path::new(
            "/tmp/antigravity/implicit/session.pb"
        )));
        assert!(!is_antigravity_conversation_file(Path::new(
            "/tmp/antigravity/conversations/session.json"
        )));
    }

    #[test]
    fn antigravity_extracts_assistant_text_from_blob() {
        let blob =
            b"\0assistant\0I checked the project and updated the implementation plan for you.\0";
        let activity = antigravity_blob_to_activity(blob).unwrap();
        assert_eq!(activity.message_type, MSG_PROCESSING);
        assert_eq!(activity.bubble_text.as_deref(), Some("Working..."));
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
    }

    #[test]
    fn antigravity_user_only_blob_is_ignored_by_emit_path() {
        let blob = b"\0user\0Please inspect the current project configuration.\0";
        let activity = antigravity_blob_to_activity(blob).unwrap();
        assert_eq!(activity.message_type, MSG_MENTION);
        assert_eq!(activity.origin, ActivityOrigin::User);
    }

    #[test]
    fn antigravity_maps_error_blob_to_error() {
        let blob = b"\0assistant\0The command failed with an exception while running tests.\0";
        let activity = antigravity_blob_to_activity(blob).unwrap();
        assert_eq!(activity.message_type, MSG_ERROR);
        assert_eq!(activity.bubble_text.as_deref(), Some("Something failed"));
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
    }

    #[test]
    fn antigravity_brain_metadata_filter_accepts_known_files() {
        assert!(is_antigravity_brain_metadata_file(Path::new(
            "/tmp/antigravity/brain/session/task.md.metadata.json"
        )));
        assert!(is_antigravity_brain_metadata_file(Path::new(
            "/tmp/antigravity/brain/session/implementation_plan.md.metadata.json"
        )));
        assert!(!is_antigravity_brain_metadata_file(Path::new(
            "/tmp/antigravity/conversations/task.md.metadata.json"
        )));
    }

    #[test]
    fn expands_unix_style_environment_variables() {
        std::env::set_var("AGENT_PET_TEST_DIR", "/tmp/agent-pet");
        assert_eq!(
            expand_env_vars("$AGENT_PET_TEST_DIR/sessions"),
            "/tmp/agent-pet/sessions"
        );
        assert_eq!(
            expand_env_vars("${AGENT_PET_TEST_DIR}/sessions"),
            "/tmp/agent-pet/sessions"
        );
        std::env::remove_var("AGENT_PET_TEST_DIR");
    }

    #[test]
    fn preserves_unknown_environment_variables() {
        std::env::remove_var("AGENT_PET_UNKNOWN_DIR");
        assert_eq!(
            expand_env_vars("$AGENT_PET_UNKNOWN_DIR/sessions"),
            "$AGENT_PET_UNKNOWN_DIR/sessions"
        );
        assert_eq!(
            expand_env_vars("${AGENT_PET_UNKNOWN_DIR}/sessions"),
            "${AGENT_PET_UNKNOWN_DIR}/sessions"
        );
    }
}
