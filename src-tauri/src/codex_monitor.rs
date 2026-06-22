// Watches Codex session JSONL files and maps live Codex activity to pet messages.

use crate::message::{
    PetNotice, PetUsageMetric, MSG_ERROR, MSG_MENTION, MSG_NEW_MESSAGE, MSG_PROCESSING,
    MSG_SUCCESS, MSG_WAITING_INPUT,
};
use crate::state_machine::PetStateMachine;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::async_runtime::Mutex;
use tokio_util::sync::CancellationToken;

const USAGE_SUMMARY_24H_SECS: u64 = 24 * 60 * 60;
const USAGE_SUMMARY_7D_SECS: u64 = 7 * 24 * 60 * 60;
const USAGE_SUMMARY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const STARTUP_ACTIVITY_REPLAY_SECS: u64 = 10 * 60;

struct CodexActivity {
    message_type: &'static str,
    bubble_text: Option<String>,
    origin: ActivityOrigin,
    usage: Option<ActivityUsage>,
    usage_metrics: Vec<ActivityUsageMetric>,
    affects_state: bool,
}

#[derive(Debug, Clone)]
struct ActivityUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: Option<u64>,
    total_cost: Option<f64>,
}

#[derive(Debug, Clone)]
struct ActivityUsageMetric {
    id_suffix: String,
    label: String,
    value: String,
    detail: String,
    percent: Option<f64>,
    status: String,
    meta: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct UsageTotals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

impl UsageTotals {
    fn from_usage(usage: &ActivityUsage) -> Self {
        Self {
            input: usage.input_tokens.unwrap_or(0),
            output: usage.output_tokens.unwrap_or(0),
            cache_read: usage.cache_read_tokens.unwrap_or(0),
            cache_write: usage.cache_write_tokens.unwrap_or(0),
        }
    }

    fn real_total(self, input_includes_cache_read: bool) -> u64 {
        let fresh_input = if input_includes_cache_read {
            self.input.saturating_sub(self.cache_read)
        } else {
            self.input
        };
        fresh_input + self.output + self.cache_read + self.cache_write
    }

    fn is_zero(self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_write == 0
    }

    fn delta_since(self, previous: Option<Self>) -> Self {
        match previous {
            Some(previous) => Self {
                input: self.input.saturating_sub(previous.input),
                output: self.output.saturating_sub(previous.output),
                cache_read: self.cache_read.saturating_sub(previous.cache_read),
                cache_write: self.cache_write.saturating_sub(previous.cache_write),
            },
            None => self,
        }
    }

    fn add_assign(&mut self, other: Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
    }
}

#[derive(Debug, Clone)]
struct MessageUsageCandidate {
    usage: UsageTotals,
    has_stop_reason: bool,
    timestamp: Option<SystemTime>,
    fallback_time: SystemTime,
    sequence: u64,
}

impl MessageUsageCandidate {
    fn effective_timestamp(&self) -> SystemTime {
        self.timestamp.unwrap_or(self.fallback_time)
    }
}

#[derive(Debug, Clone, Default)]
struct UsageSummaryState {
    total_24h: UsageTotals,
    total_7d: UsageTotals,
    last: Option<UsageTotals>,
    last_timestamp: Option<SystemTime>,
    previous_cumulative: Option<UsageTotals>,
    message_usages: HashMap<String, MessageUsageCandidate>,
    next_sequence: u64,
}

impl UsageSummaryState {
    fn add_timed_usage(
        &mut self,
        usage: UsageTotals,
        timestamp: Option<SystemTime>,
        fallback_time: SystemTime,
        now: SystemTime,
    ) {
        if usage.is_zero() {
            return;
        }

        let event_time = timestamp.unwrap_or(fallback_time);
        if is_within_window(event_time, now, USAGE_SUMMARY_24H_SECS) {
            self.total_24h.add_assign(usage);
        }
        if is_within_window(event_time, now, USAGE_SUMMARY_7D_SECS) {
            self.total_7d.add_assign(usage);
        }
        if self
            .last_timestamp
            .map(|last_time| event_time >= last_time)
            .unwrap_or(true)
        {
            self.last = Some(usage);
            self.last_timestamp = Some(event_time);
        }
    }

    fn reset_usage_windows(&mut self) {
        self.total_24h = UsageTotals::default();
        self.total_7d = UsageTotals::default();
        self.last = None;
        self.last_timestamp = None;
    }

    fn rebuild_from_message_usages(&mut self, now: SystemTime) {
        let mut candidates = self
            .message_usages
            .values()
            .cloned()
            .collect::<Vec<MessageUsageCandidate>>();
        candidates.sort_by(|a, b| {
            a.effective_timestamp()
                .cmp(&b.effective_timestamp())
                .then_with(|| a.sequence.cmp(&b.sequence))
        });
        self.reset_usage_windows();
        for candidate in candidates {
            self.add_timed_usage(
                candidate.usage,
                candidate.timestamp,
                candidate.fallback_time,
                now,
            );
        }
    }
}

#[derive(Debug, Clone, Default)]
struct SourceUsageCursor {
    summary: UsageSummaryState,
    last_full_refresh: Option<SystemTime>,
}

impl CodexActivity {
    fn has_usage_metric(&self) -> bool {
        self.usage.is_some() || !self.usage_metrics.is_empty()
    }
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
        let mut codex_usage_cursor = SourceUsageCursor::default();
        let mut claude_cursor = FileCursor::default();
        let mut claude_usage_cursor = SourceUsageCursor::default();
        let mut opencode_cursor = OpencodeCursor::default();
        let mut opencode_usage_cursor = SourceUsageCursor::default();
        let mut openclaw_cursor = FileCursor::default();
        let mut openclaw_usage_cursor = SourceUsageCursor::default();
        let mut hermes_cursor = FileCursor::default();
        let mut hermes_usage_cursor = SourceUsageCursor::default();
        let mut antigravity_conversation_cursor = BinaryFileCursor::default();
        let mut antigravity_brain_cursor = MetadataCursor::default();
        let mut antigravity_usage_cursor = SourceUsageCursor::default();

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
                    &mut codex_usage_cursor,
                    &self.state_machine,
                    "codex",
                    is_codex_session_file,
                    codex_line_to_activity,
                    codex_usage_summary_in_file,
                    codex_usage_summary_in_lines,
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
                    &mut claude_usage_cursor,
                    &self.state_machine,
                    "claude",
                    is_jsonl_file,
                    claude_line_to_activity,
                    claude_usage_summary_in_file,
                    claude_usage_summary_in_lines,
                )
                .await
                {
                    log::warn!("Claude monitor poll failed: {}", e);
                }
            }

            if let Some(db_path) = opencode_path.as_deref().filter(|path| path.exists()) {
                if let Err(e) = poll_opencode_source(
                    db_path,
                    &mut opencode_cursor,
                    &mut opencode_usage_cursor,
                    &self.state_machine,
                )
                .await
                {
                    log::warn!("opencode monitor poll failed: {}", e);
                }
            }

            if let Some(sessions_root) = openclaw_path.as_deref() {
                if let Err(e) = poll_jsonl_source(
                    sessions_root,
                    &mut openclaw_cursor,
                    &mut openclaw_usage_cursor,
                    &self.state_machine,
                    "openclaw",
                    is_openclaw_session_file,
                    openclaw_line_to_activity,
                    openclaw_usage_summary_in_file,
                    openclaw_usage_summary_in_lines,
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
                    &mut hermes_usage_cursor,
                    &self.state_machine,
                    "hermes",
                    is_hermes_session_file,
                    hermes_line_to_activity,
                    hermes_usage_summary_in_file,
                    hermes_usage_summary_in_lines,
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
                    &mut antigravity_usage_cursor,
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
    usage_cursor: &mut SourceUsageCursor,
    state_machine: &Arc<Mutex<PetStateMachine>>,
    source: &'static str,
    file_filter: fn(&Path) -> bool,
    parser: fn(&str) -> Option<CodexActivity>,
    usage_file_parser: fn(&Path, SystemTime) -> Result<UsageSummaryState, std::io::Error>,
    usage_lines_parser: fn(&str, &mut UsageSummaryState, SystemTime, SystemTime),
) -> Result<(), Box<dyn std::error::Error>> {
    let Some((path, path_modified)) = newest_file_with_modified(root, file_filter)? else {
        return Ok(());
    };
    let now = SystemTime::now();

    if cursor.path.as_ref() != Some(&path) {
        let len = fs::metadata(&path)?.len();
        let mut summary = usage_summary_in_recent_files(root, file_filter, usage_file_parser, now)?;
        let active_summary = usage_file_parser(&path, now)?;
        summary.previous_cumulative = active_summary.previous_cumulative;
        cursor.path = Some(path);
        cursor.offset = len;
        usage_cursor.summary = summary;
        usage_cursor.last_full_refresh = Some(now);
        emit_usage_summary_metrics(state_machine, source, &usage_cursor.summary).await;
        if is_within_window(path_modified, now, STARTUP_ACTIVITY_REPLAY_SECS) {
            let latest_activity =
                latest_usage_activity_in_file(cursor.path.as_ref().unwrap(), parser)?.or_else(
                    || {
                        latest_activity_in_file(cursor.path.as_ref().unwrap(), parser)
                            .ok()
                            .flatten()
                    },
                );
            if let Some(activity) = latest_activity {
                emit_activity(state_machine, source, activity).await;
            }
        }
        return Ok(());
    }

    let len = fs::metadata(&path)?.len();
    if len < cursor.offset {
        cursor.offset = 0;
    }
    if len == cursor.offset {
        if should_refresh_usage_summary(usage_cursor.last_full_refresh, now) {
            let mut summary =
                usage_summary_in_recent_files(root, file_filter, usage_file_parser, now)?;
            let active_summary = usage_file_parser(&path, now)?;
            summary.previous_cumulative = active_summary.previous_cumulative;
            usage_cursor.summary = summary;
            usage_cursor.last_full_refresh = Some(now);
            emit_usage_summary_metrics(state_machine, source, &usage_cursor.summary).await;
        }
        return Ok(());
    }

    let mut file = File::open(&path)?;
    file.seek(SeekFrom::Start(cursor.offset))?;
    let appended = read_text_lossy(&mut file)?;
    cursor.offset = len;

    let fallback_time = fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(now);
    usage_lines_parser(&appended, &mut usage_cursor.summary, now, fallback_time);
    emit_usage_summary_metrics(state_machine, source, &usage_cursor.summary).await;

    for line in appended.lines() {
        if let Some(activity) = parser(line) {
            emit_activity(state_machine, source, activity).await;
        }
    }

    Ok(())
}

fn should_refresh_usage_summary(last_refresh: Option<SystemTime>, now: SystemTime) -> bool {
    last_refresh
        .and_then(|last| now.duration_since(last).ok())
        .map(|elapsed| elapsed >= USAGE_SUMMARY_REFRESH_INTERVAL)
        .unwrap_or(true)
}

fn usage_summary_in_recent_files(
    root: &Path,
    file_filter: fn(&Path) -> bool,
    usage_file_parser: fn(&Path, SystemTime) -> Result<UsageSummaryState, std::io::Error>,
    now: SystemTime,
) -> Result<UsageSummaryState, std::io::Error> {
    let mut files = recent_jsonl_files(root, file_filter, now)?;
    files.sort_by(|(_, a_modified), (_, b_modified)| a_modified.cmp(b_modified));

    let mut summary = UsageSummaryState::default();
    let mut uses_message_dedup = false;

    for (path, _modified) in files {
        let file_summary = usage_file_parser(&path, now)?;
        if file_summary.message_usages.is_empty() {
            merge_window_totals(&mut summary, file_summary);
            continue;
        }

        uses_message_dedup = true;
        let mut candidates = file_summary
            .message_usages
            .into_iter()
            .collect::<Vec<(String, MessageUsageCandidate)>>();
        candidates.sort_by(|(_, a), (_, b)| {
            a.effective_timestamp()
                .cmp(&b.effective_timestamp())
                .then_with(|| a.sequence.cmp(&b.sequence))
        });
        for (message_id, mut candidate) in candidates {
            candidate.sequence = summary.next_sequence;
            summary.next_sequence = summary.next_sequence.saturating_add(1);
            upsert_message_usage_candidate(&mut summary.message_usages, message_id, candidate);
        }
    }

    if uses_message_dedup {
        summary.rebuild_from_message_usages(now);
    }

    Ok(summary)
}

fn recent_jsonl_files(
    root: &Path,
    file_filter: fn(&Path) -> bool,
    now: SystemTime,
) -> Result<Vec<(PathBuf, SystemTime)>, std::io::Error> {
    let mut files = Vec::new();

    if root.is_file() {
        if file_filter(root) {
            let modified = fs::metadata(root)?
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if is_within_window(modified, now, USAGE_SUMMARY_7D_SECS) {
                files.push((root.to_path_buf(), modified));
            }
        }
        return Ok(files);
    }

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
            if is_within_window(modified, now, USAGE_SUMMARY_7D_SECS) {
                files.push((path, modified));
            }
        }
    }

    Ok(files)
}

fn merge_window_totals(summary: &mut UsageSummaryState, other: UsageSummaryState) {
    summary.total_24h.add_assign(other.total_24h);
    summary.total_7d.add_assign(other.total_7d);
    if let Some(last) = other.last {
        if other
            .last_timestamp
            .zip(summary.last_timestamp)
            .map(|(other_time, current_time)| other_time >= current_time)
            .unwrap_or(summary.last.is_none())
        {
            summary.last = Some(last);
            summary.last_timestamp = other.last_timestamp;
        }
    }
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
        if activity.affects_state {
            sm.handle_message(activity.message_type);
        }
        if let Some(metric) = activity_usage_metric(
            source,
            "tokens",
            "Tokens",
            "usage_tokens",
            activity.usage.as_ref(),
        ) {
            sm.upsert_usage_metric(metric);
        }
        for metric in activity
            .usage_metrics
            .iter()
            .rev()
            .map(|usage_metric| usage_metric.to_pet_usage_metric(source))
        {
            sm.upsert_usage_metric(metric);
        }
        if let Some(notice) = activity_notice(source, &activity) {
            sm.show_notice(&notice);
            return;
        }
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

async fn emit_usage_summary_metrics(
    state_machine: &Arc<Mutex<PetStateMachine>>,
    source: &str,
    summary: &UsageSummaryState,
) {
    let mut metrics = Vec::new();
    if let Some(metric) = usage_summary_metric(
        source,
        "tokens-24h",
        "24小时用量",
        "total_24h_tokens",
        summary.total_24h,
        Some(USAGE_SUMMARY_24H_SECS),
        source_has_usage_summary(source),
    ) {
        metrics.push(metric);
    }
    if let Some(metric) = usage_summary_metric(
        source,
        "tokens-7d",
        "7天用量",
        "total_7d_tokens",
        summary.total_7d,
        Some(USAGE_SUMMARY_7D_SECS),
        source_has_usage_summary(source),
    ) {
        metrics.push(metric);
    }
    if let Some(last) = summary.last {
        if let Some(metric) = usage_summary_metric(
            source,
            "tokens-last",
            "最近一次",
            "last_tokens",
            last,
            None,
            false,
        ) {
            metrics.push(metric);
        }
    }

    if metrics.is_empty() {
        return;
    }

    let mut sm = state_machine.lock().await;
    if !sm.live_source_enabled(source) {
        return;
    }
    for metric in metrics {
        sm.upsert_usage_metric(metric);
    }
}

fn source_has_usage_summary(source: &str) -> bool {
    matches!(
        source,
        "codex" | "claude" | "opencode" | "openclaw" | "hermes" | "antigravity"
    )
}

fn usage_summary_metric(
    source: &str,
    id_suffix: &str,
    label: &str,
    kind: &str,
    totals: UsageTotals,
    window_seconds: Option<u64>,
    include_zero: bool,
) -> Option<PetUsageMetric> {
    let input_includes_cache_read = matches!(source, "codex" | "gemini" | "antigravity");
    let total_tokens = totals.real_total(input_includes_cache_read);
    if total_tokens == 0 && !include_zero {
        return None;
    }

    let mut details = Vec::new();
    let fresh_input = if input_includes_cache_read {
        totals.input.saturating_sub(totals.cache_read)
    } else {
        totals.input
    };
    if fresh_input > 0 {
        details.push(format!("in {}", format_token_count(fresh_input)));
    }
    if totals.output > 0 {
        details.push(format!("out {}", format_token_count(totals.output)));
    }
    if totals.cache_read > 0 {
        details.push(format!("cache {}", format_token_count(totals.cache_read)));
    }
    if totals.cache_write > 0 {
        details.push(format!("write {}", format_token_count(totals.cache_write)));
    }

    Some(PetUsageMetric {
        id: format!("{}-{}", source, id_suffix),
        source: source.to_string(),
        source_label: Some(source_label(source).to_string()),
        label: label.to_string(),
        value: format_token_count(total_tokens),
        detail: details.join(" · "),
        percent: None,
        status: "info".to_string(),
        meta: serde_json::json!({
            "kind": kind,
            "totalTokens": total_tokens,
            "inputTokens": fresh_input,
            "rawInputTokens": totals.input,
            "outputTokens": totals.output,
            "cacheReadTokens": totals.cache_read,
            "cacheWriteTokens": totals.cache_write,
            "source": "session_delta",
            "windowSeconds": window_seconds,
        }),
        timestamp: None,
    })
}

impl ActivityUsageMetric {
    fn to_pet_usage_metric(&self, source: &str) -> PetUsageMetric {
        PetUsageMetric {
            id: format!("{}-{}", source, self.id_suffix),
            source: source.to_string(),
            source_label: Some(source_label(source).to_string()),
            label: self.label.clone(),
            value: self.value.clone(),
            detail: self.detail.clone(),
            percent: self.percent,
            status: self.status.clone(),
            meta: self.meta.clone(),
            timestamp: None,
        }
    }
}

fn activity_usage_metric(
    source: &str,
    id_suffix: &str,
    label: &str,
    kind: &str,
    usage: Option<&ActivityUsage>,
) -> Option<PetUsageMetric> {
    let usage = usage?;
    let total_tokens = usage.total_tokens.or_else(|| {
        Some(
            usage.input_tokens.unwrap_or(0)
                + usage.output_tokens.unwrap_or(0)
                + usage.cache_read_tokens.unwrap_or(0)
                + usage.cache_write_tokens.unwrap_or(0),
        )
        .filter(|total| *total > 0)
    })?;

    let value = if let Some(total_cost) = usage
        .total_cost
        .filter(|cost| cost.is_finite() && *cost > 0.0)
    {
        format!("${:.4}", total_cost)
    } else {
        format_token_count(total_tokens)
    };

    let mut details = Vec::new();
    if let Some(input) = usage.input_tokens.filter(|tokens| *tokens > 0) {
        details.push(format!("in {}", format_token_count(input)));
    }
    if let Some(output) = usage.output_tokens.filter(|tokens| *tokens > 0) {
        details.push(format!("out {}", format_token_count(output)));
    }
    if let Some(cache_read) = usage.cache_read_tokens.filter(|tokens| *tokens > 0) {
        details.push(format!("cache {}", format_token_count(cache_read)));
    }

    Some(PetUsageMetric {
        id: format!("{}-{}", source, id_suffix),
        source: source.to_string(),
        source_label: Some(source_label(source).to_string()),
        label: label.to_string(),
        value,
        detail: details.join(" · "),
        percent: None,
        status: "info".to_string(),
        meta: serde_json::json!({
            "kind": kind,
            "totalTokens": total_tokens,
            "inputTokens": usage.input_tokens,
            "outputTokens": usage.output_tokens,
            "cacheReadTokens": usage.cache_read_tokens,
            "cacheWriteTokens": usage.cache_write_tokens,
            "totalCost": usage.total_cost,
        }),
        timestamp: None,
    })
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn activity_notice(source: &str, activity: &CodexActivity) -> Option<PetNotice> {
    let body = activity
        .bubble_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("");
    let (level, title, notice_type, action_hint) = match activity.message_type {
        MSG_WAITING_INPUT => notice_action_for_waiting_input(body),
        MSG_PROCESSING if looks_like_context_compacting(body) => (
            "info",
            "正在整理上下文",
            "context_compacting",
            Some("等待完成"),
        ),
        MSG_ERROR => ("error", "任务失败", "task_failed", Some("查看来源")),
        _ => return None,
    };
    let source_label = source_label(source).to_string();
    let body = if body.is_empty() { title } else { body };

    Some(PetNotice {
        id: format!("{}-{}", source, activity.message_type.replace('_', "-")),
        group_key: format!("{}-{}", source, activity.message_type.replace('_', "-")),
        level: level.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        source: source.to_string(),
        source_label: Some(source_label),
        notice_type: notice_type.to_string(),
        action_hint: action_hint.map(str::to_string),
        action_label: None,
        focus_source: true,
        action_kind: Some("focus".to_string()),
        automation_safe: false,
        ttl_seconds: 600,
        timestamp: None,
    })
}

fn notice_action_for_waiting_input(
    text: &str,
) -> (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
) {
    if looks_like_press_enter_prompt(text) {
        return ("warning", "等待继续", "press_enter_required", Some("Enter"));
    }
    if looks_like_approval_prompt(text) {
        return (
            "warning",
            "需要批准",
            "approval_required",
            Some("Allow / Deny"),
        );
    }
    if looks_like_confirm_prompt(text) {
        return ("warning", "需要确认", "confirm_required", Some("Y + Enter"));
    }
    if looks_like_context_compacting(text) {
        return (
            "info",
            "正在整理上下文",
            "context_compacting",
            Some("等待完成"),
        );
    }

    (
        "warning",
        "需要你处理",
        "confirm_required",
        Some("查看来源"),
    )
}

fn looks_like_approval_prompt(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("批准")
        || text.contains("允许")
        || text.contains("授权")
        || text.contains("permission")
        || text.contains("approval")
        || text.contains("allow")
        || text.contains("deny")
}

fn looks_like_confirm_prompt(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("(y/n)")
        || text.contains("[y/n]")
        || text.contains("yes/no")
        || text.contains("proceed?")
        || text.contains("continue?")
        || text.contains("是否继续")
        || text.contains("确认继续")
}

fn looks_like_press_enter_prompt(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("press enter")
        || text.contains("hit enter")
        || text.contains("enter to continue")
        || text.contains("return to continue")
        || text.contains("按 enter")
        || text.contains("按回车")
        || text.contains("回车继续")
}

fn looks_like_context_compacting(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    (text.contains("compact") && text.contains("context"))
        || text.contains("compacting")
        || text.contains("summarizing context")
        || text.contains("context prune")
        || text.contains("压缩上下文")
        || text.contains("整理上下文")
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
}

fn is_hermes_session_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("jsonl" | "json")
    )
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
    latest_matching_activity_in_file(path, parser, |_| true)
}

fn latest_usage_activity_in_file(
    path: &Path,
    parser: fn(&str) -> Option<CodexActivity>,
) -> Result<Option<CodexActivity>, std::io::Error> {
    latest_matching_activity_in_file(path, parser, CodexActivity::has_usage_metric)
}

fn latest_matching_activity_in_file(
    path: &Path,
    parser: fn(&str) -> Option<CodexActivity>,
    predicate: fn(&CodexActivity) -> bool,
) -> Result<Option<CodexActivity>, std::io::Error> {
    const TAIL_BYTES: u64 = 64 * 1024;

    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;

    let tail = read_text_lossy(&mut file)?;
    let lines = if start > 0 {
        tail.lines().skip(1).collect::<Vec<_>>()
    } else {
        tail.lines().collect::<Vec<_>>()
    };

    Ok(lines
        .into_iter()
        .rev()
        .filter_map(parser)
        .find(|activity| predicate(activity)))
}

fn codex_usage_summary_in_file(
    path: &Path,
    now: SystemTime,
) -> Result<UsageSummaryState, std::io::Error> {
    let mut file = File::open(path)?;
    let text = read_text_lossy(&mut file)?;
    let mut summary = UsageSummaryState::default();
    let fallback_time = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(now);
    codex_usage_summary_in_lines(&text, &mut summary, now, fallback_time);
    Ok(summary)
}

fn codex_usage_summary_in_lines(
    text: &str,
    summary: &mut UsageSummaryState,
    now: SystemTime,
    fallback_time: SystemTime,
) {
    for line in text.lines() {
        let Some((current, timestamp)) = codex_total_usage_from_line(line) else {
            continue;
        };
        let delta = current.delta_since(summary.previous_cumulative);
        summary.previous_cumulative = Some(current);
        summary.add_timed_usage(delta, timestamp, fallback_time, now);
    }
}

fn codex_total_usage_from_line(line: &str) -> Option<(UsageTotals, Option<SystemTime>)> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let timestamp = timestamp_from_value(&value);
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let usage = payload
        .get("info")
        .and_then(|info| info.get("total_token_usage"))
        .and_then(extract_usage)?;
    Some((UsageTotals::from_usage(&usage), timestamp))
}

fn claude_usage_summary_in_file(
    path: &Path,
    now: SystemTime,
) -> Result<UsageSummaryState, std::io::Error> {
    let mut file = File::open(path)?;
    let text = read_text_lossy(&mut file)?;
    let mut summary = UsageSummaryState::default();
    let fallback_time = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(now);
    claude_usage_summary_in_lines(&text, &mut summary, now, fallback_time);
    Ok(summary)
}

fn claude_usage_summary_in_lines(
    text: &str,
    summary: &mut UsageSummaryState,
    now: SystemTime,
    fallback_time: SystemTime,
) {
    for line in text.lines() {
        let Some((message_id, usage, has_stop_reason, timestamp)) = claude_usage_from_line(line)
        else {
            continue;
        };
        let sequence = summary.next_sequence;
        summary.next_sequence = summary.next_sequence.saturating_add(1);
        let candidate = MessageUsageCandidate {
            usage,
            has_stop_reason,
            timestamp,
            fallback_time,
            sequence,
        };
        upsert_message_usage_candidate(&mut summary.message_usages, message_id, candidate);
    }
    summary.rebuild_from_message_usages(now);
}

fn upsert_message_usage_candidate(
    messages: &mut HashMap<String, MessageUsageCandidate>,
    message_id: String,
    candidate: MessageUsageCandidate,
) {
    let should_replace = match messages.get(&message_id) {
        None => true,
        Some(existing) => {
            if candidate.has_stop_reason && !existing.has_stop_reason {
                true
            } else if candidate.has_stop_reason == existing.has_stop_reason {
                let current_time = candidate.effective_timestamp();
                let existing_time = existing.effective_timestamp();
                candidate.usage.output > existing.usage.output
                    || (candidate.usage.output == existing.usage.output
                        && (current_time > existing_time
                            || (current_time == existing_time
                                && candidate.sequence >= existing.sequence)))
            } else {
                false
            }
        }
    };

    if should_replace {
        messages.insert(message_id, candidate);
    }
}

fn claude_usage_from_line(line: &str) -> Option<(String, UsageTotals, bool, Option<SystemTime>)> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let timestamp = timestamp_from_value(&value);
    let message = value.get("message")?;
    let message_id = message.get("id").and_then(Value::as_str)?.to_string();
    let usage = extract_usage(message)?;
    let totals = UsageTotals::from_usage(&usage);
    let has_stop_reason = message
        .get("stop_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.trim().is_empty())
        .is_some();
    Some((message_id, totals, has_stop_reason, timestamp))
}

fn openclaw_usage_summary_in_file(
    path: &Path,
    now: SystemTime,
) -> Result<UsageSummaryState, std::io::Error> {
    message_usage_summary_in_file(path, now, openclaw_usage_from_value)
}

fn openclaw_usage_summary_in_lines(
    text: &str,
    summary: &mut UsageSummaryState,
    now: SystemTime,
    fallback_time: SystemTime,
) {
    message_usage_summary_in_lines(text, summary, now, fallback_time, openclaw_usage_from_value);
}

fn hermes_usage_summary_in_file(
    path: &Path,
    now: SystemTime,
) -> Result<UsageSummaryState, std::io::Error> {
    if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
        return hermes_json_usage_summary_in_file(path, now);
    }
    message_usage_summary_in_file(path, now, hermes_usage_from_value)
}

fn hermes_usage_summary_in_lines(
    text: &str,
    summary: &mut UsageSummaryState,
    now: SystemTime,
    fallback_time: SystemTime,
) {
    message_usage_summary_in_lines(text, summary, now, fallback_time, hermes_usage_from_value);
}

fn message_usage_summary_in_file(
    path: &Path,
    now: SystemTime,
    parser: fn(&Value) -> Option<(String, UsageTotals, bool, Option<SystemTime>)>,
) -> Result<UsageSummaryState, std::io::Error> {
    let mut file = File::open(path)?;
    let text = read_text_lossy(&mut file)?;
    let mut summary = UsageSummaryState::default();
    let fallback_time = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(now);
    message_usage_summary_in_lines(&text, &mut summary, now, fallback_time, parser);
    Ok(summary)
}

fn message_usage_summary_in_lines(
    text: &str,
    summary: &mut UsageSummaryState,
    now: SystemTime,
    fallback_time: SystemTime,
    parser: fn(&Value) -> Option<(String, UsageTotals, bool, Option<SystemTime>)>,
) {
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some((message_id, usage, has_stop_reason, timestamp)) = parser(&value) else {
            continue;
        };
        let sequence = summary.next_sequence;
        summary.next_sequence = summary.next_sequence.saturating_add(1);
        let candidate = MessageUsageCandidate {
            usage,
            has_stop_reason,
            timestamp,
            fallback_time,
            sequence,
        };
        upsert_message_usage_candidate(&mut summary.message_usages, message_id, candidate);
    }
    summary.rebuild_from_message_usages(now);
}

fn openclaw_usage_from_value(
    value: &Value,
) -> Option<(String, UsageTotals, bool, Option<SystemTime>)> {
    if value.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let message = value.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    message_usage_from_parts(value, message)
}

fn hermes_usage_from_value(
    value: &Value,
) -> Option<(String, UsageTotals, bool, Option<SystemTime>)> {
    let record_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message");
    if matches!(
        record_type,
        "session" | "model_change" | "thinking_level_change" | "custom"
    ) {
        return None;
    }
    let message = if record_type == "message" {
        value.get("message").unwrap_or(value)
    } else {
        value
    };
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    if message.get("model").and_then(Value::as_str) == Some("delivery-mirror")
        || message.get("provider").and_then(Value::as_str) == Some("openclaw")
            && message.get("model").and_then(Value::as_str) == Some("delivery-mirror")
    {
        return None;
    }
    message_usage_from_parts(value, message)
}

fn message_usage_from_parts(
    record: &Value,
    message: &Value,
) -> Option<(String, UsageTotals, bool, Option<SystemTime>)> {
    let usage = extract_usage(message).or_else(|| extract_usage(record))?;
    let totals = UsageTotals::from_usage(&usage);
    if totals.is_zero() {
        return None;
    }
    let message_id = message
        .get("id")
        .or_else(|| record.get("id"))
        .or_else(|| record.get("uuid"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            let ts = timestamp_from_value(record)
                .or_else(|| timestamp_from_value(message))
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos().to_string())?;
            Some(format!(
                "usage:{}:{}:{}:{}:{}",
                ts, totals.input, totals.output, totals.cache_read, totals.cache_write
            ))
        })?;
    let has_stop_reason = first_non_empty_str(message, &["stopReason", "stop_reason", "finish"])
        .or_else(|| first_non_empty_str(record, &["stopReason", "stop_reason", "finish"]))
        .is_some();
    let timestamp = timestamp_from_value(record).or_else(|| timestamp_from_value(message));
    Some((message_id, totals, has_stop_reason, timestamp))
}

fn hermes_json_usage_summary_in_file(
    path: &Path,
    now: SystemTime,
) -> Result<UsageSummaryState, std::io::Error> {
    let mut file = File::open(path)?;
    let text = read_text_lossy(&mut file)?;
    let fallback_time = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(now);
    let mut summary = UsageSummaryState::default();
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Ok(summary);
    };
    let session_id = value
        .get("session_id")
        .or_else(|| value.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for (index, message) in messages.enumerate() {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = extract_usage(message) else {
            continue;
        };
        let totals = UsageTotals::from_usage(&usage);
        if totals.is_zero() {
            continue;
        }
        let timestamp = timestamp_from_value(message)
            .or_else(|| timestamp_at_path(&value, &["last_updated"]))
            .or_else(|| timestamp_at_path(&value, &["session_start"]));
        let candidate = MessageUsageCandidate {
            usage: totals,
            has_stop_reason: first_non_empty_str(message, &["stopReason", "stop_reason", "finish"])
                .is_some(),
            timestamp,
            fallback_time,
            sequence: summary.next_sequence,
        };
        summary.next_sequence = summary.next_sequence.saturating_add(1);
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{session_id}:{index}"));
        upsert_message_usage_candidate(&mut summary.message_usages, message_id, candidate);
    }
    summary.rebuild_from_message_usages(now);
    Ok(summary)
}

fn timestamp_from_value(value: &Value) -> Option<SystemTime> {
    timestamp_at_path(value, &["timestamp"])
        .or_else(|| timestamp_at_path(value, &["ts"]))
        .or_else(|| timestamp_at_path(value, &["created_at"]))
        .or_else(|| timestamp_at_path(value, &["createdAt"]))
        .or_else(|| timestamp_at_path(value, &["time_created"]))
        .or_else(|| timestamp_at_path(value, &["timeCreated"]))
        .or_else(|| timestamp_at_path(value, &["created"]))
        .or_else(|| timestamp_at_path(value, &["updated"]))
        .or_else(|| timestamp_at_path(value, &["last_updated"]))
        .or_else(|| timestamp_at_path(value, &["lastUpdated"]))
        .or_else(|| timestamp_at_path(value, &["session_start"]))
        .or_else(|| timestamp_at_path(value, &["startTime"]))
        .or_else(|| timestamp_at_path(value, &["time", "created"]))
        .or_else(|| timestamp_at_path(value, &["time", "completed"]))
        .or_else(|| timestamp_at_path(value, &["message", "timestamp"]))
        .or_else(|| timestamp_at_path(value, &["message", "ts"]))
        .or_else(|| timestamp_at_path(value, &["message", "created_at"]))
        .or_else(|| timestamp_at_path(value, &["message", "createdAt"]))
        .or_else(|| timestamp_at_path(value, &["message", "time", "created"]))
}

fn timestamp_at_path(value: &Value, path: &[&str]) -> Option<SystemTime> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    value_to_system_time(current)
}

fn value_to_system_time(value: &Value) -> Option<SystemTime> {
    if let Some(number) = value.as_u64() {
        return unix_number_to_system_time(number);
    }
    if let Some(number) = value.as_i64().and_then(|number| u64::try_from(number).ok()) {
        return unix_number_to_system_time(number);
    }
    if let Some(number) = value.as_f64().filter(|number| *number >= 0.0) {
        return unix_number_to_system_time(number as u64);
    }

    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(number) = text.parse::<u64>() {
        return unix_number_to_system_time(number);
    }
    parse_datetime_text(text)
}

fn unix_number_to_system_time(number: u64) -> Option<SystemTime> {
    let (secs, nanos) = if number >= 10_000_000_000_000_000 {
        (number / 1_000_000_000, (number % 1_000_000_000) as u32)
    } else if number >= 10_000_000_000_000 {
        (number / 1_000_000, ((number % 1_000_000) * 1_000) as u32)
    } else if number >= 10_000_000_000 {
        (number / 1_000, ((number % 1_000) * 1_000_000) as u32)
    } else {
        (number, 0)
    };
    UNIX_EPOCH.checked_add(Duration::new(secs, nanos))
}

fn parse_datetime_text(text: &str) -> Option<SystemTime> {
    if let Ok(datetime) = DateTime::parse_from_rfc3339(text) {
        return datetime_to_system_time(datetime.with_timezone(&Utc));
    }

    const FORMATS: [&str; 4] = [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ];
    for format in FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return datetime_to_system_time(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
        }
    }
    None
}

fn datetime_to_system_time(datetime: DateTime<Utc>) -> Option<SystemTime> {
    let secs = datetime.timestamp();
    let nanos = datetime.timestamp_subsec_nanos();
    if secs < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::new(secs as u64, nanos))
}

fn is_within_window(event_time: SystemTime, now: SystemTime, window_secs: u64) -> bool {
    match now.duration_since(event_time) {
        Ok(age) => age <= Duration::from_secs(window_secs),
        Err(_) => true,
    }
}

fn read_text_lossy(file: &mut File) -> Result<String, std::io::Error> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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
            usage: None,
            usage_metrics: Vec::new(),
            affects_state: true,
        }),
        "task_started" => Some(activity(MSG_PROCESSING, "Working...")),
        "agent_message" => Some(CodexActivity {
            message_type: MSG_NEW_MESSAGE,
            bubble_text: payload_message_text(payload)
                .or_else(|| Some("Codex replied".to_string())),
            origin: ActivityOrigin::Assistant,
            usage: extract_usage(payload),
            usage_metrics: Vec::new(),
            affects_state: true,
        }),
        "token_count" => codex_token_count_activity(payload),
        "task_complete" => Some(activity(MSG_SUCCESS, "Done")),
        "exec_command_end" | "patch_apply_end" => {
            match payload.get("status").and_then(Value::as_str) {
                Some("failed") | Some("error") => Some(activity(MSG_ERROR, "Something failed")),
                _ => Some(activity(MSG_PROCESSING, "Working...")),
            }
        }
        _ => None,
    }
}

fn response_payload_to_activity(payload: &Value) -> Option<CodexActivity> {
    match payload.get("type")?.as_str()? {
        "reasoning" | "function_call_output" | "custom_tool_call_output" => {
            Some(activity(MSG_PROCESSING, "Working..."))
        }
        "function_call" | "custom_tool_call" => tool_call_approval_activity(payload)
            .or_else(|| Some(activity(MSG_PROCESSING, "Working..."))),
        "message" => match payload.get("role").and_then(Value::as_str) {
            Some("assistant") => Some(CodexActivity {
                message_type: MSG_NEW_MESSAGE,
                bubble_text: response_message_text(payload)
                    .or_else(|| Some("Codex replied".to_string())),
                origin: ActivityOrigin::Assistant,
                usage: extract_usage(payload),
                usage_metrics: Vec::new(),
                affects_state: true,
            }),
            Some("user") => Some(CodexActivity {
                message_type: MSG_MENTION,
                bubble_text: response_message_text(payload)
                    .or_else(|| Some("New request".to_string())),
                origin: ActivityOrigin::User,
                usage: None,
                usage_metrics: Vec::new(),
                affects_state: true,
            }),
            _ => None,
        },
        _ => None,
    }
}

fn tool_call_approval_activity(payload: &Value) -> Option<CodexActivity> {
    let raw_arguments = payload
        .get("arguments")
        .or_else(|| payload.get("args"))
        .or_else(|| payload.get("input"))?;
    let arguments = parse_tool_arguments(raw_arguments);

    if !value_requests_escalation(&arguments) {
        return None;
    }

    let tool_name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let reason = approval_reason(&arguments).unwrap_or_else(|| {
        format!(
            "{} 需要你确认后才能继续。",
            codex_tool_display_name(tool_name)
        )
    });

    Some(CodexActivity {
        message_type: MSG_WAITING_INPUT,
        bubble_text: Some(format!("需要批准：{}", reason.trim())),
        origin: ActivityOrigin::Assistant,
        usage: None,
        usage_metrics: Vec::new(),
        affects_state: true,
    })
}

fn parse_tool_arguments(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        other => other.clone(),
    }
}

fn value_requests_escalation(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            let key = key.as_str();
            matches!(key, "sandbox_permissions" | "sandboxPermissions")
                && value.as_str() == Some("require_escalated")
                || value_requests_escalation(value)
        }),
        Value::Array(items) => items.iter().any(value_requests_escalation),
        Value::String(text) => {
            text.contains("\"sandbox_permissions\":\"require_escalated\"")
                || text.contains("\"sandboxPermissions\":\"require_escalated\"")
                || text.contains("sandbox_permissions") && text.contains("require_escalated")
        }
        _ => false,
    }
}

fn approval_reason(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in [
                "justification",
                "approval_question",
                "approvalQuestion",
                "question",
                "reason",
            ] {
                if let Some(text) = map
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    return Some(truncate_notice_text(text));
                }
            }

            map.values().find_map(approval_reason)
        }
        Value::Array(items) => items.iter().find_map(approval_reason),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|parsed| approval_reason(&parsed)),
        _ => None,
    }
}

fn codex_tool_display_name(tool_name: &str) -> &str {
    match tool_name {
        "exec_command" => "命令执行",
        "apply_patch" => "文件修改",
        _ => "Codex 工具",
    }
}

fn codex_token_count_activity(payload: &Value) -> Option<CodexActivity> {
    let usage_metrics = codex_rate_limit_metrics(payload);

    if usage_metrics.is_empty() {
        return None;
    }

    Some(CodexActivity {
        message_type: MSG_PROCESSING,
        bubble_text: None,
        origin: ActivityOrigin::Assistant,
        usage: None,
        usage_metrics,
        affects_state: false,
    })
}

fn codex_rate_limit_metrics(payload: &Value) -> Vec<ActivityUsageMetric> {
    let Some(rate_limits) = payload.get("rate_limits") else {
        return Vec::new();
    };

    let mut metrics = Vec::new();
    if let Some(metric) = codex_rate_limit_metric(
        "short",
        "短时额度",
        "short_quota",
        rate_limits.get("primary"),
    ) {
        metrics.push(metric);
    }
    if let Some(metric) = codex_rate_limit_metric(
        "week",
        "本周额度",
        "weekly_quota",
        rate_limits.get("secondary"),
    ) {
        metrics.push(metric);
    }
    metrics
}

fn codex_rate_limit_metric(
    id_suffix: &str,
    label: &str,
    kind: &str,
    limit: Option<&Value>,
) -> Option<ActivityUsageMetric> {
    let limit = limit?;
    let used_percent = value_to_f64(limit.get("used_percent")?)?;
    let used_percent = used_percent.clamp(0.0, 100.0);
    let left_percent = (100.0 - used_percent).clamp(0.0, 100.0);
    let mut details = vec![format!("{:.0}% used", used_percent)];
    let window_minutes = first_u64(limit, &["window_minutes", "windowMinutes"]);
    let resets_at = limit
        .get("resets_at")
        .or_else(|| limit.get("resetsAt"))
        .and_then(value_to_u64);

    if let Some(window_minutes) = window_minutes {
        details.push(format_rate_limit_window(window_minutes));
    }
    if let Some(reset_detail) = resets_at.and_then(format_reset_time) {
        details.push(reset_detail);
    }

    Some(ActivityUsageMetric {
        id_suffix: format!("quota-{}", id_suffix),
        label: label.to_string(),
        value: format!("{:.0}%", left_percent),
        detail: details.join(" · "),
        percent: Some(left_percent),
        status: usage_status_for_remaining(left_percent).to_string(),
        meta: serde_json::json!({
            "kind": kind,
            "usedPercent": used_percent,
            "remainingPercent": left_percent,
            "windowMinutes": window_minutes,
            "resetsAt": resets_at,
        }),
    })
}

fn format_rate_limit_window(minutes: u64) -> String {
    if minutes >= 60 * 24 * 7 && minutes % (60 * 24 * 7) == 0 {
        format!("{}w", minutes / (60 * 24 * 7))
    } else if minutes >= 60 * 24 && minutes % (60 * 24) == 0 {
        format!("{}d", minutes / (60 * 24))
    } else if minutes >= 60 && minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}m", minutes)
    }
}

fn format_reset_time(epoch_seconds: u64) -> Option<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if epoch_seconds <= now {
        return Some("reset soon".to_string());
    }

    let minutes = (epoch_seconds - now).div_ceil(60);
    Some(format!(
        "reset in {}",
        format_rate_limit_window(minutes.max(1))
    ))
}

fn usage_status_for_remaining(percent: f64) -> &'static str {
    if percent <= 10.0 {
        "error"
    } else if percent <= 25.0 {
        "warning"
    } else {
        "success"
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
            usage: None,
            usage_metrics: Vec::new(),
            affects_state: true,
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
                    usage: extract_usage(&value),
                    usage_metrics: Vec::new(),
                    affects_state: true,
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
                    usage: extract_usage(&value).or_else(|| extract_usage(message)),
                    usage_metrics: Vec::new(),
                    affects_state: true,
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
            usage: None,
            usage_metrics: Vec::new(),
            affects_state: true,
        }),
        Some("assistant") => {
            let has_tool_call = message_has_openclaw_tool_call(message);

            if message.get("stopReason").and_then(Value::as_str) == Some("error") {
                Some(activity(MSG_ERROR, "Something failed"))
            } else if has_tool_call {
                Some(activity(MSG_PROCESSING, "Working..."))
            } else {
                Some(CodexActivity {
                    message_type: MSG_NEW_MESSAGE,
                    bubble_text: openclaw_message_text(message)
                        .or_else(|| Some("Replied".to_string())),
                    origin: ActivityOrigin::Assistant,
                    usage: extract_usage(message),
                    usage_metrics: Vec::new(),
                    affects_state: true,
                })
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
                    usage: None,
                    usage_metrics: Vec::new(),
                    affects_state: true,
                }),
                Some("assistant") => {
                    if message.get("model").and_then(Value::as_str) == Some("delivery-mirror")
                        || message.get("provider").and_then(Value::as_str) == Some("openclaw")
                            && message.get("model").and_then(Value::as_str)
                                == Some("delivery-mirror")
                    {
                        return None;
                    }

                    let has_tool_call = message_has_openclaw_tool_call(message);

                    if message.get("stopReason").and_then(Value::as_str) == Some("error") {
                        Some(activity(MSG_ERROR, "Something failed"))
                    } else if has_tool_call {
                        Some(activity(MSG_PROCESSING, "Working..."))
                    } else {
                        Some(CodexActivity {
                            message_type: MSG_NEW_MESSAGE,
                            bubble_text: hermes_message_text(message)
                                .or_else(|| Some("Replied".to_string())),
                            origin: ActivityOrigin::Assistant,
                            usage: extract_usage(message).or_else(|| extract_usage(&value)),
                            usage_metrics: Vec::new(),
                            affects_state: true,
                        })
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
    usage_cursor: &mut SourceUsageCursor,
    state_machine: &Arc<Mutex<PetStateMachine>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = SystemTime::now();
    if should_refresh_usage_summary(usage_cursor.last_full_refresh, now) {
        let gemini_root = root.parent().unwrap_or(root);
        usage_cursor.summary = antigravity_usage_summary_in_gemini_sessions(gemini_root, now)?;
        usage_cursor.last_full_refresh = Some(now);
        emit_usage_summary_metrics(state_machine, "antigravity", &usage_cursor.summary).await;
    }

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

fn antigravity_usage_summary_in_gemini_sessions(
    gemini_root: &Path,
    now: SystemTime,
) -> Result<UsageSummaryState, std::io::Error> {
    let session_files = collect_gemini_session_files(gemini_root, now)?;
    let mut summary = UsageSummaryState::default();
    for (path, modified) in session_files {
        let file_summary = gemini_session_usage_summary_in_file(&path, now, modified)?;
        merge_window_totals(&mut summary, file_summary);
    }
    Ok(summary)
}

fn collect_gemini_session_files(
    gemini_root: &Path,
    now: SystemTime,
) -> Result<Vec<(PathBuf, SystemTime)>, std::io::Error> {
    let tmp_dir = gemini_root.join("tmp");
    if !tmp_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for project_entry in fs::read_dir(tmp_dir)? {
        let project_entry = project_entry?;
        let chats_dir = project_entry.path().join("chats");
        if !chats_dir.is_dir() {
            continue;
        }
        for file_entry in fs::read_dir(chats_dir)? {
            let file_entry = file_entry?;
            let path = file_entry.path();
            let is_session = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("session-") && name.ends_with(".json"))
                .unwrap_or(false);
            if !is_session {
                continue;
            }
            let modified = file_entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if is_within_window(modified, now, USAGE_SUMMARY_7D_SECS) {
                files.push((path, modified));
            }
        }
    }
    files.sort_by(|(_, a_modified), (_, b_modified)| a_modified.cmp(b_modified));
    Ok(files)
}

fn gemini_session_usage_summary_in_file(
    path: &Path,
    now: SystemTime,
    fallback_time: SystemTime,
) -> Result<UsageSummaryState, std::io::Error> {
    let mut file = File::open(path)?;
    let text = read_text_lossy(&mut file)?;
    let mut summary = UsageSummaryState::default();
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Ok(summary);
    };
    let session_id = value
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for (index, message) in messages.enumerate() {
        if message.get("type").and_then(Value::as_str) != Some("gemini") {
            continue;
        }
        let Some(tokens) = message.get("tokens").filter(|tokens| tokens.is_object()) else {
            continue;
        };
        let usage = UsageTotals {
            input: first_u64(tokens, &["input"]).unwrap_or(0),
            output: first_u64(tokens, &["output"]).unwrap_or(0)
                + first_u64(tokens, &["thoughts"]).unwrap_or(0),
            cache_read: first_u64(tokens, &["cached"]).unwrap_or(0),
            cache_write: 0,
        };
        if usage.is_zero() {
            continue;
        }
        let timestamp = timestamp_at_path(message, &["timestamp"])
            .or_else(|| timestamp_at_path(&value, &["lastUpdated"]))
            .or_else(|| timestamp_at_path(&value, &["startTime"]));
        let candidate = MessageUsageCandidate {
            usage,
            has_stop_reason: true,
            timestamp,
            fallback_time,
            sequence: summary.next_sequence,
        };
        summary.next_sequence = summary.next_sequence.saturating_add(1);
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{session_id}:{index}"));
        upsert_message_usage_candidate(&mut summary.message_usages, message_id, candidate);
    }
    summary.rebuild_from_message_usages(now);
    Ok(summary)
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
    usage_cursor: &mut SourceUsageCursor,
    state_machine: &Arc<Mutex<PetStateMachine>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = SystemTime::now();
    if should_refresh_usage_summary(usage_cursor.last_full_refresh, now) {
        usage_cursor.summary = opencode_usage_summary_in_db(db_path, now)?;
        usage_cursor.last_full_refresh = Some(now);
        emit_usage_summary_metrics(state_machine, "opencode", &usage_cursor.summary).await;
    }

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

fn opencode_usage_summary_in_db(
    db_path: &Path,
    now: SystemTime,
) -> Result<UsageSummaryState, Box<dyn std::error::Error>> {
    let modified = opencode_db_modified(db_path).unwrap_or(now);
    let rows = query_opencode_usage_rows(db_path)?;
    let mut summary = UsageSummaryState::default();
    for row in rows {
        let Some((message_id, usage, timestamp)) = opencode_usage_from_message_row(&row) else {
            continue;
        };
        let candidate = MessageUsageCandidate {
            usage,
            has_stop_reason: true,
            timestamp,
            fallback_time: modified,
            sequence: summary.next_sequence,
        };
        summary.next_sequence = summary.next_sequence.saturating_add(1);
        upsert_message_usage_candidate(&mut summary.message_usages, message_id, candidate);
    }
    summary.rebuild_from_message_usages(now);
    Ok(summary)
}

fn opencode_db_modified(db_path: &Path) -> Option<SystemTime> {
    let base = fs::metadata(db_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let wal_path = db_path.with_extension("db-wal");
    let wal = fs::metadata(wal_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    match (base, wal) {
        (Some(base), Some(wal)) => Some(base.max(wal)),
        (Some(base), None) => Some(base),
        (None, Some(wal)) => Some(wal),
        (None, None) => None,
    }
}

struct OpencodeRow {
    time_created: i64,
    role: Option<String>,
    part_type: Option<String>,
    text: Option<String>,
    data: Value,
}

struct OpencodeUsageRow {
    id: String,
    session_id: String,
    data: Value,
}

fn query_opencode_usage_rows(
    db_path: &Path,
) -> Result<Vec<OpencodeUsageRow>, Box<dyn std::error::Error>> {
    let output = Command::new("/usr/bin/sqlite3")
        .arg("-json")
        .arg(db_path)
        .arg(
            "select id, session_id, data \
             from message \
             where json_extract(data,'$.role') = 'assistant' \
               and json_extract(data,'$.tokens') is not null \
               and json_extract(data,'$.time.completed') is not null \
             order by time_created asc;",
        )
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
            Some(OpencodeUsageRow {
                id: row.get("id")?.as_str()?.to_string(),
                session_id: row.get("session_id")?.as_str()?.to_string(),
                data,
            })
        })
        .collect();
    Ok(rows)
}

fn opencode_usage_from_message_row(
    row: &OpencodeUsageRow,
) -> Option<(String, UsageTotals, Option<SystemTime>)> {
    let tokens = row.data.get("tokens")?;
    let input = first_u64(tokens, &["input"]).unwrap_or(0);
    let output = first_u64(tokens, &["output"]).unwrap_or(0)
        + first_u64(tokens, &["reasoning"]).unwrap_or(0);
    let cache = tokens.get("cache");
    let cache_read = cache
        .and_then(|cache| first_u64(cache, &["read"]))
        .unwrap_or(0);
    let cache_write = cache
        .and_then(|cache| first_u64(cache, &["write"]))
        .unwrap_or(0);
    let usage = UsageTotals {
        input,
        output,
        cache_read,
        cache_write,
    };
    if usage.is_zero() {
        return None;
    }
    let timestamp = timestamp_at_path(&row.data, &["time", "created"]);
    Some((format!("{}:{}", row.session_id, row.id), usage, timestamp))
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
                    usage: None,
                    usage_metrics: Vec::new(),
                    affects_state: true,
                }),
                Some("assistant") => Some(CodexActivity {
                    message_type: MSG_NEW_MESSAGE,
                    bubble_text: Some(truncate_bubble_text(text)),
                    origin: ActivityOrigin::Assistant,
                    usage: extract_usage(&row.data),
                    usage_metrics: Vec::new(),
                    affects_state: true,
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
            usage: None,
            usage_metrics: Vec::new(),
            affects_state: true,
        });
    }

    if has_user_marker && !has_assistant_marker {
        return Some(CodexActivity {
            message_type: MSG_MENTION,
            bubble_text: antigravity_candidate_text(fragments)
                .or_else(|| Some("New request".to_string())),
            origin: ActivityOrigin::User,
            usage: None,
            usage_metrics: Vec::new(),
            affects_state: true,
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
        usage: None,
        usage_metrics: Vec::new(),
        affects_state: true,
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

fn extract_usage(value: &Value) -> Option<ActivityUsage> {
    let usage = value.get("usage").unwrap_or(value);
    let input_tokens = first_u64(
        usage,
        &[
            "input",
            "inputTokens",
            "input_tokens",
            "promptTokens",
            "prompt_tokens",
        ],
    );
    let output_tokens = first_u64(
        usage,
        &[
            "output",
            "outputTokens",
            "output_tokens",
            "completionTokens",
            "completion_tokens",
        ],
    );
    let cache_read_tokens = first_u64(
        usage,
        &[
            "cacheRead",
            "cache_read",
            "cacheReadTokens",
            "cache_read_tokens",
            "cache_read_input_tokens",
            "cachedTokens",
            "cached_tokens",
            "cached_input_tokens",
        ],
    );
    let cache_write_tokens = first_u64(
        usage,
        &[
            "cacheWrite",
            "cache_write",
            "cacheWriteTokens",
            "cache_write_tokens",
            "cache_creation_input_tokens",
            "cache_creation_tokens",
        ],
    );
    let total_tokens = first_u64(usage, &["total", "totalTokens", "total_tokens"]);
    let total_cost = usage
        .get("cost")
        .and_then(|cost| first_f64(cost, &["total", "totalCost", "total_cost"]))
        .or_else(|| first_f64(usage, &["cost", "totalCost", "total_cost"]));

    if [
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens,
    ]
    .into_iter()
    .flatten()
    .any(|tokens| tokens > 0)
        || total_cost.is_some_and(|cost| cost.is_finite() && cost > 0.0)
    {
        Some(ActivityUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            total_tokens,
            total_cost,
        })
    } else {
        None
    }
}

fn first_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(value_to_u64))
}

fn first_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(value_to_f64))
}

fn value_to_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|number| *number >= 0.0)
                .map(|number| number as u64)
        })
        .or_else(|| value.as_str()?.parse::<u64>().ok())
}

fn value_to_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
}

fn first_non_empty_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    })
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

fn truncate_notice_text(text: &str) -> String {
    const MAX_CHARS: usize = 140;
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
    fn maps_codex_escalated_command_to_approval_notice_activity() {
        let line = r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"find /Users/wangx/.local/share/opencode -maxdepth 3 -print\",\"sandbox_permissions\":\"require_escalated\",\"justification\":\"需要读取工作区外的 ~/.local/share/opencode 目录来确认 opencode 真实数据源结构，是否允许？\"}","call_id":"call_approval"}}"#;
        let activity = codex_line_to_activity(line).unwrap();

        assert_eq!(activity.message_type, MSG_WAITING_INPUT);
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
        assert!(activity.affects_state);
        assert_eq!(
            activity.bubble_text.as_deref(),
            Some("需要批准：需要读取工作区外的 ~/.local/share/opencode 目录来确认 opencode 真实数据源结构，是否允许？")
        );

        let notice = activity_notice("codex", &activity).unwrap();
        assert_eq!(notice.level, "warning");
        assert_eq!(notice.title, "需要批准");
        assert_eq!(notice.notice_type, "approval_required");
        assert_eq!(notice.action_hint.as_deref(), Some("Allow / Deny"));
        assert_eq!(notice.action_label, None);
        assert!(notice.focus_source);
        assert_eq!(notice.action_kind.as_deref(), Some("focus"));
        assert!(!notice.automation_safe);
        assert_eq!(notice.source_label.as_deref(), Some("Codex"));
    }

    #[test]
    fn maps_waiting_input_to_confirm_notice() {
        let activity = CodexActivity {
            message_type: MSG_WAITING_INPUT,
            bubble_text: Some("Continue? (y/n)".to_string()),
            origin: ActivityOrigin::Assistant,
            usage: None,
            usage_metrics: vec![],
            affects_state: true,
        };

        let notice = activity_notice("claude", &activity).unwrap();
        assert_eq!(notice.notice_type, "confirm_required");
        assert_eq!(notice.title, "需要确认");
        assert_eq!(notice.action_hint.as_deref(), Some("Y + Enter"));
        assert_eq!(notice.source_label.as_deref(), Some("Claude Code"));
    }

    #[test]
    fn maps_waiting_input_to_press_enter_notice() {
        let activity = CodexActivity {
            message_type: MSG_WAITING_INPUT,
            bubble_text: Some("Press Enter to continue".to_string()),
            origin: ActivityOrigin::Assistant,
            usage: None,
            usage_metrics: vec![],
            affects_state: true,
        };

        let notice = activity_notice("opencode", &activity).unwrap();
        assert_eq!(notice.notice_type, "press_enter_required");
        assert_eq!(notice.title, "等待继续");
        assert_eq!(notice.action_hint.as_deref(), Some("Enter"));
    }

    #[test]
    fn maps_processing_to_context_compacting_notice() {
        let activity = CodexActivity {
            message_type: MSG_PROCESSING,
            bubble_text: Some("Compacting context before continuing".to_string()),
            origin: ActivityOrigin::Assistant,
            usage: None,
            usage_metrics: vec![],
            affects_state: true,
        };

        let notice = activity_notice("hermes", &activity).unwrap();
        assert_eq!(notice.level, "info");
        assert_eq!(notice.notice_type, "context_compacting");
        assert_eq!(notice.title, "正在整理上下文");
        assert_eq!(notice.action_hint.as_deref(), Some("等待完成"));
    }

    #[test]
    fn maps_error_to_task_failed_notice() {
        let activity = CodexActivity {
            message_type: MSG_ERROR,
            bubble_text: Some("Command failed with exit code 1".to_string()),
            origin: ActivityOrigin::Assistant,
            usage: None,
            usage_metrics: vec![],
            affects_state: true,
        };

        let notice = activity_notice("openclaw", &activity).unwrap();
        assert_eq!(notice.level, "error");
        assert_eq!(notice.notice_type, "task_failed");
        assert_eq!(notice.title, "任务失败");
        assert_eq!(notice.action_hint.as_deref(), Some("查看来源"));
    }

    #[test]
    fn extracts_codex_assistant_message_text_for_bubble() {
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
    fn extracts_codex_token_count_as_usage_only_activity() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000000,"cached_input_tokens":500000,"output_tokens":40000,"total_tokens":1040000},"last_token_usage":{"input_tokens":204382,"cached_input_tokens":3840,"output_tokens":2024,"reasoning_output_tokens":0,"total_tokens":206406},"model_context_window":258400},"rate_limits":{"primary":{"used_percent":77.0,"window_minutes":300,"resets_at":4102444800},"secondary":{"used_percent":12.0,"window_minutes":10080,"resets_at":4102444800}}}}"#;
        let activity = codex_line_to_activity(line).unwrap();
        assert_eq!(activity.message_type, MSG_PROCESSING);
        assert_eq!(activity.bubble_text, None);
        assert_eq!(activity.origin, ActivityOrigin::Assistant);
        assert!(!activity.affects_state);

        assert!(activity.usage.is_none());
        assert_eq!(activity.usage_metrics.len(), 2);
        assert_eq!(activity.usage_metrics[0].id_suffix, "quota-short");
        assert_eq!(activity.usage_metrics[0].label, "短时额度");
        assert_eq!(activity.usage_metrics[0].value, "23%");
        assert_eq!(activity.usage_metrics[0].percent, Some(23.0));
        assert_eq!(activity.usage_metrics[0].status, "warning");
        assert_eq!(activity.usage_metrics[0].meta["kind"], "short_quota");
        assert_eq!(activity.usage_metrics[0].meta["remainingPercent"], 23.0);
        assert!(activity.usage_metrics[0].detail.contains("77% used"));
        assert!(activity.usage_metrics[0].detail.contains("5h"));
        assert_eq!(activity.usage_metrics[1].id_suffix, "quota-week");
        assert_eq!(activity.usage_metrics[1].label, "本周额度");
        assert_eq!(activity.usage_metrics[1].value, "88%");
        assert_eq!(activity.usage_metrics[1].status, "success");
        assert_eq!(activity.usage_metrics[1].meta["kind"], "weekly_quota");
    }

    #[test]
    fn codex_usage_summary_uses_cumulative_delta_not_reported_total_tokens() {
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000);
        let recent = 1_800_000 - 60;
        let content = [
            format!(r#"{{"timestamp":{},"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":100,"total_tokens":1100}}}}}}}}"#, recent),
            format!(r#"{{"timestamp":{},"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":1500,"cached_input_tokens":700,"output_tokens":180,"total_tokens":1680}}}}}}}}"#, recent + 10),
        ]
        .join("\n");
        let mut summary = UsageSummaryState::default();
        codex_usage_summary_in_lines(&content, &mut summary, now, now);

        assert_eq!(summary.total_24h.input, 1500);
        assert_eq!(summary.total_24h.cache_read, 700);
        assert_eq!(summary.total_24h.output, 180);
        assert_eq!(summary.total_24h.real_total(true), 1680);
        assert_eq!(summary.total_7d.real_total(true), 1680);
        assert_eq!(summary.last.unwrap().input, 500);
        assert_eq!(summary.last.unwrap().cache_read, 300);
        assert_eq!(summary.last.unwrap().output, 80);
        assert_eq!(summary.last.unwrap().real_total(true), 580);

        let metric = usage_summary_metric(
            "codex",
            "tokens-24h",
            "24小时用量",
            "total_24h_tokens",
            summary.total_24h,
            Some(USAGE_SUMMARY_24H_SECS),
            false,
        )
        .unwrap();
        assert_eq!(metric.value, "1.7K");
        assert_eq!(metric.detail, "in 800 · out 180 · cache 700");
        assert_eq!(metric.meta["source"], "session_delta");
        assert_eq!(metric.meta["windowSeconds"], USAGE_SUMMARY_24H_SECS);
    }

    #[test]
    fn codex_usage_summary_keeps_older_usage_in_seven_day_window_only() {
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000);
        let older_than_24h = 2_000_000 - USAGE_SUMMARY_24H_SECS - 60;
        let content = format!(
            r#"{{"timestamp":{},"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":100,"total_tokens":1100}}}}}}}}"#,
            older_than_24h
        );
        let mut summary = UsageSummaryState::default();
        codex_usage_summary_in_lines(&content, &mut summary, now, now);

        assert_eq!(summary.total_24h.real_total(true), 0);
        assert_eq!(summary.total_7d.real_total(true), 1100);
        assert_eq!(summary.last.unwrap().real_total(true), 1100);
    }

    #[test]
    fn usage_summary_metric_can_show_empty_supported_windows() {
        let metric = usage_summary_metric(
            "claude",
            "tokens-24h",
            "24小时用量",
            "total_24h_tokens",
            UsageTotals::default(),
            Some(USAGE_SUMMARY_24H_SECS),
            true,
        )
        .unwrap();

        assert_eq!(metric.value, "0");
        assert_eq!(metric.meta["kind"], "total_24h_tokens");
        assert_eq!(metric.meta["totalTokens"], 0);
    }

    #[test]
    fn latest_usage_activity_ignores_token_count_without_quota_metrics() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("agent-pet-usage-{}.jsonl", unique));
        let content = [
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#,
        ]
        .join("\n");
        fs::write(&path, content).unwrap();

        let activity = latest_usage_activity_in_file(&path, codex_line_to_activity).unwrap();
        fs::remove_file(&path).unwrap();

        assert!(activity.is_none());
    }

    #[test]
    fn claude_usage_summary_deduplicates_assistant_message_updates() {
        let content = [
            r#"{"type":"assistant","message":{"id":"msg_1","model":"claude-sonnet","usage":{"input_tokens":3,"cache_read_input_tokens":5000,"cache_creation_input_tokens":10000,"output_tokens":26}}}"#,
            r#"{"type":"assistant","message":{"id":"msg_1","model":"claude-sonnet","usage":{"input_tokens":3,"cache_read_input_tokens":5000,"cache_creation_input_tokens":10000,"output_tokens":150},"stop_reason":"end_turn"}}"#,
            r#"{"type":"assistant","message":{"id":"msg_2","model":"claude-sonnet","usage":{"input_tokens":10,"cache_read_input_tokens":20,"output_tokens":5},"stop_reason":"end_turn"}}"#,
        ]
        .join("\n");
        let mut summary = UsageSummaryState::default();
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000);
        claude_usage_summary_in_lines(&content, &mut summary, now, now);

        assert_eq!(summary.total_24h.input, 13);
        assert_eq!(summary.total_24h.cache_read, 5020);
        assert_eq!(summary.total_24h.cache_write, 10000);
        assert_eq!(summary.total_24h.output, 155);
        assert_eq!(summary.total_24h.real_total(false), 15188);
        assert_eq!(summary.total_7d.real_total(false), 15188);
        assert_eq!(summary.last.unwrap().input, 10);
        assert_eq!(summary.last.unwrap().real_total(false), 35);
    }

    #[test]
    fn openclaw_usage_summary_reads_assistant_usage_messages() {
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000);
        let content = r#"{"type":"message","id":"m1","timestamp":1799999000000,"message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"stop","usage":{"input":100,"output":20,"cacheRead":40,"cacheWrite":5}}}"#;
        let mut summary = UsageSummaryState::default();
        openclaw_usage_summary_in_lines(content, &mut summary, now, now);

        assert_eq!(summary.total_24h.input, 100);
        assert_eq!(summary.total_24h.output, 20);
        assert_eq!(summary.total_24h.cache_read, 40);
        assert_eq!(summary.total_24h.cache_write, 5);
    }

    #[test]
    fn hermes_usage_summary_reads_json_session_messages() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("agent-pet-hermes-{}.json", unique));
        fs::write(
            &path,
            r#"{"session_id":"s1","last_updated":"2026-03-13T12:47:12Z","messages":[{"role":"assistant","id":"a1","usage":{"prompt_tokens":1000,"completion_tokens":250,"cached_tokens":125,"total_tokens":1375}}]}"#,
        )
        .unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_773_490_000);
        let summary = hermes_usage_summary_in_file(&path, now).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(summary.total_24h.input, 1000);
        assert_eq!(summary.total_24h.output, 250);
        assert_eq!(summary.total_24h.cache_read, 125);
        assert_eq!(summary.total_24h.real_total(false), 1375);
    }

    #[test]
    fn opencode_usage_from_message_row_matches_cc_switch_tokens() {
        let row = OpencodeUsageRow {
            id: "msg_1".to_string(),
            session_id: "ses_1".to_string(),
            data: serde_json::json!({
                "role": "assistant",
                "time": {"created": 1779755333700i64, "completed": 1779755350639i64},
                "tokens": {
                    "total": 56554,
                    "input": 3272,
                    "output": 383,
                    "reasoning": 419,
                    "cache": {"write": 0, "read": 52480}
                }
            }),
        };

        let (message_id, usage, timestamp) = opencode_usage_from_message_row(&row).unwrap();
        assert_eq!(message_id, "ses_1:msg_1");
        assert_eq!(usage.input, 3272);
        assert_eq!(usage.output, 802);
        assert_eq!(usage.cache_read, 52480);
        assert_eq!(usage.cache_write, 0);
        assert!(timestamp.is_some());
    }

    #[test]
    fn gemini_session_usage_counts_thoughts_as_output_for_antigravity() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("session-agent-pet-{}.json", unique));
        fs::write(
            &path,
            r#"{"sessionId":"g1","lastUpdated":"2026-03-13T12:47:12Z","messages":[{"id":"m1","type":"gemini","timestamp":"2026-03-13T12:47:12Z","tokens":{"input":8522,"output":29,"cached":3138,"thoughts":405}}]}"#,
        )
        .unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_773_490_000);
        let summary = gemini_session_usage_summary_in_file(&path, now, now).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(summary.total_24h.input, 8522);
        assert_eq!(summary.total_24h.output, 434);
        assert_eq!(summary.total_24h.cache_read, 3138);
        assert_eq!(summary.total_24h.real_total(true), 8956);
    }

    #[test]
    fn extracts_openclaw_assistant_message_text_for_bubble() {
        let line = r#"{"type":"message","id":"7d33653f","parentId":"4541d9a1","timestamp":"2026-03-13T12:47:12.949Z","message":{"role":"assistant","content":[{"type":"text","text":"It is a football player."}],"stopReason":"stop","api":"ollama","provider":"ollama","model":"glm-5:cloud","timestamp":1773406032947}}"#;
        let activity = openclaw_line_to_activity(line).unwrap();
        assert_eq!(activity.message_type, MSG_NEW_MESSAGE);
        assert_eq!(
            activity.bubble_text.as_deref(),
            Some("It is a football player.")
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
    fn extracts_openclaw_usage_metric_from_real_usage_fields() {
        let line = r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Done."}],"stopReason":"stop","usage":{"input":1200,"output":345,"cacheRead":600,"cacheWrite":0,"totalTokens":2145,"cost":{"total":0.0123}}}}"#;
        let activity = openclaw_line_to_activity(line).unwrap();
        let metric = activity_usage_metric(
            "openclaw",
            "tokens",
            "Tokens",
            "usage_tokens",
            activity.usage.as_ref(),
        )
        .unwrap();

        assert_eq!(metric.id, "openclaw-tokens");
        assert_eq!(metric.source_label.as_deref(), Some("OpenClaw"));
        assert_eq!(metric.label, "Tokens");
        assert_eq!(metric.value, "$0.0123");
        assert_eq!(metric.detail, "in 1.2K · out 345 · cache 600");
    }

    #[test]
    fn extracts_usage_from_snake_case_fields_without_cost() {
        let value = serde_json::json!({
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 250,
                "cached_tokens": 125,
                "total_tokens": 1375
            }
        });
        let metric = activity_usage_metric(
            "hermes",
            "tokens",
            "Tokens",
            "usage_tokens",
            extract_usage(&value).as_ref(),
        )
        .unwrap();

        assert_eq!(metric.value, "1.4K");
        assert_eq!(metric.detail, "in 1.0K · out 250 · cache 125");
    }

    #[test]
    fn extracts_claude_cache_creation_and_read_usage_fields() {
        let value = serde_json::json!({
            "message": {
                "usage": {
                    "input_tokens": 1529,
                    "cache_creation_input_tokens": 952,
                    "cache_read_input_tokens": 17462,
                    "output_tokens": 75
                }
            }
        });
        let usage = extract_usage(value.get("message").unwrap()).unwrap();
        assert_eq!(usage.input_tokens, Some(1529));
        assert_eq!(usage.output_tokens, Some(75));
        assert_eq!(usage.cache_read_tokens, Some(17462));
        assert_eq!(usage.cache_write_tokens, Some(952));

        let metric =
            activity_usage_metric("claude", "tokens", "Tokens", "usage_tokens", Some(&usage))
                .unwrap();
        assert_eq!(metric.value, "20.0K");
        assert_eq!(metric.detail, "in 1.5K · out 75 · cache 17.5K");
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
