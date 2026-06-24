mod keep_awake;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::HashMap,
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration as StdDuration, UNIX_EPOCH},
};
use tauri::Manager;
use thiserror::Error;
use uuid::Uuid;

const SUPPORTED_RATIOS: &[&str] = &["1:1", "3:4", "16:9", "4:3", "9:16", "21:9"];
const SUPPORTED_MODELS: &[&str] = &["seedance2.0", "seedance2.0fast"];
const MAX_IMAGES: usize = 9;
const MAX_AUDIO: usize = 3;
/// 自动查询最长等待时间（4小时），超过后停止自动查询，改为手动
const MAX_WAIT_HOURS: i64 = 4;
const MAX_NO_REMOTE_QUEUE_INFO_MINUTES: i64 = 5;
/// 5xx 服务器错误自动重试上限次数
const MAX_SERVER_ERROR_RETRIES: u32 = 2;
/// 上传 EOF / connection reset 等提交阶段瞬时错误，最多自动重试次数。
const MAX_TRANSIENT_SUBMIT_RETRIES: u32 = 3;
/// 队列空闲后，因提交瞬时错误失败的任务额外补试次数。
const MAX_IDLE_TRANSIENT_FAILED_RETRIES: u32 = 3;
/// 队列空闲后，因并发限制被旧策略标失败的任务额外补试次数。
const MAX_IDLE_CONCURRENCY_FAILED_RETRIES: u32 = 3;
/// 只自动捞起近期并发限制失败，避免把几周前的历史失败任务重新提交。
const MAX_CONCURRENCY_FAILURE_RECOVERY_HOURS: i64 = 24;
const IMAGE_SUBMIT_TIMEOUT_SECS: u64 = 300;
const IMAGE_SUBMIT_CONNECT_TIMEOUT_SECS: u64 = 300;
const SCHEDULER_TICK_INTERVAL_SECS: u64 = 30;
static PROCESS_QUEUE_RUNNING: AtomicBool = AtomicBool::new(false);

/// 退避间隔阶梯（秒），index = consecutive_no_result_queries 的次数映射
const BACKOFF_INTERVALS_SECS: &[u64] = &[0, 60, 120, 300, 600];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    pub name: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub stored_path: String,
    pub source_path: String,
    pub mime: String,
    pub size_bytes: u64,
    pub duration_seconds: Option<f64>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub content_hash: Option<String>,
}

impl Asset {
    pub fn from_path(
        path: &Path,
        assets_dir: &Path,
        name: Option<String>,
    ) -> Result<Self, SchedulerError> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let kind = match extension.as_str() {
            "png" | "jpg" | "jpeg" | "webp" => AssetKind::Image,
            "mp3" | "wav" | "m4a" | "aac" => AssetKind::Audio,
            "mp4" | "mov" | "webm" | "mkv" => return Err(SchedulerError::UnsupportedVideoAsset),
            _ => return Err(SchedulerError::UnsupportedAssetType(extension)),
        };
        let id = format!("asset_{}", Uuid::new_v4().simple());
        fs::create_dir_all(assets_dir).map_err(|error| SchedulerError::Io(error.to_string()))?;
        let stored_path = assets_dir.join(format!("{id}.{extension}"));
        fs::copy(path, &stored_path).map_err(|error| SchedulerError::Io(error.to_string()))?;
        let metadata =
            fs::metadata(&stored_path).map_err(|error| SchedulerError::Io(error.to_string()))?;
        let file_name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("素材")
            .to_string();
        let normalized_name = sanitize_asset_name_for_mention(&file_name);
        let mime = match kind {
            AssetKind::Image => match extension.as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                _ => "image/png",
            },
            AssetKind::Audio => match extension.as_str() {
                "wav" => "audio/wav",
                "m4a" => "audio/mp4",
                "aac" => "audio/aac",
                _ => "audio/mpeg",
            },
        };
        Ok(Self {
            id,
            kind,
            name: name
                .map(|value| sanitize_asset_name_for_mention(&value))
                .unwrap_or(normalized_name),
            aliases: vec![],
            tags: vec![],
            stored_path: stored_path.to_string_lossy().to_string(),
            source_path: path.to_string_lossy().to_string(),
            mime: mime.to_string(),
            size_bytes: metadata.len(),
            duration_seconds: None,
            created_at: now_rfc3339(),
            content_hash: None,
        })
    }
}

fn sanitize_asset_name_for_mention(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "素材".to_string();
    }
    let mut normalized = String::with_capacity(trimmed.len());
    let mut last_was_underscore = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !last_was_underscore {
                normalized.push('_');
                last_was_underscore = true;
            }
        } else {
            normalized.push(ch);
            last_was_underscore = false;
        }
    }
    normalized.trim_matches('_').to_string()
}

fn rebuild_asset_hash_index(data: &mut AppData) {
    data.asset_hash_index.clear();
    for a in &data.assets {
        if let Some(ref h) = a.content_hash {
            data.asset_hash_index
                .entry(h.clone())
                .or_insert(a.id.clone());
        }
    }
}

/// 清理超过 `days` 天的临时图片（tagged `temp_image`），同时删除本地文件。
pub fn purge_expired_temp_images(data: &mut AppData, days: i64) {
    let cutoff = Utc::now() - Duration::days(days);
    let mut expired_paths = Vec::new();
    let mut expired_hashes = Vec::new();
    data.assets.retain(|a| {
        if a.tags.contains(&"temp_image".to_string()) {
            if let Ok(dt) = DateTime::parse_from_rfc3339(&a.created_at) {
                if dt < cutoff {
                    expired_paths.push(a.stored_path.clone());
                    if let Some(ref h) = a.content_hash {
                        expired_hashes.push(h.clone());
                    }
                    return false;
                }
            }
        }
        true
    });
    for hash in expired_hashes {
        data.asset_hash_index.remove(&hash);
    }
    for path in expired_paths {
        let _ = fs::remove_file(&path);
    }
}

pub fn save_clipboard_image_asset(
    data: &mut AppData,
    assets_dir: &Path,
    input: ClipboardImageInput,
) -> Result<Asset, SchedulerError> {
    purge_expired_temp_images(data, 10);

    // 计算内容哈希，复用已有素材
    let content_hash = {
        let mut hasher = Sha256::new();
        hasher.update(&input.bytes);
        format!("{:x}", hasher.finalize())
    };
    if let Some(existing_id) = data.asset_hash_index.get(&content_hash) {
        if let Some(existing) = data.assets.iter().find(|a| a.id == *existing_id) {
            return Ok(existing.clone());
        }
    }

    let extension = clipboard_image_extension(&input.file_name, &input.mime)?;
    if input.bytes.is_empty() {
        return Err(SchedulerError::Io("剪贴板图片为空".to_string()));
    }
    fs::create_dir_all(assets_dir).map_err(|error| SchedulerError::Io(error.to_string()))?;
    let id = format!("asset_{}", Uuid::new_v4().simple());
    let stored_path = assets_dir.join(format!("{id}.{extension}"));
    fs::write(&stored_path, &input.bytes).map_err(|error| SchedulerError::Io(error.to_string()))?;
    let metadata =
        fs::metadata(&stored_path).map_err(|error| SchedulerError::Io(error.to_string()))?;
    let mime = if input.mime.trim().is_empty() {
        match extension.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => "image/png",
        }
        .to_string()
    } else {
        input.mime
    };
    let asset = Asset {
        id,
        kind: AssetKind::Image,
        name: "粘贴图片".to_string(),
        aliases: vec![],
        tags: vec![
            "clipboard".to_string(),
            "temporary".to_string(),
            "temp_image".to_string(),
        ],
        stored_path: stored_path.to_string_lossy().to_string(),
        source_path: "clipboard".to_string(),
        mime,
        size_bytes: metadata.len(),
        duration_seconds: None,
        created_at: now_rfc3339(),
        content_hash: Some(content_hash.clone()),
    };
    data.asset_hash_index.insert(content_hash, asset.id.clone());
    data.assets.push(asset.clone());
    Ok(asset)
}

pub fn paste_system_clipboard_image_asset(
    data: &mut AppData,
    assets_dir: &Path,
) -> Result<Asset, SchedulerError> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| SchedulerError::Io(format!("无法读取系统剪贴板：{error}")))?;
    let image = clipboard
        .get_image()
        .map_err(|error| SchedulerError::Io(format!("剪贴板没有图片：{error}")))?;
    let png_bytes = encode_rgba_png(
        image.width as u32,
        image.height as u32,
        image.bytes.as_ref(),
    )?;
    save_clipboard_image_asset(
        data,
        assets_dir,
        ClipboardImageInput {
            file_name: "clipboard.png".to_string(),
            mime: "image/png".to_string(),
            bytes: png_bytes,
        },
    )
}

fn encode_rgba_png(width: u32, height: u32, bytes: &[u8]) -> Result<Vec<u8>, SchedulerError> {
    let expected_len = width as usize * height as usize * 4;
    if bytes.len() != expected_len {
        return Err(SchedulerError::Io("剪贴板图片像素数据异常".to_string()));
    }
    let mut encoded = Vec::new();
    {
        let mut cursor = Cursor::new(&mut encoded);
        let mut encoder = png::Encoder::new(&mut cursor, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| SchedulerError::Io(error.to_string()))?;
        writer
            .write_image_data(bytes)
            .map_err(|error| SchedulerError::Io(error.to_string()))?;
    }
    Ok(encoded)
}

fn decode_png_rgba(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), SchedulerError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| SchedulerError::Io(error.to_string()))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|error| SchedulerError::Io(error.to_string()))?;
    let data = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data
            .chunks_exact(3)
            .flat_map(|chunk| [chunk[0], chunk[1], chunk[2], 255])
            .collect(),
        png::ColorType::Grayscale => data
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => data
            .chunks_exact(2)
            .flat_map(|chunk| [chunk[0], chunk[0], chunk[0], chunk[1]])
            .collect(),
        png::ColorType::Indexed => {
            return Err(SchedulerError::Io("暂不支持索引色 PNG".to_string()));
        }
    };
    Ok((info.width as usize, info.height as usize, rgba))
}

fn clipboard_image_extension(file_name: &str, mime: &str) -> Result<String, SchedulerError> {
    let file_ext = Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let ext = if ["png", "jpg", "jpeg", "webp"].contains(&file_ext.as_str()) {
        file_ext
    } else {
        match mime {
            "image/jpeg" => "jpg".to_string(),
            "image/webp" => "webp".to_string(),
            "image/png" | "" => "png".to_string(),
            value => return Err(SchedulerError::UnsupportedAssetType(value.to_string())),
        }
    };
    Ok(ext)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub description: String,
    pub asset_ids: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoParams {
    pub model_version: String,
    pub ratio: String,
    pub duration: u8,
    pub video_resolution: String,
}

impl Default for VideoParams {
    fn default() -> Self {
        Self {
            model_version: "seedance2.0".to_string(),
            ratio: "9:16".to_string(),
            duration: 15,
            video_resolution: "720p".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDraft {
    #[serde(default)]
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    pub image_asset_ids: Vec<String>,
    #[serde(default)]
    pub audio_asset_ids: Vec<String>,
    pub role_ids: Vec<String>,
    pub manual_mention_ids: Vec<String>,
    #[serde(default, alias = "auto_match_assets")]
    pub auto_match_roles: bool,
    pub params: VideoParams,
    #[serde(default)]
    pub scheduled_at: Option<String>,
    #[serde(default)]
    pub temp_image_asset_ids: Vec<String>,
    #[serde(default)]
    pub temp_image_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TaskExecutionInputSnapshot {
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub image_asset_ids: Vec<String>,
    #[serde(default)]
    pub audio_asset_ids: Vec<String>,
    #[serde(default)]
    pub role_ids: Vec<String>,
    #[serde(default)]
    pub manual_mention_ids: Vec<String>,
    #[serde(default)]
    pub auto_match_roles: bool,
    #[serde(default)]
    pub params: VideoParams,
    #[serde(default)]
    pub temp_image_asset_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskExecutionRecord {
    pub id: String,
    #[serde(default)]
    pub submit_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub finished_at: String,
    #[serde(default)]
    pub input_snapshot: TaskExecutionInputSnapshot,
    #[serde(default)]
    pub command_preview: Vec<String>,
    #[serde(default, alias = "attempts")]
    pub query_records: Vec<TaskAttempt>,
    #[serde(default)]
    pub result_paths: Vec<String>,
    #[serde(default)]
    pub result_urls: Vec<String>,
    #[serde(default)]
    pub error_kind: String,
    #[serde(default)]
    pub error_detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskAttempt {
    pub id: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub command_preview: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub error_kind: String,
    #[serde(default)]
    pub duration_seconds: f64,
    #[serde(default)]
    pub error_detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub image_asset_ids: Vec<String>,
    pub audio_asset_ids: Vec<String>,
    pub role_ids: Vec<String>,
    pub manual_mention_ids: Vec<String>,
    #[serde(default, alias = "auto_match_assets")]
    pub auto_match_roles: bool,
    pub params: VideoParams,
    pub status: String,
    pub scheduled_at: Option<String>,
    pub next_run_at: Option<String>,
    pub submit_id: String,
    pub attempt_count: u32,
    pub concurrency_retry_count: u32,
    pub last_error: String,
    pub command_preview: Vec<String>,
    #[serde(default)]
    pub attempts: Vec<TaskAttempt>,
    #[serde(default)]
    pub result_paths: Vec<String>,
    #[serde(default)]
    pub result_urls: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub finished_at: String,
    #[serde(default)]
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub queue_info: Option<QueueInfo>,
    #[serde(default)]
    pub temp_image_asset_ids: Vec<String>,
    #[serde(default)]
    pub temp_image_paths: Vec<String>,
    #[serde(default)]
    pub execution_records: Vec<TaskExecutionRecord>,
    /// 上次自动查询时间，用于退避计算
    #[serde(default)]
    pub last_auto_query_at: Option<String>,
    /// 是否因超过 MAX_WAIT_HOURS 而停止自动查询
    #[serde(default)]
    pub auto_query_stopped: bool,
    /// 连续无结果查询次数，用于退避阶梯
    #[serde(default)]
    pub consecutive_no_result_queries: u32,
    /// 5xx 服务器错误重试次数（上限 MAX_SERVER_ERROR_RETRIES，超过后标 failed）
    #[serde(default)]
    pub server_error_retry_count: u32,
    #[serde(default = "default_planned_submit_count")]
    pub planned_submit_count: u32,
}

impl From<TaskDraft> for ScheduledTask {
    fn from(value: TaskDraft) -> Self {
        let now = now_rfc3339();
        let scheduled_at = value.scheduled_at;
        let status = if scheduled_at.is_some() {
            "scheduled"
        } else {
            "queued"
        };
        Self {
            id: format!("task_{}", Uuid::new_v4().simple()),
            title: normalize_task_title(&value.title, &value.prompt),
            prompt: value.prompt,
            image_asset_ids: value.image_asset_ids,
            audio_asset_ids: value.audio_asset_ids,
            role_ids: value.role_ids,
            manual_mention_ids: value.manual_mention_ids,
            temp_image_asset_ids: value.temp_image_asset_ids,
            temp_image_paths: value.temp_image_paths,
            auto_match_roles: value.auto_match_roles,
            params: value.params,
            status: status.to_string(),
            scheduled_at: scheduled_at.clone(),
            next_run_at: scheduled_at,
            submit_id: String::new(),
            attempt_count: 0,
            concurrency_retry_count: 0,
            last_error: String::new(),
            command_preview: vec![],
            attempts: vec![],
            result_paths: vec![],
            result_urls: vec![],
            created_at: now.clone(),
            updated_at: now,
            finished_at: String::new(),
            submitted_at: None,
            queue_info: None,
            execution_records: vec![],
            last_auto_query_at: None,
            auto_query_stopped: false,
            consecutive_no_result_queries: 0,
            server_error_retry_count: 0,
            planned_submit_count: default_planned_submit_count(),
        }
    }
}

fn default_planned_submit_count() -> u32 {
    1
}

fn normalize_planned_submit_count(value: u32) -> u32 {
    value.clamp(1, 10)
}

fn normalize_task_title(title: &str, prompt: &str) -> String {
    let explicit = title.trim();
    if !explicit.is_empty() {
        return explicit.chars().take(30).collect();
    }
    let cleaned = prompt
        .chars()
        .filter(|ch| !matches!(ch, '@' | '#' | '*' | '"' | '\'' | '“' | '”' | '‘' | '’'))
        .collect::<String>()
        .split(|ch: char| {
            ch.is_whitespace() || "，。,.!?！？、；;：:（）()[]【】\n\r\t".contains(ch)
        })
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("");
    let title = cleaned.chars().take(7).collect::<String>();
    if title.is_empty() {
        "未命名任务".to_string()
    } else {
        title
    }
}

// ── Structured Log System ──────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Success,
    Debug,
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogSource {
    CLI,
    Scheduler,
    Worker,
    RetryPolicy,
    System,
    AI,
    ImageGen,
    Asset,
    Role,
    Settings,
}

impl Default for LogSource {
    fn default() -> Self {
        LogSource::System
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: String,
    pub timestamp: String,
    #[serde(default)]
    pub level: LogLevel,
    #[serde(default)]
    pub source: LogSource,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub event_type: String,
    pub message: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub task_title: Option<String>,
    #[serde(default)]
    pub submit_id: Option<String>,
    #[serde(default)]
    pub execution_record_id: Option<String>,
    #[serde(default)]
    pub error_detail: Option<String>,
    #[serde(default)]
    pub raw_output: Option<String>,
    #[serde(default)]
    pub stdout: Option<String>,
    #[serde(default)]
    pub stderr: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    /// Legacy compatibility: original string from old Vec<String> logs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_string: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LogEntryDraft {
    pub level: LogLevel,
    pub source: LogSource,
    pub category: String,
    pub event_type: String,
    pub message: String,
    pub detail: String,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub submit_id: Option<String>,
    pub execution_record_id: Option<String>,
    pub error_detail: Option<String>,
    pub raw_output: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub module: Option<String>,
}

/// Custom deserializer: accept both old `Vec<String>` and new `Vec<LogEntry>`.
fn deserialize_logs_compat<'de, D>(deserializer: D) -> Result<Vec<LogEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{SeqAccess, Visitor};
    use std::fmt;

    struct LogsVisitor;

    impl<'de> Visitor<'de> for LogsVisitor {
        type Value = Vec<LogEntry>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a sequence of log entries (string or object)")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Vec<LogEntry>, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut logs = Vec::new();
            let mut index: usize = 0;
            while let Some(value) = seq.next_element::<serde_json::Value>()? {
                match value {
                    serde_json::Value::String(s) => {
                        logs.push(LogEntry {
                            id: format!("legacy_{index}"),
                            timestamp: String::new(),
                            level: LogLevel::Info,
                            source: LogSource::System,
                            category: "system".to_string(),
                            event_type: "legacy_string_log".to_string(),
                            message: s.clone(),
                            detail: String::new(),
                            task_id: None,
                            task_title: None,
                            submit_id: None,
                            execution_record_id: None,
                            error_detail: None,
                            raw_output: None,
                            stdout: None,
                            stderr: None,
                            module: None,
                            legacy_string: Some(s),
                        });
                    }
                    serde_json::Value::Object(_) => {
                        let entry: LogEntry =
                            serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                        logs.push(entry);
                    }
                    other => {
                        let s = other.to_string();
                        logs.push(LogEntry {
                            id: format!("legacy_{index}"),
                            timestamp: String::new(),
                            level: LogLevel::Info,
                            source: LogSource::System,
                            category: "system".to_string(),
                            event_type: "legacy_string_log".to_string(),
                            message: s.clone(),
                            detail: String::new(),
                            task_id: None,
                            task_title: None,
                            submit_id: None,
                            execution_record_id: None,
                            error_detail: None,
                            raw_output: None,
                            stdout: None,
                            stderr: None,
                            module: None,
                            legacy_string: Some(s),
                        });
                    }
                }
                index += 1;
            }
            Ok(logs)
        }
    }

    deserializer.deserialize_seq(LogsVisitor)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppData {
    pub settings: SchedulerSettings,
    pub assets: Vec<Asset>,
    pub roles: Vec<Role>,
    pub tasks: Vec<ScheduledTask>,
    #[serde(default, deserialize_with = "deserialize_logs_compat")]
    pub logs: Vec<LogEntry>,
    #[serde(default)]
    pub imagegen_history: Vec<ImageGenHistoryItem>,
    #[serde(skip, default = "HashMap::new")]
    pub asset_hash_index: HashMap<String, String>,
}

impl Default for AppData {
    fn default() -> Self {
        Self {
            settings: SchedulerSettings::default(),
            assets: vec![],
            roles: vec![],
            tasks: vec![],
            logs: vec![],
            imagegen_history: vec![],
            asset_hash_index: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenHistoryItem {
    pub id: String,
    pub prompt: String,
    pub size: String,
    #[serde(default)]
    pub stored_path: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub reference_asset_ids: Vec<String>,
    pub created_at: String,
    /// "pending" | "completed" | "failed"
    #[serde(default = "default_status_completed")]
    pub status: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

fn default_status_completed() -> String {
    "completed".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CliStatus {
    pub available: bool,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HostPlatform {
    pub os: String,
    pub arch: String,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImportAssetInput {
    pub path: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpVideoTaskDefaults {
    #[serde(default, alias = "aspectRatio")]
    pub orientation: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub duration: Option<u8>,
    #[serde(default, alias = "videoResolution")]
    pub video_resolution: Option<String>,
    #[serde(default, alias = "plannedSubmitCount")]
    pub planned_submit_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpVideoTaskInput {
    #[serde(default)]
    pub title: String,
    pub prompt: String,
    #[serde(default, alias = "imagePaths", alias = "images", alias = "imagePath")]
    pub image_paths: Vec<String>,
    #[serde(default, alias = "audioPaths", alias = "audios", alias = "audioPath")]
    pub audio_paths: Vec<String>,
    #[serde(
        default,
        alias = "startAt",
        alias = "scheduled_at",
        alias = "scheduledAt"
    )]
    pub start_at: Option<String>,
    #[serde(default, alias = "aspectRatio")]
    pub orientation: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub duration: Option<u8>,
    #[serde(default, alias = "videoResolution")]
    pub video_resolution: Option<String>,
    #[serde(default, alias = "plannedSubmitCount")]
    pub planned_submit_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpQueueVideosInput {
    #[serde(
        default,
        alias = "startAt",
        alias = "scheduled_at",
        alias = "scheduledAt"
    )]
    pub start_at: Option<String>,
    #[serde(default)]
    pub defaults: McpVideoTaskDefaults,
    #[serde(default, alias = "tasks", alias = "videos")]
    pub items: Vec<McpVideoTaskInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpQueuedVideoTask {
    pub task: ScheduledTask,
    pub imported_assets: Vec<Asset>,
}

fn json_string_array(value: &JsonValue) -> Vec<String> {
    match value {
        JsonValue::String(item) => json_string_value_array(item),
        JsonValue::Array(items) => items
            .iter()
            .flat_map(json_string_array)
            .filter(|item| !item.trim().is_empty())
            .collect(),
        JsonValue::Object(map) => json_object_string_array(map),
        _ => vec![],
    }
}

fn json_string_value_array(item: &str) -> Vec<String> {
    let trimmed = item.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    if let Ok(parsed) = serde_json::from_str::<JsonValue>(trimmed) {
        let parsed_values = json_string_array(&parsed);
        if !parsed_values.is_empty() {
            return parsed_values;
        }
    }
    vec![normalize_mcp_path_string(trimmed)]
}

fn json_object_string_array(map: &JsonMap<String, JsonValue>) -> Vec<String> {
    const DIRECT_KEYS: &[&str] = &[
        "path",
        "file",
        "file_path",
        "filePath",
        "local_path",
        "localPath",
        "absolute_path",
        "absolutePath",
        "resolved_path",
        "resolvedPath",
        "stored_path",
        "storedPath",
        "source_path",
        "sourcePath",
        "uri",
        "url",
        "href",
        "text",
        "value",
        "content",
    ];
    const NESTED_KEYS: &[&str] = &[
        "paths", "files", "items", "values", "data", "resource", "asset", "input", "image", "audio",
    ];

    for key in DIRECT_KEYS {
        if let Some(value) = map.get(*key) {
            let values = json_string_array(value);
            if !values.is_empty() {
                return values;
            }
        }
    }
    for key in NESTED_KEYS {
        if let Some(value) = map.get(*key) {
            let values = json_string_array(value);
            if !values.is_empty() {
                return values;
            }
        }
    }

    let mut numeric_keys = map.keys().collect::<Vec<_>>();
    numeric_keys.sort();
    if numeric_keys
        .iter()
        .all(|key| key.chars().all(|ch| ch.is_ascii_digit()))
    {
        return numeric_keys
            .into_iter()
            .filter_map(|key| map.get(key))
            .flat_map(json_string_array)
            .collect();
    }

    vec![]
}

fn normalize_mcp_path_string(value: &str) -> String {
    let trimmed = value.trim();
    let Some(rest) = trimmed.strip_prefix("file://") else {
        return trimmed.to_string();
    };
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    let normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    percent_decode_path(&normalized)
}

fn percent_decode_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(output).unwrap_or_else(|_| value.to_string())
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn take_first_string_array(map: &mut JsonMap<String, JsonValue>, keys: &[&str]) -> Vec<String> {
    let mut values = Vec::new();
    for key in keys {
        if let Some(value) = map.remove(*key) {
            for item in json_string_array(&value) {
                if !values.iter().any(|existing| existing == &item) {
                    values.push(item);
                }
            }
        }
    }
    values
}

fn normalize_mcp_video_task_value(value: JsonValue) -> JsonValue {
    let JsonValue::Object(mut map) = value else {
        return value;
    };
    let image_paths = take_first_string_array(
        &mut map,
        &[
            "image_paths",
            "imagePaths",
            "images",
            "image",
            "imagePath",
            "image_path",
        ],
    );
    if !image_paths.is_empty() {
        map.insert(
            "image_paths".to_string(),
            JsonValue::Array(image_paths.into_iter().map(JsonValue::String).collect()),
        );
    }
    let audio_paths = take_first_string_array(
        &mut map,
        &[
            "audio_paths",
            "audioPaths",
            "audios",
            "audio",
            "audioPath",
            "audio_path",
        ],
    );
    if !audio_paths.is_empty() {
        map.insert(
            "audio_paths".to_string(),
            JsonValue::Array(audio_paths.into_iter().map(JsonValue::String).collect()),
        );
    }
    JsonValue::Object(map)
}

pub fn parse_mcp_video_task_input(value: JsonValue) -> Result<McpVideoTaskInput, SchedulerError> {
    serde_json::from_value(normalize_mcp_video_task_value(value))
        .map_err(|error| SchedulerError::Io(format!("MCP 参数解析失败：{error}")))
}

pub fn parse_mcp_queue_videos_input(
    value: JsonValue,
) -> Result<McpQueueVideosInput, SchedulerError> {
    let normalized = match value {
        JsonValue::Object(mut map) => {
            for key in ["items", "tasks", "videos"] {
                if let Some(JsonValue::Array(items)) = map.remove(key) {
                    let normalized_items = items
                        .into_iter()
                        .map(normalize_mcp_video_task_value)
                        .collect::<Vec<_>>();
                    map.insert("items".to_string(), JsonValue::Array(normalized_items));
                    break;
                }
            }
            JsonValue::Object(map)
        }
        other => other,
    };
    serde_json::from_value(normalized)
        .map_err(|error| SchedulerError::Io(format!("MCP 批量参数解析失败：{error}")))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClipboardImageInput {
    pub file_name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImportRoleMediaInput {
    pub role_id: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoveRoleMediaInput {
    pub role_id: String,
    pub asset_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoleInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub description: String,
    pub asset_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageModelConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_image_model_name")]
    pub model: String,
}

impl Default for ImageModelConfig {
    fn default() -> Self {
        Self {
            id: default_active_image_model_id(),
            name: "OpenAI 图片默认".to_string(),
            base_url: default_openai_base_url(),
            api_key: String::new(),
            model: default_image_model_name(),
        }
    }
}

fn default_image_model_name() -> String {
    "gpt-image-1".to_string()
}

fn default_image_model_configs() -> Vec<ImageModelConfig> {
    vec![ImageModelConfig::default()]
}

fn default_active_image_model_id() -> String {
    "default-image-openai".to_string()
}

fn normalize_image_model_config(config: &mut ImageModelConfig, index: usize) {
    if config.id.trim().is_empty() {
        config.id = if index == 0 {
            default_active_image_model_id()
        } else {
            format!("image-model-{}", index + 1)
        };
    }
    if config.name.trim().is_empty() {
        config.name = if index == 0 {
            "OpenAI 图片默认".to_string()
        } else {
            format!("图片模型 {}", index + 1)
        };
    }
    if config.base_url.trim().is_empty() {
        config.base_url = default_openai_base_url();
    }
    if config.model.trim().is_empty() {
        config.model = default_image_model_name();
    }
}

fn normalize_image_model_settings(settings: &mut SchedulerSettings) {
    let should_migrate_legacy = settings.image_model_config.as_ref().is_some_and(|legacy| {
        settings.image_model_configs.is_empty()
            || (settings.image_model_configs.len() == 1
                && settings.image_model_configs[0].id == default_active_image_model_id()
                && settings.image_model_configs[0].api_key.trim().is_empty()
                && (!legacy.api_key.trim().is_empty()
                    || legacy.base_url.trim() != default_openai_base_url()
                    || legacy.model.trim() != default_image_model_name()))
    });
    if should_migrate_legacy {
        if let Some(mut legacy) = settings.image_model_config.clone() {
            normalize_image_model_config(&mut legacy, 0);
            settings.image_model_configs = vec![legacy];
            settings.active_image_model_id = settings.image_model_configs[0].id.clone();
        }
    }
    if settings.image_model_configs.is_empty() {
        settings.image_model_configs = default_image_model_configs();
    }
    for (index, config) in settings.image_model_configs.iter_mut().enumerate() {
        normalize_image_model_config(config, index);
    }
    if settings.active_image_model_id.trim().is_empty()
        || !settings
            .image_model_configs
            .iter()
            .any(|config| config.id == settings.active_image_model_id)
    {
        settings.active_image_model_id = settings
            .image_model_configs
            .first()
            .map(|config| config.id.clone())
            .unwrap_or_else(default_active_image_model_id);
    }
    settings.image_model_config = active_image_model_config(settings).cloned();
}

fn active_image_model_config(settings: &SchedulerSettings) -> Option<&ImageModelConfig> {
    settings
        .image_model_configs
        .iter()
        .find(|config| config.id == settings.active_image_model_id)
        .or_else(|| settings.image_model_configs.first())
        .or(settings.image_model_config.as_ref())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateSettingsInput {
    pub concurrency_limit_policy: ConcurrencyLimitPolicy,
    pub concurrency_retry_delay_seconds: u64,
    pub concurrency_retry_max_attempts: u32,
    pub auto_query_enabled: bool,
    pub poll_interval_seconds: u64,
    pub log_retention_count: u32,
    pub mac_install_command: String,
    pub windows_install_command: String,
    #[serde(default = "default_ai_model_configs")]
    pub ai_model_configs: Vec<AiModelConfig>,
    #[serde(default = "default_active_ai_model_id")]
    pub active_ai_model_id: String,
    #[serde(default = "default_true")]
    pub prevent_sleep: bool,
    #[serde(default = "default_image_model_configs")]
    pub image_model_configs: Vec<ImageModelConfig>,
    #[serde(default = "default_active_image_model_id")]
    pub active_image_model_id: String,
    #[serde(default)]
    pub image_model_config: Option<ImageModelConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptMentionRewrite {
    pub original: String,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTaskInputs {
    pub image_paths: Vec<String>,
    pub audio_paths: Vec<String>,
    pub image_asset_ids: Vec<String>,
    pub audio_asset_ids: Vec<String>,
    pub manual_mention_ids: Vec<String>,
    pub matched_role_ids: Vec<String>,
    pub unresolved_mentions: Vec<String>,
    #[serde(default)]
    pub prompt_rewrites: Vec<PromptMentionRewrite>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchedulerError {
    #[error("找不到素材：{0}")]
    MissingAsset(String),
    #[error("找不到角色：{0}")]
    MissingRole(String),
    #[error("MVP 至少需要 1 个图片输入")]
    MissingImageInput,
    #[error("图片最多支持 {MAX_IMAGES} 个")]
    TooManyImages,
    #[error("音频最多支持 {MAX_AUDIO} 个")]
    TooManyAudio,
    #[error("ratio 不支持：{0}")]
    UnsupportedRatio(String),
    #[error("model_version 不支持：{0}")]
    UnsupportedModel(String),
    #[error("duration 必须在 4-15 秒之间")]
    UnsupportedDuration,
    #[error("video_resolution 仅支持 720p")]
    UnsupportedResolution,
    #[error("MVP 暂不支持视频素材")]
    UnsupportedVideoAsset,
    #[error("不支持的素材类型：{0}")]
    UnsupportedAssetType(String),
    #[error("IO 错误：{0}")]
    Io(String),
    #[error("计划时间不能是过去时间")]
    ScheduledAtInPast,
}

#[derive(Debug)]
pub struct AppStore {
    root_dir: PathBuf,
    data: Mutex<AppData>,
}

impl AppStore {
    pub fn load(root_dir: PathBuf) -> Self {
        let data = load_app_data_from_disk(&root_dir).unwrap_or_default();
        let store = Self {
            root_dir,
            data: Mutex::new(data),
        };
        store.compact_on_disk_if_oversized();
        store
    }

    /// 启动时一次性压实：当磁盘 `state.json` 明显大于规整（紧凑序列化 + 历史裁剪）后的数据时，
    /// 重写一次使旧的 pretty 格式 / 历史臃肿文件立即瘦身。压实后磁盘体积≈紧凑数据，不会重复触发。
    fn compact_on_disk_if_oversized(&self) {
        let path = self.root_dir.join("state.json");
        let Ok(meta) = fs::metadata(&path) else {
            return;
        };
        let data = self.data.lock().expect("store lock");
        let Ok(serialized) = serde_json::to_string(&*data) else {
            return;
        };
        // 留 10% 余量，避免等大文件的无谓重写。
        if meta.len() as usize > serialized.len() + serialized.len() / 10 {
            let _ = self.persist(&data);
        }
    }

    pub fn snapshot(&self) -> AppData {
        let mut data = self.data.lock().expect("store lock");
        if let Ok(latest) = load_app_data_from_disk(&self.root_dir) {
            *data = latest;
        }
        data.clone()
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root_dir.join("role-media")
    }

    pub fn imagegen_dir(&self) -> PathBuf {
        self.root_dir.join("imagegen")
    }

    pub fn mutate<F, T>(&self, mutate: F) -> Result<T, SchedulerError>
    where
        F: FnOnce(&mut AppData) -> Result<T, SchedulerError>,
    {
        let mut data = self.data.lock().expect("store lock");
        if let Ok(latest) = load_app_data_from_disk(&self.root_dir) {
            *data = latest;
        }
        let result = mutate(&mut data)?;
        self.persist(&data)?;
        Ok(result)
    }

    /// 廉价变更签名：仅 stat `state.json`（不读取/解析内容），返回 `体积:mtime毫秒`。
    /// 供前端轮询比对——签名不变即跳过整份状态拉取，空闲时彻底省去大文件读解析。
    /// 基于文件元数据，可捕获本进程与外部进程（如 MCP）的任何写入。
    pub fn state_signature(&self) -> String {
        let path = self.root_dir.join("state.json");
        match fs::metadata(&path) {
            Ok(meta) => {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                format!("{}:{}", meta.len(), mtime)
            }
            Err(_) => "0:0".to_string(),
        }
    }

    fn persist(&self, data: &AppData) -> Result<(), SchedulerError> {
        fs::create_dir_all(&self.root_dir)
            .map_err(|error| SchedulerError::Io(error.to_string()))?;
        let state_path = self.root_dir.join("state.json");
        let temp_path = self.root_dir.join("state.json.tmp");
        // 紧凑序列化：state.json 为机器状态文件，pretty 缩进会让磁盘体积近乎翻倍，
        // 加重每次读/写/解析的 I/O 与 CPU。compact 不改变可读回的数据。
        let content =
            serde_json::to_string(data).map_err(|error| SchedulerError::Io(error.to_string()))?;
        fs::write(&temp_path, content).map_err(|error| SchedulerError::Io(error.to_string()))?;
        fs::rename(temp_path, state_path).map_err(|error| SchedulerError::Io(error.to_string()))?;
        Ok(())
    }
}

fn load_app_data_from_disk(root_dir: &Path) -> Result<AppData, SchedulerError> {
    let data_path = root_dir.join("state.json");
    let mut data: AppData = fs::read_to_string(&data_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();
    normalize_loaded_app_data(&mut data);
    Ok(data)
}

fn normalize_loaded_app_data(data: &mut AppData) {
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    normalize_image_model_settings(&mut data.settings);
    backfill_execution_records_from_attempts(data);
    compact_retry_execution_records_for_display(data);
    recover_tasks_on_load(data);
    backfill_draft_command_previews(data);
    apply_log_retention(data);
    cap_execution_history(data);
    rebuild_asset_hash_index(data);
}

pub fn default_store_dir() -> PathBuf {
    std::env::var("DREAMINA_SCHEDULER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".dreamina-scheduler")
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiModelConfig {
    pub id: String,
    pub name: String,
    #[serde(default = "default_ai_api_mode")]
    pub api_mode: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
    #[serde(default = "default_openai_model")]
    pub model: String,
}

fn default_ai_api_mode() -> String {
    "responses".to_string()
}
fn default_openai_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}
fn default_openai_model() -> String {
    "gpt-5.4".to_string()
}
fn default_ai_model_configs() -> Vec<AiModelConfig> {
    vec![AiModelConfig {
        id: "default-openai".to_string(),
        name: "OpenAI 默认".to_string(),
        api_mode: default_ai_api_mode(),
        api_key: String::new(),
        base_url: default_openai_base_url(),
        model: default_openai_model(),
    }]
}
fn default_active_ai_model_id() -> String {
    "default-openai".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiTitleRequest {
    pub url: String,
    pub body: serde_json::Value,
}

const AI_TITLE_SYSTEM_PROMPT: &str =
    "你是短视频任务标题助手。请基于用户的视频生成提示词，生成一个 6-14 个中文字符的任务标题。只返回 JSON：{\"title\":\"...\"}。";

pub fn build_ai_title_request(
    config: &AiModelConfig,
    prompt: &str,
) -> Result<AiTitleRequest, SchedulerError> {
    if config.api_key.trim().is_empty() {
        return Err(SchedulerError::Io("AI 模型 API Key 为空".to_string()));
    }
    if config.base_url.trim().is_empty() {
        return Err(SchedulerError::Io("AI 模型 Base URL 为空".to_string()));
    }
    if config.model.trim().is_empty() {
        return Err(SchedulerError::Io("AI 模型名称为空".to_string()));
    }
    let base_url = config.base_url.trim().trim_end_matches('/');
    let mode = config.api_mode.trim().to_ascii_lowercase();
    let user_prompt = prompt.trim();
    if mode == "chat" {
        return Ok(AiTitleRequest {
            url: format!("{base_url}/chat/completions"),
            body: serde_json::json!({
                "model": config.model.trim(),
                "messages": [
                    { "role": "system", "content": AI_TITLE_SYSTEM_PROMPT },
                    { "role": "user", "content": user_prompt }
                ],
                "response_format": { "type": "json_object" },
                "temperature": 0.4
            }),
        });
    }
    Ok(AiTitleRequest {
        url: format!("{base_url}/responses"),
        body: serde_json::json!({
            "model": config.model.trim(),
            "input": [
                { "role": "system", "content": AI_TITLE_SYSTEM_PROMPT },
                { "role": "user", "content": user_prompt }
            ],
            "text": { "format": { "type": "json_object" } },
            "temperature": 0.4
        }),
    })
}

pub fn sanitize_generated_task_title(value: &str) -> String {
    let mut text = value
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string();
    if let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) {
        text = line.to_string();
    }
    for prefix in ["标题：", "标题:", "title：", "title:"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_string();
        }
    }
    text = text
        .trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '“' | '”' | '‘' | '’' | '《' | '》' | '<' | '>' | '「' | '」'
            )
        })
        .trim()
        .to_string();
    text.chars().take(18).collect()
}

pub fn extract_generated_task_title(value: &serde_json::Value) -> Option<String> {
    // 1. 直接包含 title 字段（响应 body 本身就是 {"title":"..."}）
    if let Some(title) = find_json_string_field(value, "title") {
        let clean = sanitize_generated_task_title(&title);
        if !clean.is_empty() {
            return Some(clean);
        }
    }

    // 2. Chat Completions API：choices[0].message.content
    if let Some(content) = value
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|msg| msg.get("content"))
        .and_then(|c| c.as_str())
    {
        if let Some(title) = extract_title_from_content_str(content) {
            return Some(title);
        }
    }

    // 3. Responses API：output[0].content[0].text
    if let Some(text) = value
        .get("output")
        .and_then(|v| v.as_array())
        .and_then(|output| output.first())
        .and_then(|item| item.get("content"))
        .and_then(|v| v.as_array())
        .and_then(|content| content.first())
        .and_then(|item| item.get("text"))
        .and_then(|t| t.as_str())
    {
        if let Some(title) = extract_title_from_content_str(text) {
            return Some(title);
        }
    }

    // 4. Responses API 备用路径：output[0].text
    if let Some(text) = value
        .get("output")
        .and_then(|v| v.as_array())
        .and_then(|output| output.first())
        .and_then(|item| item.get("text"))
        .and_then(|t| t.as_str())
    {
        if let Some(title) = extract_title_from_content_str(text) {
            return Some(title);
        }
    }

    None
}

fn extract_title_from_content_str(content: &str) -> Option<String> {
    // 尝试解析为 JSON，寻找 title 字段
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(title) = find_json_string_field(&parsed, "title") {
            let clean = sanitize_generated_task_title(&title);
            if !clean.is_empty() {
                return Some(clean);
            }
        }
    }
    // 尝试 "title: xxx" 纯文本格式
    if let Some(title) = first_field(content, "title") {
        let clean = sanitize_generated_task_title(&title);
        if !clean.is_empty() {
            return Some(clean);
        }
    }
    // 内容本身足够短时直接用作标题
    let clean = sanitize_generated_task_title(content);
    if !clean.is_empty() && clean.chars().count() <= 18 {
        return Some(clean);
    }
    None
}

pub fn format_ai_model_test_log(
    api_mode: &str,
    model: &str,
    url: &str,
    raw_response: &str,
    parsed: Option<&str>,
    error: Option<&str>,
) -> String {
    let parsed_text = parsed.unwrap_or("-");
    let error_text = error.unwrap_or("-");
    format!(
        "AI 模型测试：mode={}；model={}；url={}；parsed={}；error={}；raw={}",
        api_mode,
        model,
        url,
        parsed_text,
        error_text,
        truncate_log(raw_response)
    )
}

pub fn format_image_model_settings_log(settings: &SchedulerSettings) -> String {
    let active = active_image_model_config(settings);
    let active_label = active
        .map(|config| format!("{} / {}", config.name, config.model))
        .unwrap_or_else(|| "-".to_string());
    let base_url = active.map(|config| config.base_url.as_str()).unwrap_or("-");
    let key_state = active
        .map(|config| {
            if config.api_key.trim().is_empty() {
                "未填写"
            } else {
                "已填写"
            }
        })
        .unwrap_or("未填写");
    format!(
        "图片模型数量={}；active_image_model_id={}；当前图片模型={}；base_url={}；api_key={}",
        settings.image_model_configs.len(),
        settings.active_image_model_id,
        active_label,
        base_url,
        key_state,
    )
}

fn truncate_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

pub fn parse_imagegen_json_response(
    response_text: &str,
    url: &str,
) -> Result<serde_json::Value, String> {
    let trimmed = response_text.trim_start();
    if trimmed.starts_with("<!doctype html")
        || trimmed.starts_with("<!DOCTYPE html")
        || trimmed.starts_with("<html")
    {
        return Err(format!(
            "图片生成接口返回 HTML 页面，请检查图片模型 Base URL 是否为 API 地址、路径前缀是否正确，以及该供应商是否支持图片生成接口。url={}；原始内容：{}",
            url,
            truncate_text(response_text, 300),
        ));
    }
    serde_json::from_str::<serde_json::Value>(response_text).map_err(|e| {
        format!(
            "解析响应失败：{e}；url={}；原始内容：{}",
            url,
            truncate_text(response_text, 300)
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConcurrencyLimitPolicy {
    SilentRetry,
    SilentFail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerSettings {
    pub concurrency_limit_policy: ConcurrencyLimitPolicy,
    pub concurrency_retry_delay_seconds: u64,
    pub concurrency_retry_max_attempts: u32,
    #[serde(default = "default_true")]
    pub auto_query_enabled: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_seconds: u64,
    #[serde(default = "default_log_retention")]
    pub log_retention_count: u32,
    #[serde(default = "default_mac_install_command")]
    pub mac_install_command: String,
    #[serde(default)]
    pub windows_install_command: String,
    #[serde(default = "default_ai_model_configs")]
    pub ai_model_configs: Vec<AiModelConfig>,
    #[serde(default = "default_active_ai_model_id")]
    pub active_ai_model_id: String,
    #[serde(default = "default_true")]
    pub prevent_sleep: bool,
    #[serde(default = "default_image_model_configs")]
    pub image_model_configs: Vec<ImageModelConfig>,
    #[serde(default = "default_active_image_model_id")]
    pub active_image_model_id: String,
    #[serde(default)]
    pub image_model_config: Option<ImageModelConfig>,
}

fn default_true() -> bool {
    true
}
fn default_poll_interval() -> u64 {
    60
}
fn default_log_retention() -> u32 {
    500
}
fn default_mac_install_command() -> String {
    "curl -fsSL https://jimeng.jianying.com/cli | bash".to_string()
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            concurrency_limit_policy: ConcurrencyLimitPolicy::SilentRetry,
            concurrency_retry_delay_seconds: 300,
            concurrency_retry_max_attempts: 8,
            auto_query_enabled: true,
            poll_interval_seconds: 60,
            log_retention_count: 500,
            mac_install_command: default_mac_install_command(),
            windows_install_command: String::new(),
            ai_model_configs: default_ai_model_configs(),
            active_ai_model_id: default_active_ai_model_id(),
            prevent_sleep: true,
            image_model_configs: default_image_model_configs(),
            active_image_model_id: default_active_image_model_id(),
            image_model_config: Some(ImageModelConfig::default()),
        }
    }
}

/// 检查当前是否存在需要防休眠的任务（排队/预定/等待重试/提交中/查询中/非停止的提交后查询）。
pub fn needs_keep_awake(tasks: &[ScheduledTask]) -> bool {
    tasks.iter().any(|t| {
        matches!(
            t.status.as_str(),
            "queued" | "scheduled" | "retry_wait" | "submitting" | "submitted" | "querying"
        ) && !t.auto_query_stopped
    })
}

/// 完全空闲（无活跃任务）时的调度等待上限。
const IDLE_WAIT_SECS: u64 = 60;
/// 有活跃任务时维持的灵敏间隔（沿用原调度间隔常量）。
const ACTIVE_WAIT_SECS: u64 = SCHEDULER_TICK_INTERVAL_SECS;

/// 是否存在需要调度线程持续照看的活跃任务。与 `needs_keep_awake` 同源真值。
pub fn has_active_tasks(tasks: &[ScheduledTask]) -> bool {
    needs_keep_awake(tasks)
}

/// 二元自适应等待时长：有活跃任务保持 30s，完全空闲拉长到 60s。
/// 拉长后的灵敏度由入队 `SchedulerWaker::notify` 唤醒兜底。
pub fn compute_wait_duration(tasks: &[ScheduledTask]) -> StdDuration {
    if has_active_tasks(tasks) {
        StdDuration::from_secs(ACTIVE_WAIT_SECS)
    } else {
        StdDuration::from_secs(IDLE_WAIT_SECS)
    }
}

/// 调度线程唤醒原语：把不可打断的 `sleep` 换成可被入队事件提前唤醒的等待。
pub struct SchedulerWaker {
    /// true=有新工作待处理，用于防止唤醒丢失。
    pending: std::sync::Mutex<bool>,
    cvar: std::sync::Condvar,
}

impl SchedulerWaker {
    pub fn new() -> Self {
        Self {
            pending: std::sync::Mutex::new(false),
            cvar: std::sync::Condvar::new(),
        }
    }

    /// 入队/改期路径调用：标记 pending 并唤醒调度线程。
    pub fn notify(&self) {
        let mut pending = self.pending.lock().expect("waker lock");
        *pending = true;
        self.cvar.notify_one();
    }

    /// 调度线程调用：最多等 `wait`，被 `notify` 可提前返回；返回前消费 pending。
    pub fn wait(&self, wait: StdDuration) {
        let mut pending = self.pending.lock().expect("waker lock");
        if !*pending {
            let (guard, _timeout) = self
                .cvar
                .wait_timeout(pending, wait)
                .expect("waker wait_timeout");
            pending = guard;
        }
        *pending = false;
    }
}

impl Default for SchedulerWaker {
    fn default() -> Self {
        Self::new()
    }
}

/// 根据连续无结果查询次数返回退避间隔秒数
pub fn backoff_interval_secs(consecutive_count: u32) -> u64 {
    let idx = consecutive_count as usize;
    if idx >= BACKOFF_INTERVALS_SECS.len() {
        *BACKOFF_INTERVALS_SECS.last().unwrap_or(&600)
    } else {
        BACKOFF_INTERVALS_SECS[idx]
    }
}

/// 判断任务是否到了退避允许的查询时间
pub fn is_backoff_due(task: &ScheduledTask, now: DateTime<Utc>) -> bool {
    if task.consecutive_no_result_queries == 0 {
        return true;
    }
    let Some(ref last_at) = task.last_auto_query_at else {
        return true;
    };
    let interval =
        Duration::seconds(backoff_interval_secs(task.consecutive_no_result_queries) as i64);
    DateTime::parse_from_rfc3339(last_at)
        .map(|t| now.signed_duration_since(t.with_timezone(&Utc)) >= interval)
        .unwrap_or(true)
}

/// 判断任务是否超过最长等待时间（4小时），需要停止自动查询
pub fn is_past_max_wait(task: &ScheduledTask, now: DateTime<Utc>) -> bool {
    let Some(ref submitted_at) = task.submitted_at else {
        return false;
    };
    // 如果 submitted_at 是空字符串，跳过检查
    let trimmed = submitted_at.trim();
    if trimmed.is_empty() {
        return false;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|t| {
            now.signed_duration_since(t.with_timezone(&Utc)) >= Duration::hours(MAX_WAIT_HOURS)
        })
        .unwrap_or(false)
}

fn is_past_no_remote_queue_info_wait(task: &ScheduledTask, now: DateTime<Utc>) -> bool {
    let Some(ref submitted_at) = task.submitted_at else {
        return false;
    };
    let trimmed = submitted_at.trim();
    if trimmed.is_empty() {
        return false;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|t| {
            now.signed_duration_since(t.with_timezone(&Utc))
                >= Duration::minutes(MAX_NO_REMOTE_QUEUE_INFO_MINUTES)
        })
        .unwrap_or(false)
}

fn query_output_has_remote_progress_info(parsed: &QueryOutput, raw: &str) -> bool {
    if parsed.queue_info.is_some()
        || !parsed.result_paths.is_empty()
        || !parsed.result_urls.is_empty()
        || parsed.fail_reason.is_some()
        || parsed.error_code.is_some()
    {
        return true;
    }
    let Some(value) = serde_json::from_str::<serde_json::Value>(raw).ok() else {
        return false;
    };
    find_json_string_field(&value, "task_status").is_some()
        || find_json_string_field(&value, "task_id").is_some()
        || find_json_string_field(&value, "history_id").is_some()
        || find_json_string_field(&value, "history_record_id").is_some()
}

/// 查询后更新退避状态
pub fn update_query_backoff(task: &mut ScheduledTask) {
    task.consecutive_no_result_queries += 1;
    task.last_auto_query_at = Some(now_rfc3339());
}

/// 重置退避状态（用于手动查询、重新提交）
pub fn reset_query_backoff(task: &mut ScheduledTask) {
    task.last_auto_query_at = None;
    task.consecutive_no_result_queries = 0;
    task.auto_query_stopped = false;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandPlan {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DreaminaErrorKind {
    ConcurrencyLimit,
    ComplianceRequired,
    Transient,
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedDreaminaError {
    pub kind: DreaminaErrorKind,
    pub next_status: String,
    pub retry_after_seconds: Option<u64>,
    pub show_modal: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitOutput {
    pub submit_id: Option<String>,
    pub gen_status: Option<String>,
    pub fail_reason: Option<String>,
    /// API 级别错误码（如 Dreamina 返回的 code 字段）。>= 400 时即使有 submit_id 也视为提交失败。
    pub error_code: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryOutput {
    pub gen_status: Option<String>,
    pub fail_reason: Option<String>,
    pub result_paths: Vec<String>,
    pub result_urls: Vec<String>,
    pub queue_info: Option<QueueInfo>,
    /// API 级别错误码（如 Dreamina 返回的 code 字段）。>= 400 时视为查询失败。
    pub error_code: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueInfo {
    pub queue_idx: Option<u64>,
    pub priority: Option<u64>,
    pub queue_status: Option<String>,
    pub queue_length: Option<u64>,
}

pub fn resolve_task_inputs(
    task: &TaskDraft,
    assets: &[Asset],
    roles: &[Role],
) -> Result<ResolvedTaskInputs, SchedulerError> {
    let asset_by_id = assets
        .iter()
        .map(|asset| (asset.id.as_str(), asset))
        .collect::<HashMap<_, _>>();
    let mut image_asset_ids = Vec::new();
    let mut audio_asset_ids = Vec::new();
    let manual_mention_ids = Vec::new();
    let matched_role_ids = Vec::new();
    let mut unresolved_mentions = Vec::new();
    let mut prompt_rewrites = Vec::new();
    let mentions = extract_mentions(&task.prompt);

    for asset_id in &task.image_asset_ids {
        push_asset_id(
            asset_id,
            AssetKind::Image,
            &asset_by_id,
            &mut image_asset_ids,
            &mut audio_asset_ids,
        )?;
    }
    for asset_id in &task.audio_asset_ids {
        push_asset_id(
            asset_id,
            AssetKind::Audio,
            &asset_by_id,
            &mut image_asset_ids,
            &mut audio_asset_ids,
        )?;
    }

    if !mentions.is_empty() {
        let mention_asset_index = build_mention_asset_index(task, assets, roles);
        for mention in mentions {
            let candidate = mention_asset_index
                .get(mention.as_str())
                .cloned()
                .or_else(|| {
                    resolve_storyboard_fallback_candidate(
                        mention.as_str(),
                        task,
                        &asset_by_id,
                        &image_asset_ids,
                    )
                });
            if let Some(candidate) = candidate {
                match candidate.kind {
                    AssetKind::Image => {
                        push_unique(&mut image_asset_ids, candidate.asset_id.clone());
                        let index = image_asset_ids
                            .iter()
                            .position(|id| id == &candidate.asset_id)
                            .unwrap_or(image_asset_ids.len() - 1)
                            + 1;
                        push_prompt_rewrite(&mut prompt_rewrites, mention, format!("图{index}"));
                    }
                    AssetKind::Audio => {
                        push_unique(&mut audio_asset_ids, candidate.asset_id.clone());
                        let index = audio_asset_ids
                            .iter()
                            .position(|id| id == &candidate.asset_id)
                            .unwrap_or(audio_asset_ids.len() - 1)
                            + 1;
                        push_prompt_rewrite(&mut prompt_rewrites, mention, format!("音频{index}"));
                    }
                }
            } else {
                push_unique(&mut unresolved_mentions, mention);
            }
        }
    }

    validate_input_counts(&image_asset_ids, &audio_asset_ids)?;

    Ok(ResolvedTaskInputs {
        image_paths: ids_to_paths(&image_asset_ids, &asset_by_id)?,
        audio_paths: ids_to_paths(&audio_asset_ids, &asset_by_id)?,
        image_asset_ids,
        audio_asset_ids,
        manual_mention_ids,
        matched_role_ids,
        unresolved_mentions,
        prompt_rewrites,
    })
}

pub fn build_multimodal2video_args(
    task: &TaskDraft,
    inputs: &ResolvedTaskInputs,
) -> Result<Vec<String>, SchedulerError> {
    validate_video_params(&task.params)?;
    validate_input_counts(&inputs.image_asset_ids, &inputs.audio_asset_ids)?;

    let mut args = vec!["multimodal2video".to_string()];
    for path in &inputs.image_paths {
        args.push(format!("--image={path}"));
    }
    for path in &inputs.audio_paths {
        args.push(format!("--audio={path}"));
    }
    let prompt = build_prompt_for_cli(task.prompt.trim(), &inputs.prompt_rewrites);
    if !prompt.is_empty() {
        args.push(format!("--prompt={prompt}"));
    }
    args.push(format!("--model_version={}", task.params.model_version));
    args.push(format!("--ratio={}", task.params.ratio));
    args.push(format!("--duration={}", task.params.duration));
    args.push(format!(
        "--video_resolution={}",
        task.params.video_resolution
    ));
    Ok(args)
}

fn build_submit_preflight_detail(
    task: &ScheduledTask,
    resolved: &ResolvedTaskInputs,
    assets: &[Asset],
) -> String {
    let asset_by_id = assets
        .iter()
        .map(|asset| (asset.id.as_str(), asset))
        .collect::<HashMap<_, _>>();
    let detail = serde_json::json!({
        "task_id": task.id,
        "title": task.title,
        "attempt_count_after_start": task.attempt_count,
        "planned_submit_count": planned_submit_count(task),
        "params": task.params,
        "resolved_counts": {
            "images": resolved.image_paths.len(),
            "audios": resolved.audio_paths.len(),
            "prompt_rewrites": resolved.prompt_rewrites.len(),
            "unresolved_mentions": resolved.unresolved_mentions.len(),
        },
        "images": build_asset_preflight_entries(&resolved.image_asset_ids, &resolved.image_paths, &asset_by_id),
        "audios": build_asset_preflight_entries(&resolved.audio_asset_ids, &resolved.audio_paths, &asset_by_id),
        "unresolved_mentions": resolved.unresolved_mentions,
    });
    serde_json::to_string_pretty(&detail).unwrap_or_else(|_| detail.to_string())
}

fn build_asset_preflight_entries(
    asset_ids: &[String],
    paths: &[String],
    asset_by_id: &HashMap<&str, &Asset>,
) -> Vec<serde_json::Value> {
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let asset_id = asset_ids.get(index).cloned().unwrap_or_default();
            let asset = asset_by_id.get(asset_id.as_str()).copied();
            let metadata = fs::metadata(path);
            let (exists, actual_size_bytes, modified_unix_secs, metadata_error) = match metadata {
                Ok(meta) => {
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map(|duration| duration.as_secs());
                    (true, Some(meta.len()), modified, None)
                }
                Err(error) => (false, None, None, Some(error.to_string())),
            };
            serde_json::json!({
                "index": index + 1,
                "asset_id": asset_id,
                "asset_name": asset.map(|a| a.name.clone()).unwrap_or_default(),
                "asset_kind": asset.map(|a| format!("{:?}", a.kind)).unwrap_or_default(),
                "mime": asset.map(|a| a.mime.clone()).unwrap_or_default(),
                "declared_size_bytes": asset.map(|a| a.size_bytes),
                "duration_seconds": asset.and_then(|a| a.duration_seconds),
                "content_hash": asset.and_then(|a| a.content_hash.clone()),
                "source_path": asset.map(|a| a.source_path.clone()).unwrap_or_default(),
                "stored_path": asset.map(|a| a.stored_path.clone()).unwrap_or_default(),
                "resolved_path": path,
                "resolved_matches_stored_path": asset.map(|a| a.stored_path == *path).unwrap_or(false),
                "extension": Path::new(path).extension().and_then(|value| value.to_str()).unwrap_or(""),
                "exists": exists,
                "actual_size_bytes": actual_size_bytes,
                "modified_unix_secs": modified_unix_secs,
                "metadata_error": metadata_error,
            })
        })
        .collect()
}

fn build_prompt_for_cli(prompt: &str, rewrites: &[PromptMentionRewrite]) -> String {
    let prompt = prompt.trim();
    if rewrites.is_empty() {
        return prompt.to_string();
    }
    rewrite_prompt_mentions(prompt, rewrites)
}

fn consecutive_transient_submit_retries(task: &ScheduledTask) -> u32 {
    task.execution_records
        .iter()
        .rev()
        .take_while(|record| record.status == "retry_wait" && record.error_kind == "Transient")
        .count() as u32
}

fn latest_execution_record(task: &ScheduledTask) -> Option<&TaskExecutionRecord> {
    task.execution_records
        .iter()
        .rev()
        .find(|record| !record.status.trim().is_empty())
}

fn latest_record_is_transient_failure(task: &ScheduledTask) -> bool {
    latest_execution_record(task)
        .map(|record| record.status == "failed" && record.error_kind == "Transient")
        .unwrap_or(false)
}

fn apply_submit_error_retry_state(
    task: &mut ScheduledTask,
    classified: &ClassifiedDreaminaError,
    _concurrency_retry_max_attempts: u32,
) -> String {
    let mut next_status = classified.next_status.clone();
    if classified.kind == DreaminaErrorKind::ConcurrencyLimit {
        task.concurrency_retry_count += 1;
    } else if classified.kind == DreaminaErrorKind::Transient
        && next_status == "retry_wait"
        && (consecutive_transient_submit_retries(task) >= MAX_TRANSIENT_SUBMIT_RETRIES
            || latest_record_is_transient_failure(task))
    {
        next_status = "failed".to_string();
    }
    if next_status == "retry_wait" {
        if let Some(seconds) = classified.retry_after_seconds {
            task.next_run_at = Some((Utc::now() + Duration::seconds(seconds as i64)).to_rfc3339());
        }
    } else if next_status == "failed" {
        task.next_run_at = None;
    }
    next_status
}

fn compact_submit_error_detail(kind: &str, message: &str) -> String {
    match kind {
        "ConcurrencyLimit" => "并发任务仍在生成中，已自动排队等待下次重试。".to_string(),
        "Transient" => "提交时遇到临时网络或平台错误，已自动排队等待下次重试。".to_string(),
        _ => message.trim().to_string(),
    }
}

fn compact_failed_submit_error_detail(kind: &str, message: &str) -> String {
    match kind {
        "ConcurrencyLimit" => "并发任务仍在生成中，已自动排队等待下次重试。".to_string(),
        "Transient" => "提交时遇到临时网络或平台错误，自动重试已达上限，已标记失败。".to_string(),
        _ => message.trim().to_string(),
    }
}

fn should_merge_retry_execution_record(status: &str, error_kind: &str) -> bool {
    matches!(status, "retry_wait" | "failed")
        && matches!(error_kind, "ConcurrencyLimit" | "Transient")
}

fn upsert_submit_execution_record(task: &mut ScheduledTask, record: TaskExecutionRecord) {
    if should_merge_retry_execution_record(&record.status, &record.error_kind) {
        if let Some(existing) = task
            .execution_records
            .iter_mut()
            .rev()
            .find(|item| {
                matches!(item.status.as_str(), "retry_wait" | "failed")
                    && item.error_kind == record.error_kind
            })
        {
            existing.finished_at = record.finished_at;
            existing.status = record.status;
            existing.submit_id = record.submit_id;
            existing.command_preview = record.command_preview;
            existing.error_detail = record.error_detail;
            return;
        }
    }
    task.execution_records.push(record);
}

fn submit_execution_record_error_detail(status: &str, error_kind: &str, message: &str) -> String {
    if message.trim().is_empty() {
        String::new()
    } else if status == "failed" {
        compact_failed_submit_error_detail(error_kind, message)
    } else {
        compact_submit_error_detail(error_kind, message)
    }
}

pub fn compact_retry_execution_records_for_display(data: &mut AppData) -> usize {
    let mut removed = 0;

    for task in &mut data.tasks {
        let mut compacted = Vec::with_capacity(task.execution_records.len());

        for mut record in task.execution_records.drain(..) {
            if !should_merge_retry_execution_record(&record.status, &record.error_kind) {
                compacted.push(record);
                continue;
            }

            record.error_detail = submit_execution_record_error_detail(
                &record.status,
                &record.error_kind,
                &record.error_detail,
            );
            let existing_index = compacted.iter().rposition(|existing: &TaskExecutionRecord| {
                should_merge_retry_execution_record(&existing.status, &existing.error_kind)
                    && existing.error_kind == record.error_kind
            });

            let Some(index) = existing_index else {
                compacted.push(record);
                continue;
            };

            if record.started_at >= compacted[index].started_at {
                compacted[index] = record;
            } else {
                compacted[index].error_detail = submit_execution_record_error_detail(
                    &compacted[index].status,
                    &compacted[index].error_kind,
                    &compacted[index].error_detail,
                );
            }
            removed += 1;
        }

        task.execution_records = compacted;
    }

    removed
}

pub fn classify_dreamina_error(
    message: &str,
    settings: &SchedulerSettings,
) -> ClassifiedDreaminaError {
    let text = message.trim().to_string();
    let lower_text = text.to_ascii_lowercase();
    if is_concurrency_limit(&text) {
        return ClassifiedDreaminaError {
            kind: DreaminaErrorKind::ConcurrencyLimit,
            next_status: "retry_wait".to_string(),
            retry_after_seconds: Some(settings.concurrency_retry_delay_seconds),
            show_modal: false,
            message: text,
        };
    }
    if text.contains("AigcComplianceConfirmationRequired") {
        return ClassifiedDreaminaError {
            kind: DreaminaErrorKind::ComplianceRequired,
            next_status: "blocked".to_string(),
            retry_after_seconds: None,
            show_modal: false,
            message: text,
        };
    }
    if text.contains("Unexpected end of JSON input")
        || text.contains("Unexpected token")
        || text.contains("context deadline exceeded")
        || text.contains("timeout")
        || lower_text.contains("eof")
        || lower_text.contains("applyimageupload")
        || lower_text.contains("applyuploadinner")
        || lower_text.contains("imagex.bytedanceapi.com")
        || lower_text.contains("vod.bytedanceapi.com")
        || lower_text.contains("upload video/audio")
        || lower_text.contains("connection reset")
        || lower_text.contains("broken pipe")
    {
        return ClassifiedDreaminaError {
            kind: DreaminaErrorKind::Transient,
            next_status: "retry_wait".to_string(),
            retry_after_seconds: Some(settings.concurrency_retry_delay_seconds),
            show_modal: false,
            message: text,
        };
    }
    ClassifiedDreaminaError {
        kind: DreaminaErrorKind::Generic,
        next_status: "failed".to_string(),
        retry_after_seconds: None,
        show_modal: false,
        message: text,
    }
}

pub fn parse_submit_output(output: &str) -> SubmitOutput {
    let parsed = serde_json::from_str::<serde_json::Value>(output).ok();
    let mut text = output.to_string();
    if let Some(value) = &parsed {
        collect_json_strings(value, &mut text);
    }
    let error_code = parsed.as_ref().and_then(|v| {
        v.get("code")
            .and_then(|c| c.as_i64())
            .filter(|&c| c != 0 && c != 200)
    });
    SubmitOutput {
        submit_id: if error_code.map_or(false, |c| c >= 400) {
            // API 报错时忽略响应中可能残留的 submit_id，避免把错误响应误判为成功提交
            None
        } else {
            parsed
                .as_ref()
                .and_then(|value| find_json_string_field(value, "submit_id"))
                .or_else(|| first_field(&text, "submit_id"))
        },
        gen_status: parsed
            .as_ref()
            .and_then(|value| find_json_string_field(value, "gen_status"))
            .or_else(|| first_field(&text, "gen_status")),
        fail_reason: parsed
            .as_ref()
            .and_then(|value| find_json_string_field(value, "fail_reason"))
            .or_else(|| {
                parsed
                    .as_ref()
                    .and_then(|value| find_json_string_field(value, "failReason"))
            })
            .or_else(|| {
                parsed
                    .as_ref()
                    .and_then(|value| find_json_string_field(value, "message"))
            })
            .or_else(|| first_field(&text, "fail_reason"))
            .or_else(|| first_field(&text, "failReason"))
            .or_else(|| first_field(&text, "message")),
        error_code,
    }
}

pub fn parse_query_output(output: &str) -> QueryOutput {
    let parsed = serde_json::from_str::<serde_json::Value>(output).ok();
    let mut text = output.to_string();
    if let Some(value) = &parsed {
        collect_json_strings(value, &mut text);
    }
    let queue_info = parsed
        .as_ref()
        .and_then(|v| v.get("queue_info"))
        .and_then(|qi| {
            Some(QueueInfo {
                queue_idx: qi.get("queue_idx").and_then(|v| v.as_u64()),
                priority: qi.get("priority").and_then(|v| v.as_u64()),
                queue_status: qi
                    .get("queue_status")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                queue_length: qi.get("queue_length").and_then(|v| v.as_u64()),
            })
        });
    QueryOutput {
        gen_status: parsed
            .as_ref()
            .and_then(|value| find_json_string_field(value, "gen_status"))
            .or_else(|| first_field(&text, "gen_status"))
            .or_else(|| first_field(&text, "status")),
        fail_reason: parsed
            .as_ref()
            .and_then(|value| find_json_string_field(value, "fail_reason"))
            .or_else(|| {
                parsed
                    .as_ref()
                    .and_then(|value| find_json_string_field(value, "failReason"))
            })
            .or_else(|| {
                parsed
                    .as_ref()
                    .and_then(|value| find_json_string_field(value, "message"))
            })
            .or_else(|| first_field(&text, "fail_reason"))
            .or_else(|| first_field(&text, "failReason"))
            .or_else(|| first_field(&text, "message")),
        result_paths: collect_result_paths(&text),
        result_urls: collect_result_urls(&text),
        queue_info,
        error_code: parsed.as_ref().and_then(|v| {
            v.get("code")
                .and_then(|c| c.as_i64())
                .filter(|&c| c != 0 && c != 200)
        }),
    }
}

pub fn build_install_plan(
    settings: &SchedulerSettings,
    os: &str,
) -> Result<CommandPlan, SchedulerError> {
    match os {
        "macos" => {
            let command = settings.mac_install_command.trim();
            if command.is_empty() {
                return Err(SchedulerError::Io("macOS 安装命令未配置".to_string()));
            }
            Ok(CommandPlan {
                program: "sh".to_string(),
                args: vec!["-lc".to_string(), command.to_string()],
            })
        }
        "windows" => {
            let command = settings.windows_install_command.trim();
            if command.is_empty() {
                return Err(SchedulerError::Io(
                    "Windows 安装命令未配置，请在设置中填入官方 PowerShell 安装命令".to_string(),
                ));
            }
            Ok(CommandPlan {
                program: "powershell".to_string(),
                args: vec![
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            })
        }
        other => Err(SchedulerError::Io(format!("暂不支持当前系统安装：{other}"))),
    }
}

pub fn build_login_plan(cli_path: &str, headless: bool) -> CommandPlan {
    let mut args = vec!["login".to_string()];
    if headless {
        args.push("--headless".to_string());
    }
    CommandPlan {
        program: cli_path.to_string(),
        args,
    }
}

pub fn check_dreamina_cli_status() -> CliStatus {
    let candidates = dreamina_candidates();
    for candidate in candidates {
        let output = Command::new(&candidate).arg("-h").output();
        if let Ok(output) = output {
            if output.status.success() {
                return CliStatus {
                    available: true,
                    path: candidate,
                    message: String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .next()
                        .unwrap_or("dreamina 可用")
                        .to_string(),
                };
            }
        }
    }
    CliStatus {
        available: false,
        path: String::new(),
        message: "未找到 dreamina CLI".to_string(),
    }
}

pub fn host_platform() -> HostPlatform {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let os_label = match os.as_str() {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        value => value,
    }
    .to_string();
    let arch_label = match arch.as_str() {
        "aarch64" => "ARM64",
        "x86_64" => "Intel",
        "x86" => "x86",
        value => value,
    }
    .to_string();
    let label = format!("{os_label} {arch_label}");
    HostPlatform { os, arch, label }
}

pub fn get_dreamina_credit_text() -> Result<String, SchedulerError> {
    let status = check_dreamina_cli_status();
    if !status.available {
        return Err(SchedulerError::Io(status.message));
    }
    let output = Command::new(status.path)
        .arg("user_credit")
        .output()
        .map_err(|error| SchedulerError::Io(error.to_string()))?;
    let text = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        String::from_utf8_lossy(&output.stderr).to_string()
    };
    Ok(text)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditInfo {
    pub available: bool,
    pub total: String,
    pub used: String,
    pub remaining: String,
    pub raw_text: String,
}

pub fn parse_credit_info(raw: &str) -> CreditInfo {
    let mut total = String::new();
    let mut used = String::new();
    let mut remaining = String::new();

    // 尝试 JSON 解析
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(v) = obj.get("total").or(obj.get("credit_total")) {
            total = v.to_string().trim_matches('"').to_string();
        }
        if let Some(v) = obj.get("used").or(obj.get("credit_used")) {
            used = v.to_string().trim_matches('"').to_string();
        }
        if let Some(v) = obj
            .get("remaining")
            .or(obj.get("credit_remaining"))
            .or(obj.get("balance"))
        {
            remaining = v.to_string().trim_matches('"').to_string();
        }
    }

    // 如果 JSON 解析没拿到字段，尝试从文本中提取
    if total.is_empty() {
        for line in raw.lines() {
            let l = line.trim().to_lowercase();
            if l.contains("total") || l.contains("总额") || l.contains("总额度") {
                let num: String = line
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                    .collect();
                if !num.is_empty() {
                    total = num;
                }
            }
            if l.contains("used") || l.contains("已用") || l.contains("已使用") {
                let num: String = line
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                    .collect();
                if !num.is_empty() {
                    used = num;
                }
            }
            if l.contains("remaining") || l.contains("剩余") || l.contains("balance") {
                let num: String = line
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                    .collect();
                if !num.is_empty() {
                    remaining = num;
                }
            }
        }
    }

    let available = !remaining.is_empty() || !total.is_empty();
    CreditInfo {
        available,
        total,
        used,
        remaining,
        raw_text: raw.to_string(),
    }
}

pub fn create_role(input: CreateRoleInput) -> Role {
    let now = now_rfc3339();
    Role {
        id: input
            .id
            .unwrap_or_else(|| format!("role_{}", Uuid::new_v4().simple())),
        name: input.name,
        aliases: input.aliases,
        tags: input.tags,
        description: input.description,
        asset_ids: input.asset_ids,
        created_at: now.clone(),
        updated_at: now,
    }
}

pub fn upsert_role(data: &mut AppData, input: CreateRoleInput) -> Role {
    let role = create_role(input);
    if let Some(index) = data.roles.iter().position(|item| item.id == role.id) {
        let created_at = data.roles[index].created_at.clone();
        data.roles[index] = Role {
            created_at,
            updated_at: now_rfc3339(),
            ..role.clone()
        };
        data.roles[index].clone()
    } else {
        data.roles.push(role.clone());
        role
    }
}

pub fn delete_role(data: &mut AppData, role_id: &str) -> Result<(), SchedulerError> {
    if data.tasks.iter().any(|task| {
        task.role_ids.iter().any(|id| id == role_id)
            || task.manual_mention_ids.iter().any(|id| id == role_id)
    }) {
        return Err(SchedulerError::Io(
            "角色已被任务引用，暂不能删除".to_string(),
        ));
    }
    let before = data.roles.len();
    data.roles.retain(|role| role.id != role_id);
    if data.roles.len() == before {
        return Err(SchedulerError::MissingRole(role_id.to_string()));
    }
    Ok(())
}

pub fn import_media_to_role(
    data: &mut AppData,
    role_media_dir: &Path,
    input: ImportRoleMediaInput,
) -> Result<Role, SchedulerError> {
    let role_index = data
        .roles
        .iter()
        .position(|role| role.id == input.role_id)
        .ok_or_else(|| SchedulerError::MissingRole(input.role_id.clone()))?;
    if input.paths.is_empty() {
        return Err(SchedulerError::Io("请选择要导入的图片或音频".to_string()));
    }

    let mut normalized_paths = Vec::new();
    for path in input.paths {
        let normalized = normalize_source_path(&path);
        if !normalized.is_empty() && !normalized_paths.contains(&normalized) {
            normalized_paths.push(normalized);
        }
    }

    for path in normalized_paths {
        if let Some(asset_id) = find_existing_role_asset_id(data, role_index, &path) {
            push_unique(&mut data.roles[role_index].asset_ids, asset_id);
            continue;
        }
        let source = PathBuf::from(&path);
        let asset = Asset::from_path(&source, role_media_dir, None)?;
        let asset_id = asset.id.clone();
        data.assets.push(asset);
        push_unique(&mut data.roles[role_index].asset_ids, asset_id);
    }
    data.roles[role_index].updated_at = now_rfc3339();
    Ok(data.roles[role_index].clone())
}

fn normalize_source_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    fs::canonicalize(trimmed)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| trimmed.to_string())
}

fn find_existing_role_asset_id(
    data: &AppData,
    role_index: usize,
    source_path: &str,
) -> Option<String> {
    let normalized_source = normalize_source_path(source_path);
    data.roles[role_index]
        .asset_ids
        .iter()
        .find_map(|asset_id| {
            data.assets
                .iter()
                .find(|asset| {
                    asset.id == *asset_id
                        && normalize_source_path(&asset.source_path) == normalized_source
                })
                .map(|asset| asset.id.clone())
        })
}

pub fn remove_media_from_role(
    data: &mut AppData,
    input: RemoveRoleMediaInput,
) -> Result<Role, SchedulerError> {
    let role_index = data
        .roles
        .iter()
        .position(|role| role.id == input.role_id)
        .ok_or_else(|| SchedulerError::MissingRole(input.role_id.clone()))?;
    let remove_keys = data
        .assets
        .iter()
        .find(|asset| asset.id == input.asset_id)
        .map(asset_match_keys)
        .unwrap_or_default();
    let ids_to_remove: Vec<String> = data.roles[role_index]
        .asset_ids
        .iter()
        .filter_map(|asset_id| {
            if asset_id == &input.asset_id {
                return Some(asset_id.clone());
            }
            data.assets
                .iter()
                .find(|asset| asset.id == *asset_id)
                .filter(|asset| {
                    let keys = asset_match_keys(asset);
                    !remove_keys.is_empty() && keys.iter().any(|key| remove_keys.contains(key))
                })
                .map(|asset| asset.id.clone())
        })
        .collect();
    let before = data.roles[role_index].asset_ids.len();
    data.roles[role_index]
        .asset_ids
        .retain(|asset_id| !ids_to_remove.iter().any(|id| id == asset_id));
    if before == data.roles[role_index].asset_ids.len() {
        return Err(SchedulerError::MissingAsset(input.asset_id));
    }
    data.roles[role_index].updated_at = now_rfc3339();

    for removed_id in ids_to_remove {
        let still_used_by_role = data.roles.iter().any(|role| {
            role.asset_ids
                .iter()
                .any(|asset_id| asset_id == &removed_id)
        });
        let still_used_by_task = data.tasks.iter().any(|task| {
            task.image_asset_ids
                .iter()
                .any(|asset_id| asset_id == &removed_id)
                || task
                    .audio_asset_ids
                    .iter()
                    .any(|asset_id| asset_id == &removed_id)
        });
        if still_used_by_role || still_used_by_task {
            continue;
        }
        if let Some(asset_index) = data.assets.iter().position(|asset| asset.id == removed_id) {
            let asset = data.assets.remove(asset_index);
            let _ = fs::remove_file(asset.stored_path);
        }
    }

    Ok(data.roles[role_index].clone())
}

fn asset_match_keys(asset: &Asset) -> Vec<String> {
    let mut keys = Vec::new();
    let source = normalize_source_path(&asset.source_path);
    if !source.is_empty() {
        keys.push(format!("source:{source}"));
    }
    if keys.is_empty() {
        let stored = normalize_source_path(&asset.stored_path);
        if !stored.is_empty() {
            keys.push(format!("stored:{stored}"));
        }
    }
    keys
}

pub fn create_task_with_preview(
    data: &AppData,
    draft: TaskDraft,
) -> Result<ScheduledTask, SchedulerError> {
    if let Some(ref scheduled_at) = draft.scheduled_at {
        if !scheduled_at.trim().is_empty() {
            if let Ok(time) = DateTime::parse_from_rfc3339(scheduled_at) {
                if time.with_timezone(&Utc) <= Utc::now() {
                    return Err(SchedulerError::ScheduledAtInPast);
                }
            }
        }
    }
    let resolved = resolve_task_inputs(&draft, &data.assets, &data.roles)?;
    let command_preview = build_multimodal2video_args(&draft, &resolved)?;
    let mut task = ScheduledTask::from(draft);
    apply_resolved_inputs_to_task(&mut task, &resolved);
    task.command_preview = command_preview;
    Ok(task)
}

fn mcp_orientation_to_ratio(value: Option<&str>) -> Result<String, SchedulerError> {
    match value
        .unwrap_or("portrait")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "portrait" | "vertical" | "9:16" => Ok("9:16".to_string()),
        "landscape" | "horizontal" | "16:9" => Ok("16:9".to_string()),
        other => Err(SchedulerError::UnsupportedRatio(other.to_string())),
    }
}

fn mcp_model_to_version(value: Option<&str>) -> Result<String, SchedulerError> {
    match value.unwrap_or("fast").trim().to_ascii_lowercase().as_str() {
        "" | "fast" | "seedance2.0fast" => Ok("seedance2.0fast".to_string()),
        "standard" | "seedance2.0" => Ok("seedance2.0".to_string()),
        other => Err(SchedulerError::UnsupportedModel(other.to_string())),
    }
}

fn mcp_video_params_from_parts(
    orientation: Option<&str>,
    model: Option<&str>,
    duration: Option<u8>,
    video_resolution: Option<&str>,
) -> Result<VideoParams, SchedulerError> {
    let params = VideoParams {
        model_version: mcp_model_to_version(model)?,
        ratio: mcp_orientation_to_ratio(orientation)?,
        duration: duration.unwrap_or(15),
        video_resolution: video_resolution.unwrap_or("720p").trim().to_string(),
    };
    validate_video_params(&params)?;
    Ok(params)
}

fn import_mcp_asset_from_path(
    assets_dir: &Path,
    source_path: &str,
) -> Result<Asset, SchedulerError> {
    let source = PathBuf::from(source_path);
    let mut asset = Asset::from_path(&source, assets_dir, None)?;
    asset.tags.push("mcp".to_string());
    Ok(asset)
}

pub fn queue_mcp_video_task(
    data: &mut AppData,
    assets_dir: &Path,
    input: McpVideoTaskInput,
) -> Result<McpQueuedVideoTask, SchedulerError> {
    let mut imported_assets = Vec::new();
    let mut image_asset_ids = Vec::new();
    let mut audio_asset_ids = Vec::new();
    let mcp_assets_dir = assets_dir.join("mcp");

    for path in &input.image_paths {
        let asset = import_mcp_asset_from_path(&mcp_assets_dir, path)?;
        if asset.kind != AssetKind::Image {
            return Err(SchedulerError::UnsupportedAssetType(asset.mime));
        }
        image_asset_ids.push(asset.id.clone());
        imported_assets.push(asset);
    }
    for path in &input.audio_paths {
        let asset = import_mcp_asset_from_path(&mcp_assets_dir, path)?;
        if asset.kind != AssetKind::Audio {
            return Err(SchedulerError::UnsupportedAssetType(asset.mime));
        }
        audio_asset_ids.push(asset.id.clone());
        imported_assets.push(asset);
    }

    let draft = TaskDraft {
        title: input.title,
        prompt: input.prompt,
        image_asset_ids,
        audio_asset_ids,
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: mcp_video_params_from_parts(
            input.orientation.as_deref(),
            input.model.as_deref(),
            input.duration,
            input.video_resolution.as_deref(),
        )?,
        scheduled_at: input.start_at.filter(|value| !value.trim().is_empty()),
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let mut preview_data = data.clone();
    preview_data.assets.extend(imported_assets.clone());
    let mut task = create_task_with_preview(&preview_data, draft)?;
    task.planned_submit_count =
        normalize_planned_submit_count(input.planned_submit_count.unwrap_or(1));
    data.assets.extend(imported_assets.clone());
    data.tasks.push(task.clone());
    append_log(
        data,
        LogEntryDraft {
            level: LogLevel::Success,
            source: LogSource::Scheduler,
            category: "task".to_string(),
            event_type: "mcp_create".to_string(),
            message: format!("MCP 创建任务：{}", task.title),
            detail: String::new(),
            task_id: Some(task.id.clone()),
            task_title: Some(task.title.clone()),
            submit_id: None,
            execution_record_id: None,
            error_detail: None,
            raw_output: None,
            stdout: None,
            stderr: None,
            module: Some("mcp".to_string()),
        },
    );

    Ok(McpQueuedVideoTask {
        task,
        imported_assets,
    })
}

fn merge_mcp_video_defaults(
    mut item: McpVideoTaskInput,
    start_at: &Option<String>,
    defaults: &McpVideoTaskDefaults,
) -> McpVideoTaskInput {
    if item.start_at.is_none() {
        item.start_at = start_at.clone();
    }
    if item.orientation.is_none() {
        item.orientation = defaults.orientation.clone();
    }
    if item.model.is_none() {
        item.model = defaults.model.clone();
    }
    if item.duration.is_none() {
        item.duration = defaults.duration;
    }
    if item.video_resolution.is_none() {
        item.video_resolution = defaults.video_resolution.clone();
    }
    if item.planned_submit_count.is_none() {
        item.planned_submit_count = defaults.planned_submit_count;
    }
    item
}

pub fn queue_mcp_video_tasks(
    data: &mut AppData,
    assets_dir: &Path,
    input: McpQueueVideosInput,
) -> Result<Vec<McpQueuedVideoTask>, SchedulerError> {
    let mut working = data.clone();
    let mut queued = Vec::new();
    for item in input.items {
        let merged = merge_mcp_video_defaults(item, &input.start_at, &input.defaults);
        queued.push(queue_mcp_video_task(&mut working, assets_dir, merged)?);
    }
    *data = working;
    Ok(queued)
}

pub fn create_draft_task(
    data: &AppData,
    draft: TaskDraft,
) -> Result<ScheduledTask, SchedulerError> {
    validate_draft_references(data, &draft)?;
    let resolved = resolve_optional_draft_inputs(data, &draft)?;
    let command_preview = match &resolved {
        Some(resolved) => build_multimodal2video_args(&draft, resolved)?,
        None => vec![],
    };
    let mut task = ScheduledTask::from(draft);
    if let Some(resolved) = &resolved {
        apply_resolved_inputs_to_task(&mut task, resolved);
    }
    task.status = "draft".to_string();
    task.scheduled_at = None;
    task.next_run_at = None;
    task.command_preview = command_preview;
    Ok(task)
}

fn resolve_optional_draft_inputs(
    data: &AppData,
    draft: &TaskDraft,
) -> Result<Option<ResolvedTaskInputs>, SchedulerError> {
    match resolve_task_inputs(draft, &data.assets, &data.roles) {
        Ok(resolved) => Ok(Some(resolved)),
        Err(SchedulerError::MissingImageInput) => Ok(None),
        Err(error) => Err(error),
    }
}

fn apply_resolved_inputs_to_task(task: &mut ScheduledTask, resolved: &ResolvedTaskInputs) {
    task.image_asset_ids = resolved.image_asset_ids.clone();
    task.audio_asset_ids = resolved.audio_asset_ids.clone();
}

pub fn backfill_draft_command_previews(data: &mut AppData) -> usize {
    let assets = data.assets.clone();
    let roles = data.roles.clone();
    let mut updated = 0;

    for task in &mut data.tasks {
        if task.status != "draft" {
            continue;
        }
        let draft = draft_from_task(task);
        if let Ok((args, resolved)) =
            build_optional_draft_preview_from_parts(&draft, &assets, &roles)
        {
            let image_asset_ids = resolved
                .as_ref()
                .map(|resolved| resolved.image_asset_ids.clone());
            let audio_asset_ids = resolved
                .as_ref()
                .map(|resolved| resolved.audio_asset_ids.clone());
            if task.command_preview != args {
                task.command_preview = args;
                updated += 1;
            }
            if let Some(image_asset_ids) = image_asset_ids {
                if task.image_asset_ids != image_asset_ids {
                    task.image_asset_ids = image_asset_ids;
                    updated += 1;
                }
            }
            if let Some(audio_asset_ids) = audio_asset_ids {
                if task.audio_asset_ids != audio_asset_ids {
                    task.audio_asset_ids = audio_asset_ids;
                    updated += 1;
                }
            }
        }
    }

    updated
}

pub fn backfill_execution_records_from_attempts(data: &mut AppData) -> usize {
    let mut updated = 0;

    for task in &mut data.tasks {
        let attempts = task.attempts.clone();
        for attempt in attempts.iter().filter(|attempt| {
            attempt
                .command_preview
                .first()
                .map(|cmd| cmd == "multimodal2video")
                .unwrap_or(false)
        }) {
            let raw = format!("{}\n{}", attempt.stdout, attempt.stderr);
            let parsed = parse_submit_output(&raw);
            let submit_id = parsed.submit_id.unwrap_or_default();
            let already_exists = if submit_id.is_empty() {
                task.execution_records.iter().any(|record| {
                    record.started_at == attempt.started_at
                        && record.command_preview == attempt.command_preview
                })
            } else {
                task.execution_records
                    .iter()
                    .any(|record| record.submit_id == submit_id)
            };
            if already_exists {
                continue;
            }

            let status = attempt.status.clone();
            let error_kind = attempt.error_kind.clone();
            let error_detail = submit_execution_record_error_detail(
                &status,
                &error_kind,
                &attempt.error_detail,
            );
            let before_len = task.execution_records.len();
            upsert_submit_execution_record(task, TaskExecutionRecord {
                id: format!("exec_legacy_{}", attempt.id),
                submit_id,
                status,
                started_at: attempt.started_at.clone(),
                finished_at: attempt.finished_at.clone(),
                input_snapshot: TaskExecutionInputSnapshot {
                    prompt: task.prompt.clone(),
                    image_asset_ids: task.image_asset_ids.clone(),
                    audio_asset_ids: task.audio_asset_ids.clone(),
                    role_ids: task.role_ids.clone(),
                    manual_mention_ids: task.manual_mention_ids.clone(),
                    auto_match_roles: task.auto_match_roles,
                    params: task.params.clone(),
                    temp_image_asset_ids: task.temp_image_asset_ids.clone(),
                },
                command_preview: attempt.command_preview.clone(),
                query_records: vec![],
                result_paths: vec![],
                result_urls: vec![],
                error_kind,
                error_detail,
            });
            if task.execution_records.len() > before_len {
                updated += 1;
            }
        }

        for attempt in attempts.iter().filter(|attempt| {
            attempt
                .command_preview
                .first()
                .map(|cmd| cmd == "query_result")
                .unwrap_or(false)
        }) {
            let submit_id = query_submit_id_from_attempt(attempt);
            if submit_id.is_empty() {
                continue;
            }
            let Some(record) = task
                .execution_records
                .iter_mut()
                .find(|record| record.submit_id == submit_id)
            else {
                continue;
            };
            if !record
                .query_records
                .iter()
                .any(|query| query.id == attempt.id)
            {
                record.query_records.push(attempt.clone());
            }
            let raw = format!("{}\n{}", attempt.stdout, attempt.stderr);
            let parsed = parse_query_output(&raw);
            if attempt.status == "succeeded"
                || !parsed.result_paths.is_empty()
                || !parsed.result_urls.is_empty()
            {
                record.status = "succeeded".to_string();
                if !parsed.result_paths.is_empty() {
                    record.result_paths = parsed.result_paths;
                }
                if !parsed.result_urls.is_empty() {
                    record.result_urls = parsed.result_urls;
                }
                record.finished_at = attempt.finished_at.clone();
            } else if attempt.status == "failed" || attempt.status == "query_timeout" {
                record.status = attempt.status.clone();
                record.error_detail = attempt.error_detail.clone();
                record.finished_at = attempt.finished_at.clone();
            }
        }

        if !task.submit_id.trim().is_empty() {
            if let Some(record) = task
                .execution_records
                .iter_mut()
                .find(|record| record.submit_id == task.submit_id)
            {
                if task.status == "succeeded" {
                    if record.result_paths.is_empty() {
                        record.result_paths = task.result_paths.clone();
                    }
                    if record.result_urls.is_empty() {
                        record.result_urls = task.result_urls.clone();
                    }
                    record.status = "succeeded".to_string();
                }
            }
        }
        archive_task_results_into_matching_execution_record(task);
    }

    updated
}

pub fn recover_tasks_on_load(data: &mut AppData) {
    let settings = data.settings.clone();
    for task in &mut data.tasks {
        // 清理旧数据中残留的"应用重启，查询中断"文案（迁移兼容）
        for rec in &mut task.execution_records {
            if rec.error_detail == "应用重启，查询中断" {
                rec.error_detail.clear();
            }
        }
        // 兼容旧数据：将 query_timeout 转为 submitted + auto_query_stopped=true
        if task.status == "query_timeout" {
            task.status = "submitted".to_string();
            task.auto_query_stopped = true;
            if task.last_error.is_empty() || task.last_error.contains("查询超时") {
                task.last_error = "自动查询已超时，请手动查询".to_string();
            }
        }
        if task.status == "retry_wait" && is_concurrency_limit(&task.last_error) {
            recover_failed_concurrency_execution_record(task);
        }
        if should_recover_failed_concurrency_limit(task, &settings) {
            task.status = "retry_wait".to_string();
            task.next_run_at = Some(
                (Utc::now() + Duration::seconds(settings.concurrency_retry_delay_seconds as i64))
                    .to_rfc3339(),
            );
            task.updated_at = now_rfc3339();
            recover_failed_concurrency_execution_record(task);
        } else if task.status == "submitting" {
            task.status = "queued".to_string();
            task.last_error.clear();
        } else if task.status == "querying" {
            // 重启后，有结果 → succeeded，无结果 → submitted + auto_query_stopped=true
            let current_record_has_results = task
                .execution_records
                .iter()
                .find(|r| r.submit_id == task.submit_id)
                .map(|r| !r.result_paths.is_empty() || !r.result_urls.is_empty())
                .unwrap_or(false);
            let has_results = current_record_has_results
                || (task.execution_records.is_empty()
                    && (!task.result_paths.is_empty() || !task.result_urls.is_empty()));

            if has_results {
                task.status = "succeeded".to_string();
                task.last_error.clear();
                if task.result_paths.is_empty() {
                    if let Some(rec) = task
                        .execution_records
                        .iter()
                        .find(|r| r.submit_id == task.submit_id)
                    {
                        task.result_paths = rec.result_paths.clone();
                        task.result_urls = rec.result_urls.clone();
                    }
                }
                if let Some(rec) = task
                    .execution_records
                    .iter_mut()
                    .find(|r| r.submit_id == task.submit_id)
                {
                    if rec.status != "succeeded" {
                        rec.status = "succeeded".to_string();
                    }
                }
            } else {
                // 无结果：标记为 submitted 并允许自动查询一次（reset backoff，不停止）
                task.status = "submitted".to_string();
                task.last_error.clear();
                reset_query_backoff(task);
                // 不设 auto_query_stopped，让 process_queue_command 的第一次轮询来查一次
            }
        }
    }
}

pub fn delete_execution_record_from_data(
    data: &mut AppData,
    task_id: &str,
    execution_id: &str,
) -> Result<ScheduledTask, SchedulerError> {
    let task_index = data
        .tasks
        .iter()
        .position(|t| t.id == task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;

    let before_count = data.tasks[task_index].execution_records.len();
    data.tasks[task_index]
        .execution_records
        .retain(|r| r.id != execution_id);

    if data.tasks[task_index].execution_records.len() == before_count {
        return Err(SchedulerError::Io(format!(
            "找不到执行记录：{execution_id}"
        )));
    }

    let current_submit_id = data.tasks[task_index].submit_id.clone();
    let remaining_has_current = !current_submit_id.is_empty()
        && data.tasks[task_index]
            .execution_records
            .iter()
            .any(|r| r.submit_id == current_submit_id);

    if data.tasks[task_index].execution_records.is_empty() {
        // 无剩余执行记录：保留 prompt/素材/参数，回到安全可编辑状态
        data.tasks[task_index].submit_id.clear();
        data.tasks[task_index].status = "draft".to_string();
        data.tasks[task_index].result_paths.clear();
        data.tasks[task_index].result_urls.clear();
        data.tasks[task_index].last_error.clear();
        data.tasks[task_index].queue_info = None;
        data.tasks[task_index].finished_at.clear();
    } else if !remaining_has_current {
        // 删除的是当前 submit_id 对应的记录：回退到最新剩余记录
        let latest_rec = data.tasks[task_index]
            .execution_records
            .iter()
            .max_by(|a, b| a.started_at.cmp(&b.started_at))
            .cloned()
            .expect("execution_records is non-empty");
        data.tasks[task_index].submit_id = latest_rec.submit_id.clone();
        data.tasks[task_index].status = latest_rec.status.clone();
        data.tasks[task_index].result_paths = latest_rec.result_paths.clone();
        data.tasks[task_index].result_urls = latest_rec.result_urls.clone();
        data.tasks[task_index].last_error = latest_rec.error_detail.clone();
        data.tasks[task_index].queue_info = None;
        if !latest_rec.finished_at.is_empty() {
            data.tasks[task_index].finished_at = latest_rec.finished_at.clone();
        }
    }
    // remaining_has_current：当前 submit_id 对应记录仍在，顶层字段无需变更

    data.tasks[task_index].updated_at = now_rfc3339();
    Ok(data.tasks[task_index].clone())
}

fn archive_task_results_into_matching_execution_record(task: &mut ScheduledTask) -> bool {
    if task.result_paths.is_empty() && task.result_urls.is_empty() {
        return false;
    }

    let mut target_index = None;
    if !task.result_urls.is_empty() {
        target_index = task.execution_records.iter().position(|record| {
            record
                .result_urls
                .iter()
                .any(|url| task.result_urls.iter().any(|task_url| task_url == url))
        });
    }
    if target_index.is_none() && !task.submit_id.trim().is_empty() {
        target_index = task
            .execution_records
            .iter()
            .position(|record| record.submit_id == task.submit_id);
    }
    if target_index.is_none() {
        let candidates = task
            .execution_records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.status == "succeeded")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            target_index = candidates.first().copied();
        }
    }

    let Some(index) = target_index else {
        return false;
    };
    let record = &mut task.execution_records[index];
    let mut changed = false;
    for path in &task.result_paths {
        if !record.result_paths.contains(path) {
            record.result_paths.push(path.clone());
            changed = true;
        }
    }
    for url in &task.result_urls {
        if !record.result_urls.contains(url) {
            record.result_urls.push(url.clone());
            changed = true;
        }
    }
    if (!task.result_paths.is_empty() || !task.result_urls.is_empty())
        && record.status != "succeeded"
    {
        record.status = "succeeded".to_string();
        changed = true;
    }
    changed
}

fn query_submit_id_from_attempt(attempt: &TaskAttempt) -> String {
    for arg in &attempt.command_preview {
        if let Some(value) = arg.strip_prefix("--submit_id=") {
            return value.trim().to_string();
        }
    }
    let raw = format!("{}\n{}", attempt.stdout, attempt.stderr);
    parse_query_output(&raw)
        .gen_status
        .and_then(|_| first_field(&raw, "submit_id"))
        .unwrap_or_default()
}

fn build_optional_draft_preview_from_parts(
    draft: &TaskDraft,
    assets: &[Asset],
    roles: &[Role],
) -> Result<(Vec<String>, Option<ResolvedTaskInputs>), SchedulerError> {
    match resolve_task_inputs(draft, assets, roles) {
        Ok(resolved) => {
            let args = build_multimodal2video_args(draft, &resolved)?;
            Ok((args, Some(resolved)))
        }
        Err(SchedulerError::MissingImageInput) => Ok((vec![], None)),
        Err(error) => Err(error),
    }
}

fn draft_from_task(task: &ScheduledTask) -> TaskDraft {
    TaskDraft {
        title: task.title.clone(),
        prompt: task.prompt.clone(),
        image_asset_ids: task.image_asset_ids.clone(),
        audio_asset_ids: task.audio_asset_ids.clone(),
        role_ids: task.role_ids.clone(),
        manual_mention_ids: task.manual_mention_ids.clone(),
        auto_match_roles: task.auto_match_roles,
        params: task.params.clone(),
        scheduled_at: task.scheduled_at.clone(),
        temp_image_asset_ids: task.temp_image_asset_ids.clone(),
        temp_image_paths: task.temp_image_paths.clone(),
    }
}

fn validate_draft_references(data: &AppData, draft: &TaskDraft) -> Result<(), SchedulerError> {
    for asset_id in draft
        .image_asset_ids
        .iter()
        .chain(draft.audio_asset_ids.iter())
    {
        if !asset_id.trim().is_empty() && !data.assets.iter().any(|asset| asset.id == *asset_id) {
            return Err(SchedulerError::MissingAsset(asset_id.clone()));
        }
    }
    for role_id in draft.role_ids.iter().chain(draft.manual_mention_ids.iter()) {
        if !role_id.trim().is_empty() && !data.roles.iter().any(|role| role.id == *role_id) {
            return Err(SchedulerError::MissingRole(role_id.clone()));
        }
    }
    Ok(())
}

fn planned_submit_count(task: &ScheduledTask) -> u32 {
    normalize_planned_submit_count(task.planned_submit_count)
}

fn successful_execution_count(task: &ScheduledTask) -> u32 {
    let record_count = task
        .execution_records
        .iter()
        .filter(|record| record.status == "succeeded")
        .count() as u32;
    if record_count > 0 {
        return record_count;
    }
    if task.status == "succeeded"
        && (!task.submit_id.trim().is_empty()
            || !task.result_paths.is_empty()
            || !task.result_urls.is_empty())
    {
        return 1;
    }
    0
}

fn needs_more_successful_submits(task: &ScheduledTask) -> bool {
    successful_execution_count(task) < planned_submit_count(task)
}

fn ensure_task_has_pending_submit(task: &mut ScheduledTask) {
    let successful = successful_execution_count(task);
    if successful >= planned_submit_count(task) {
        task.planned_submit_count = normalize_planned_submit_count(successful.saturating_add(1));
    }
}

fn is_active_remote_task(task: &ScheduledTask) -> bool {
    task.status == "submitting"
        || ((task.status == "querying" || task.status == "submitted")
            && !task.submit_id.trim().is_empty()
            && !task.auto_query_stopped)
}

fn has_concurrency_cooldown(data: &AppData, now: DateTime<Utc>) -> bool {
    data.tasks.iter().any(|task| {
        task.status == "retry_wait"
            && is_concurrency_limit(&task.last_error)
            && !is_due(task.next_run_at.as_deref(), now)
    })
}

fn is_due_for_submit(task: &ScheduledTask, now: DateTime<Utc>) -> bool {
    task.status == "queued"
        || (task.status == "retry_wait" && is_due(task.next_run_at.as_deref(), now))
        || (task.status == "scheduled" && is_due(task.next_run_at.as_deref(), now))
}

fn due_sort_key(task: &ScheduledTask) -> String {
    task.next_run_at
        .clone()
        .or_else(|| task.scheduled_at.clone())
        .unwrap_or_else(|| task.created_at.clone())
}

fn retryable_failed_execution_count(task: &ScheduledTask, error_kind: &str) -> u32 {
    task.execution_records
        .iter()
        .filter(|record| record.status == "failed" && record.error_kind == error_kind)
        .count() as u32
}

fn retryable_idle_failed_limit(error_kind: &str) -> u32 {
    match error_kind {
        "Transient" => MAX_IDLE_TRANSIENT_FAILED_RETRIES,
        "ConcurrencyLimit" => MAX_IDLE_CONCURRENCY_FAILED_RETRIES,
        _ => 0,
    }
}

fn record_finished_or_started_at(record: &TaskExecutionRecord) -> &str {
    if record.finished_at.trim().is_empty() {
        record.started_at.as_str()
    } else {
        record.finished_at.as_str()
    }
}

fn is_recent_concurrency_failure(record: &TaskExecutionRecord, now: DateTime<Utc>) -> bool {
    if record.error_kind != "ConcurrencyLimit" {
        return true;
    }
    DateTime::parse_from_rfc3339(record_finished_or_started_at(record))
        .map(|time| {
            now.signed_duration_since(time.with_timezone(&Utc))
                <= Duration::hours(MAX_CONCURRENCY_FAILURE_RECOVERY_HOURS)
        })
        .unwrap_or(false)
}

fn is_idle_failed_retry_due(
    task: &ScheduledTask,
    now: DateTime<Utc>,
    retry_delay_seconds: u64,
) -> bool {
    if task.status != "failed" || !needs_more_successful_submits(task) {
        return false;
    }
    let Some(latest) = latest_execution_record(task) else {
        return false;
    };
    if latest.status != "failed" {
        return false;
    }
    let retry_limit = retryable_idle_failed_limit(latest.error_kind.as_str());
    if retry_limit == 0 {
        return false;
    }
    if !is_recent_concurrency_failure(latest, now) {
        return false;
    }
    let failed_count = retryable_failed_execution_count(task, latest.error_kind.as_str());
    if failed_count == 0 || failed_count > retry_limit {
        return false;
    }
    DateTime::parse_from_rfc3339(record_finished_or_started_at(latest))
        .map(|time| now >= time.with_timezone(&Utc) + Duration::seconds(retry_delay_seconds as i64))
        .unwrap_or(true)
}

fn should_recover_failed_concurrency_limit(
    task: &ScheduledTask,
    settings: &SchedulerSettings,
) -> bool {
    if task.status != "failed"
        || settings.concurrency_limit_policy != ConcurrencyLimitPolicy::SilentRetry
    {
        return false;
    }
    latest_execution_record(task)
        .map(|record| {
            record.status == "failed"
                && is_recent_concurrency_failure(record, Utc::now())
                && (record.error_kind == "ConcurrencyLimit" || is_concurrency_limit(&task.last_error))
        })
        .unwrap_or(false)
}

fn recover_failed_concurrency_execution_record(task: &mut ScheduledTask) {
    if let Some(record) = task
        .execution_records
        .iter_mut()
        .rev()
        .find(|record| {
            record.status == "failed"
                && (record.error_kind == "ConcurrencyLimit"
                    || is_concurrency_limit(&record.error_detail))
        })
    {
        record.status = "retry_wait".to_string();
        record.error_kind = "ConcurrencyLimit".to_string();
        record.error_detail = compact_submit_error_detail("ConcurrencyLimit", &record.error_detail);
        record.finished_at = now_rfc3339();
    }
}

fn next_due_submit_task_id(data: &AppData, now: DateTime<Utc>) -> Option<String> {
    data.tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| needs_more_successful_submits(task) && is_due_for_submit(task, now))
        .min_by(|(left_index, left), (right_index, right)| {
            successful_execution_count(left)
                .cmp(&successful_execution_count(right))
                .then_with(|| due_sort_key(left).cmp(&due_sort_key(right)))
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left_index.cmp(right_index))
        })
        .map(|(_, task)| task.id.clone())
}

fn next_idle_failed_retry_task_id(data: &AppData, now: DateTime<Utc>) -> Option<String> {
    let retry_delay_seconds = data.settings.concurrency_retry_delay_seconds;
    data.tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| is_idle_failed_retry_due(task, now, retry_delay_seconds))
        .min_by(|(left_index, left), (right_index, right)| {
            successful_execution_count(left)
                .cmp(&successful_execution_count(right))
                .then_with(|| {
                    retryable_failed_execution_count(left, "Transient")
                        .cmp(&retryable_failed_execution_count(right, "Transient"))
                })
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left_index.cmp(right_index))
        })
        .map(|(_, task)| task.id.clone())
}

fn next_submit_task_id(data: &AppData, now: DateTime<Utc>) -> Option<String> {
    next_due_submit_task_id(data, now).or_else(|| next_idle_failed_retry_task_id(data, now))
}

fn apply_planned_submit_completion(task: &mut ScheduledTask) {
    if task.status != "succeeded" {
        return;
    }
    if needs_more_successful_submits(task) {
        task.status = "queued".to_string();
        task.next_run_at = None;
    }
}

pub fn set_task_planned_submit_count(
    data: &mut AppData,
    task_id: &str,
    count: u32,
) -> Result<ScheduledTask, SchedulerError> {
    let task_index = data
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
    let task = &mut data.tasks[task_index];
    task.planned_submit_count = normalize_planned_submit_count(count);
    if !is_active_remote_task(task) {
        if needs_more_successful_submits(task) {
            if task.status == "succeeded" || task.status == "failed" {
                task.status = "queued".to_string();
                task.next_run_at = None;
                task.last_error.clear();
                task.finished_at.clear();
            }
        } else {
            task.status = "succeeded".to_string();
        }
    }
    task.updated_at = now_rfc3339();
    Ok(task.clone())
}

pub fn process_next_due_task(data: &mut AppData) -> Result<Option<ScheduledTask>, SchedulerError> {
    process_next_due_task_with_runner(data, |args| run_dreamina_command(args))
}

pub fn process_next_due_task_with_runner<F>(
    data: &mut AppData,
    mut runner: F,
) -> Result<Option<ScheduledTask>, SchedulerError>
where
    F: FnMut(&[String]) -> Result<(String, String), String>,
{
    if data.tasks.iter().any(|task| task.status == "submitting") {
        return Ok(None);
    }

    let now = Utc::now();
    if let Some(task_id) = data
        .tasks
        .iter()
        .find(|task| {
            (task.status == "querying" || task.status == "submitted")
                && !task.submit_id.trim().is_empty()
                && !task.auto_query_stopped
        })
        .map(|task| {
            if is_backoff_due(task, now) {
                Some(task.id.clone())
            } else {
                None
            }
        })
    {
        let Some(task_id) = task_id else {
            return Ok(None);
        };
        return query_task_once_with_runner(data, &task_id, &mut runner).map(Some);
    }

    if has_concurrency_cooldown(data, now) {
        return Ok(None);
    }

    for task in data
        .tasks
        .iter_mut()
        .filter(|task| task.status == "scheduled" && is_due(task.next_run_at.as_deref(), now))
    {
        task.status = "queued".to_string();
        task.updated_at = now_rfc3339();
    }

    let Some(task_id) = next_submit_task_id(data, now) else {
        return Ok(None);
    };

    submit_task_once_with_runner(data, &task_id, &mut runner).map(Some)
}

pub fn pause_task(data: &mut AppData, task_id: &str) -> Result<ScheduledTask, SchedulerError> {
    let task_index = data
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
    let allowed = ["scheduled", "queued", "retry_wait"];
    if !allowed.contains(&data.tasks[task_index].status.as_str()) {
        return Err(SchedulerError::Io(format!(
            "当前状态 {} 不可暂停",
            data.tasks[task_index].status
        )));
    }
    data.tasks[task_index].status = "paused".to_string();
    data.tasks[task_index].updated_at = now_rfc3339();
    Ok(data.tasks[task_index].clone())
}

pub fn resume_task(
    data: &mut AppData,
    task_id: &str,
    mode: &str,
) -> Result<ScheduledTask, SchedulerError> {
    let task_index = data
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
    if data.tasks[task_index].status != "paused" {
        return Err(SchedulerError::Io(format!(
            "当前状态 {} 不可恢复",
            data.tasks[task_index].status
        )));
    }
    if mode == "immediate" {
        data.tasks[task_index].status = "queued".to_string();
        data.tasks[task_index].scheduled_at = None;
        data.tasks[task_index].next_run_at = None;
    } else {
        let scheduled_at = data.tasks[task_index].scheduled_at.clone();
        if let Some(ref at) = scheduled_at {
            data.tasks[task_index].status = "scheduled".to_string();
            data.tasks[task_index].next_run_at = Some(at.clone());
        } else {
            data.tasks[task_index].status = "queued".to_string();
        }
    }
    data.tasks[task_index].updated_at = now_rfc3339();
    Ok(data.tasks[task_index].clone())
}

pub fn reschedule_task(
    data: &mut AppData,
    task_id: &str,
    new_scheduled_at: &str,
) -> Result<ScheduledTask, SchedulerError> {
    let task_index = data
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
    let allowed = [
        "draft",
        "queued",
        "scheduled",
        "paused",
        "retry_wait",
        "failed",
        "succeeded",
    ];
    if !allowed.contains(&data.tasks[task_index].status.as_str()) {
        return Err(SchedulerError::Io(format!(
            "当前状态 {} 不可重新排期",
            data.tasks[task_index].status
        )));
    }
    if new_scheduled_at.trim().is_empty() {
        ensure_task_has_pending_submit(&mut data.tasks[task_index]);
        data.tasks[task_index].scheduled_at = None;
        data.tasks[task_index].next_run_at = None;
        data.tasks[task_index].status = "queued".to_string();
        data.tasks[task_index].updated_at = now_rfc3339();
        return Ok(data.tasks[task_index].clone());
    }
    if !new_scheduled_at.trim().is_empty() {
        if let Ok(time) = DateTime::parse_from_rfc3339(new_scheduled_at) {
            if time.with_timezone(&Utc) <= Utc::now() {
                return Err(SchedulerError::ScheduledAtInPast);
            }
        }
    }
    ensure_task_has_pending_submit(&mut data.tasks[task_index]);
    data.tasks[task_index].scheduled_at = Some(new_scheduled_at.to_string());
    data.tasks[task_index].next_run_at = Some(new_scheduled_at.to_string());
    data.tasks[task_index].status = "scheduled".to_string();
    data.tasks[task_index].updated_at = now_rfc3339();
    Ok(data.tasks[task_index].clone())
}

pub fn delete_task_from_data(data: &mut AppData, task_id: &str) -> Result<(), SchedulerError> {
    let before = data.tasks.len();
    data.tasks.retain(|task| task.id != task_id);
    if data.tasks.len() == before {
        return Err(SchedulerError::Io(format!("找不到任务：{task_id}")));
    }
    Ok(())
}

/// 更新任务的可编辑字段，保留执行历史（attempts、execution_records、结果、错误记录不变）。
/// save_mode: "task"（重新排队）或 "draft"（保存草稿）
pub fn update_task_from_data(
    data: &mut AppData,
    task_id: &str,
    draft: TaskDraft,
    save_mode: &str,
) -> Result<ScheduledTask, SchedulerError> {
    validate_draft_references(data, &draft)?;

    // 确认任务存在
    if !data.tasks.iter().any(|t| t.id == task_id) {
        return Err(SchedulerError::Io(format!("找不到任务：{task_id}")));
    }

    // 先在不持可变借用的情况下解析命令预览
    let (new_status, new_command_preview, new_scheduled_at, resolved_inputs) =
        if save_mode == "draft" {
            let resolved = resolve_optional_draft_inputs(data, &draft)?;
            let command_preview = match &resolved {
                Some(resolved) => build_multimodal2video_args(&draft, resolved)?,
                None => vec![],
            };
            ("draft".to_string(), command_preview, None, resolved)
        } else {
            let resolved = resolve_task_inputs(&draft, &data.assets, &data.roles)?;
            let command_preview = build_multimodal2video_args(&draft, &resolved)?;
            let scheduled_at = draft.scheduled_at.clone();
            let status = if scheduled_at.is_some() {
                "scheduled".to_string()
            } else {
                "queued".to_string()
            };
            (status, command_preview, scheduled_at, Some(resolved))
        };

    // 统一写入，此时不持有其他 data 借用
    let task = data.tasks.iter_mut().find(|t| t.id == task_id).unwrap();
    task.title = normalize_task_title(&draft.title, &draft.prompt);
    task.prompt = draft.prompt;
    if let Some(resolved) = &resolved_inputs {
        task.image_asset_ids = resolved.image_asset_ids.clone();
        task.audio_asset_ids = resolved.audio_asset_ids.clone();
    } else {
        task.image_asset_ids = draft.image_asset_ids;
        task.audio_asset_ids = draft.audio_asset_ids;
    }
    task.role_ids = draft.role_ids;
    task.manual_mention_ids = draft.manual_mention_ids;
    task.auto_match_roles = draft.auto_match_roles;
    task.params = draft.params;
    task.temp_image_asset_ids = draft.temp_image_asset_ids;
    task.temp_image_paths = draft.temp_image_paths;
    task.updated_at = now_rfc3339();
    task.command_preview = new_command_preview;
    // draft 模式：若任务已处于有效执行状态则保留，不强制回退为 draft
    if save_mode == "draft" && task.status != "draft" {
        // 保留 status / scheduled_at / next_run_at，只更新内容字段
    } else {
        task.status = new_status;
        task.scheduled_at = new_scheduled_at.clone();
        task.next_run_at = new_scheduled_at;
    }

    Ok(data
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .cloned()
        .unwrap())
}

pub fn query_task_once(data: &mut AppData, task_id: &str) -> Result<ScheduledTask, SchedulerError> {
    query_task_once_with_runner(data, task_id, |args| run_dreamina_command(args))
}

/// 同 `query_task_submit_id_once_with_runner`，但跳过 4 小时上限（用于手动查询）
pub fn manual_query_task_submit_id_with_runner<F>(
    data: &mut AppData,
    task_id: &str,
    submit_id: &str,
    mut runner: F,
) -> Result<ScheduledTask, SchedulerError>
where
    F: FnMut(&[String]) -> Result<(String, String), String>,
{
    query_task_submit_id_once_with_runner_inner(data, task_id, submit_id, &mut runner, true)
}

/// Download a list of remote URLs to `results_dir`, returns successfully saved local paths.
/// Skips already-downloaded files (same URL base name already exists).
pub fn download_result_urls(urls: &[String], results_dir: &Path) -> Vec<String> {
    let _ = fs::create_dir_all(results_dir);
    let mut saved = Vec::new();
    for url in urls {
        let ext = url
            .split('?')
            .next()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("mp4");
        let safe_ext = if ["mp4", "mov", "webm", "mkv", "png", "jpg", "jpeg", "webp"].contains(&ext)
        {
            ext
        } else {
            "mp4"
        };
        let id = format!("result_{}", Uuid::new_v4().simple());
        let local_path = results_dir.join(format!("{id}.{safe_ext}"));
        match reqwest::blocking::get(url.as_str()) {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(bytes) = resp.bytes() {
                    if fs::write(&local_path, &bytes).is_ok() {
                        saved.push(local_path.to_string_lossy().to_string());
                    }
                }
            }
            _ => {}
        }
    }
    saved
}

#[derive(Debug, Clone)]
pub struct DueTaskCli {
    pub task_id: String,
    pub args: Vec<String>,
    pub is_query: bool,
}

#[derive(Debug, Clone)]
pub struct DueTaskBuildError {
    pub task_id: String,
    pub task_title: String,
    pub message: String,
}

/// 从快照中探测下一个应执行的 CLI 调用。
/// 不修改 data，供 async 命令在锁外预先构建参数。
pub fn peek_due_task_cli(data: &AppData) -> Result<Option<DueTaskCli>, DueTaskBuildError> {
    if data.tasks.iter().any(|t| t.status == "submitting") {
        return Ok(None);
    }
    let now = Utc::now();
    // 优先处理已提交、正在排队的任务；未到查询退避时阻塞新提交。
    if let Some(task) = data.tasks.iter().find(|t| {
        (t.status == "querying" || t.status == "submitted")
            && !t.submit_id.trim().is_empty()
            && !t.auto_query_stopped
    }) {
        if !is_backoff_due(task, now) {
            return Ok(None);
        }
        // 对于 querying 状态且超过 4 小时的任务，标记停止（不在 peek 中修改数据，返回 None）
        // 让 process_queue_command 的锁内部分处理
        let args = vec![
            "query_result".to_string(),
            format!("--submit_id={}", task.submit_id),
        ];
        return Ok(Some(DueTaskCli {
            task_id: task.id.clone(),
            args,
            is_query: true,
        }));
    }
    if has_concurrency_cooldown(data, now) {
        return Ok(None);
    }
    // 找下一个待提交任务；普通任务优先，队列空闲时再补试瞬时失败任务。
    let Some(task_id) = next_submit_task_id(data, now) else {
        return Ok(None);
    };
    let Some(task) = data.tasks.iter().find(|task| task.id == task_id) else {
        return Ok(None);
    };
    let draft = TaskDraft {
        title: task.title.clone(),
        prompt: task.prompt.clone(),
        image_asset_ids: task.image_asset_ids.clone(),
        audio_asset_ids: task.audio_asset_ids.clone(),
        role_ids: task.role_ids.clone(),
        manual_mention_ids: task.manual_mention_ids.clone(),
        auto_match_roles: task.auto_match_roles,
        params: task.params.clone(),
        scheduled_at: task.scheduled_at.clone(),
        temp_image_asset_ids: task.temp_image_asset_ids.clone(),
        temp_image_paths: task.temp_image_paths.clone(),
    };
    let resolved = resolve_task_inputs(&draft, &data.assets, &data.roles).map_err(|error| {
        DueTaskBuildError {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
            message: format!("构建提交输入失败：{error}"),
        }
    })?;
    let args =
        build_multimodal2video_args(&draft, &resolved).map_err(|error| DueTaskBuildError {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
            message: format!("构建提交命令失败：{error}"),
        })?;
    Ok(Some(DueTaskCli {
        task_id: task.id.clone(),
        args,
        is_query: false,
    }))
}

/// 后台调度循环的空闲短路判定：仅当存在活跃任务、或有到期工作（含构建出错需处理）时才跑重函数。
/// 完全空闲（无活跃任务且 `peek_due_task_cli` 返回 Ok(None)）时返回 false，跳过整份读写与噪音日志。
pub fn should_process_now(data: &AppData) -> bool {
    has_active_tasks(&data.tasks) || !matches!(peek_due_task_cli(data), Ok(None))
}

pub fn query_task_once_with_runner<F>(
    data: &mut AppData,
    task_id: &str,
    mut runner: F,
) -> Result<ScheduledTask, SchedulerError>
where
    F: FnMut(&[String]) -> Result<(String, String), String>,
{
    let submit_id = data
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .map(|task| task.submit_id.clone())
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
    query_task_submit_id_once_with_runner(data, task_id, &submit_id, &mut runner)
}

pub fn query_task_submit_id_once_with_runner<F>(
    data: &mut AppData,
    task_id: &str,
    submit_id: &str,
    mut runner: F,
) -> Result<ScheduledTask, SchedulerError>
where
    F: FnMut(&[String]) -> Result<(String, String), String>,
{
    query_task_submit_id_once_with_runner_inner(data, task_id, submit_id, &mut runner, false)
}

fn query_task_submit_id_once_with_runner_inner<F>(
    data: &mut AppData,
    task_id: &str,
    submit_id: &str,
    runner: &mut F,
    is_manual: bool,
) -> Result<ScheduledTask, SchedulerError>
where
    F: FnMut(&[String]) -> Result<(String, String), String>,
{
    let task_index = data
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
    let submit_id = submit_id.trim().to_string();
    if submit_id.trim().is_empty() {
        return Err(SchedulerError::Io(
            "任务没有 submit_id，无法查询".to_string(),
        ));
    }
    let is_current_submit = data.tasks[task_index].submit_id == submit_id;

    let started_at = now_rfc3339();

    let args = vec![
        "query_result".to_string(),
        format!("--submit_id={submit_id}"),
    ];
    let (stdout, stderr) = match runner(&args) {
        Ok(output) => output,
        Err(message) => {
            if is_current_submit {
                data.tasks[task_index].status = "failed".to_string();
                data.tasks[task_index].last_error = message.clone();
                data.tasks[task_index].updated_at = now_rfc3339();
            }
            if let Some(rec) = data.tasks[task_index]
                .execution_records
                .iter_mut()
                .find(|r| r.submit_id == submit_id)
            {
                let finished = now_rfc3339();
                rec.status = "failed".to_string();
                rec.error_detail = message.clone();
                rec.finished_at = finished.clone();
                rec.query_records.push(TaskAttempt {
                    id: format!("qr_{}", Uuid::new_v4().simple()),
                    started_at,
                    finished_at: finished,
                    status: "failed".to_string(),
                    command_preview: args,
                    stdout: String::new(),
                    stderr: String::new(),
                    error_kind: String::new(),
                    duration_seconds: 0.0,
                    error_detail: message,
                });
            }
            return Ok(data.tasks[task_index].clone());
        }
    };
    let raw = format!("{stdout}\n{stderr}");
    let parsed = parse_query_output(&raw);
    let status_text = parsed.gen_status.clone().unwrap_or_default().to_lowercase();
    let final_status;
    let mut final_result_paths = Vec::new();
    let mut final_result_urls = Vec::new();
    let mut final_error_detail = String::new();
    let mut final_queue_info = None;

    if status_text.contains("success")
        || !parsed.result_paths.is_empty()
        || !parsed.result_urls.is_empty()
    {
        final_status = "succeeded".to_string();
        final_result_paths = parsed.result_paths;
        final_result_urls = parsed.result_urls;
    } else if status_text.contains("fail")
        || status_text.contains("cancel")
        || parsed.error_code.map_or(false, |c| c >= 400)
    {
        final_status = "failed".to_string();
        final_error_detail = parsed.fail_reason.unwrap_or_else(|| raw.trim().to_string());
    } else {
        // 无结果（仍在排队或处理中）
        // 如果 submitted_at 未设置（旧任务或遗漏），在第一次收到"仍在排队"时补填
        if is_current_submit && data.tasks[task_index].submitted_at.is_none() {
            data.tasks[task_index].submitted_at = Some(now_rfc3339());
        }

        let now = Utc::now();
        let has_remote_progress_info = query_output_has_remote_progress_info(&parsed, &raw);
        if !is_manual
            && is_current_submit
            && !has_remote_progress_info
            && is_past_no_remote_queue_info_wait(&data.tasks[task_index], now)
        {
            final_status = "failed".to_string();
            final_error_detail = format!(
                "查询超过 {} 分钟仍未返回远端队列信息，疑似提交未真正进入生成队列",
                MAX_NO_REMOTE_QUEUE_INFO_MINUTES
            );
            reset_query_backoff(&mut data.tasks[task_index]);
        } else if !is_manual && is_current_submit && is_past_max_wait(&data.tasks[task_index], now)
        {
            // 检查是否超过 4 小时等待上限（手动查询不触发该限制）
            final_status = "submitted".to_string();
            final_error_detail = format!(
                "自动查询已停止（已等待超过 {} 小时），请手动查询",
                MAX_WAIT_HOURS
            );
            data.tasks[task_index].auto_query_stopped = true;
            data.tasks[task_index].last_error = final_error_detail.clone();
        } else {
            final_status = "querying".to_string();
            if is_current_submit {
                // 更新退避状态
                update_query_backoff(&mut data.tasks[task_index]);
            }
            if let Some(reason) = parsed.fail_reason {
                final_error_detail = reason;
            }
            if let Some(qi) = parsed.queue_info {
                final_queue_info = Some(qi);
            }
        }
    }
    let finished = now_rfc3339();
    if is_current_submit {
        data.tasks[task_index].status = final_status.clone();
        data.tasks[task_index].updated_at = finished.clone();
        if final_status == "succeeded" {
            data.tasks[task_index].result_paths = final_result_paths.clone();
            data.tasks[task_index].result_urls = final_result_urls.clone();
            data.tasks[task_index].finished_at = finished.clone();
            data.tasks[task_index].last_error.clear();
            data.tasks[task_index].queue_info = None;
        } else if final_status == "failed" || final_status == "query_timeout" {
            data.tasks[task_index].finished_at = finished.clone();
            data.tasks[task_index].queue_info = None;
            data.tasks[task_index].last_error = final_error_detail.clone();
        } else {
            data.tasks[task_index].queue_info = final_queue_info.clone();
            data.tasks[task_index].last_error = final_error_detail.clone();
        }
    }
    let duration_secs = calc_duration_seconds(&started_at, &finished);
    let query_record = TaskAttempt {
        id: format!("qr_{}", Uuid::new_v4().simple()),
        started_at: started_at.clone(),
        finished_at: finished.clone(),
        status: final_status.clone(),
        command_preview: args.clone(),
        stdout: truncate_log(&stdout),
        stderr: truncate_log(&stderr),
        error_kind: String::new(),
        duration_seconds: duration_secs,
        error_detail: final_error_detail.clone(),
    };
    // 兼容旧字段：同时写入顶层 attempts
    if is_current_submit {
        data.tasks[task_index].attempts.push(TaskAttempt {
            id: format!("attempt_{}", Uuid::new_v4().simple()),
            started_at,
            finished_at: finished.clone(),
            status: final_status.clone(),
            command_preview: args,
            stdout: truncate_log(&stdout),
            stderr: truncate_log(&stderr),
            error_kind: String::new(),
            duration_seconds: duration_secs,
            error_detail: final_error_detail.clone(),
        });
    }
    // 查询记录追加到对应执行记录的 query_records
    if let Some(rec) = data.tasks[task_index]
        .execution_records
        .iter_mut()
        .find(|r| r.submit_id == submit_id)
    {
        rec.query_records.push(query_record);
        rec.status = final_status.clone();
        if final_status == "succeeded" {
            rec.result_paths = final_result_paths;
            rec.result_urls = final_result_urls;
            rec.finished_at = finished;
        } else if final_status == "failed" || final_status == "query_timeout" {
            rec.error_detail = final_error_detail;
            rec.finished_at = finished;
        } else {
            rec.error_detail = final_error_detail;
        }
    }
    apply_planned_submit_completion(&mut data.tasks[task_index]);
    Ok(data.tasks[task_index].clone())
}

pub fn submit_task_once(
    data: &mut AppData,
    task_id: &str,
) -> Result<ScheduledTask, SchedulerError> {
    submit_task_once_with_runner(data, task_id, |args| run_dreamina_command(args))
}

pub fn submit_task_once_with_runner<F>(
    data: &mut AppData,
    task_id: &str,
    mut runner: F,
) -> Result<ScheduledTask, SchedulerError>
where
    F: FnMut(&[String]) -> Result<(String, String), String>,
{
    let task_index = data
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
    let task = data.tasks[task_index].clone();
    let draft = TaskDraft {
        title: task.title.clone(),
        prompt: task.prompt.clone(),
        image_asset_ids: task.image_asset_ids.clone(),
        audio_asset_ids: task.audio_asset_ids.clone(),
        role_ids: task.role_ids.clone(),
        manual_mention_ids: task.manual_mention_ids.clone(),
        auto_match_roles: task.auto_match_roles,
        params: task.params.clone(),
        scheduled_at: task.scheduled_at.clone(),
        temp_image_asset_ids: task.temp_image_asset_ids.clone(),
        temp_image_paths: task.temp_image_paths.clone(),
    };
    let resolved = resolve_task_inputs(&draft, &data.assets, &data.roles)?;
    let args = build_multimodal2video_args(&draft, &resolved)?;
    let started_at = now_rfc3339();

    data.tasks[task_index].status = "submitting".to_string();
    data.tasks[task_index].attempt_count += 1;
    data.tasks[task_index].command_preview = args.clone();
    archive_task_results_into_matching_execution_record(&mut data.tasks[task_index]);
    data.tasks[task_index].submit_id.clear();
    data.tasks[task_index].result_paths.clear();
    data.tasks[task_index].result_urls.clear();
    data.tasks[task_index].finished_at.clear();
    data.tasks[task_index].submitted_at = None;
    data.tasks[task_index].queue_info = None;
    data.tasks[task_index].last_error.clear();
    data.tasks[task_index].updated_at = now_rfc3339();
    // 重置退避状态
    reset_query_backoff(&mut data.tasks[task_index]);
    let preflight_detail =
        build_submit_preflight_detail(&data.tasks[task_index], &resolved, &data.assets);
    let log_task = data.tasks[task_index].clone();
    append_task_log(
        data,
        &log_task,
        LogEntryDraft {
            level: LogLevel::Info,
            source: LogSource::Worker,
            category: "task".to_string(),
            event_type: "submit_preflight".to_string(),
            message: format!("提交前素材诊断：{}", log_task.title),
            detail: preflight_detail.clone(),
            task_id: None,
            task_title: None,
            submit_id: None,
            execution_record_id: None,
            error_detail: None,
            raw_output: None,
            stdout: None,
            stderr: None,
            module: Some("submit".to_string()),
        },
    );

    let (stdout, stderr) = match runner(&args) {
        Ok(output) => output,
        Err(message) => {
            let finished = now_rfc3339();
            let duration_secs = calc_duration_seconds(&started_at, &finished);
            let classified = classify_dreamina_error(&message, &data.settings);
            let concurrency_retry_max_attempts = data.settings.concurrency_retry_max_attempts;
            let next_status = apply_submit_error_retry_state(
                &mut data.tasks[task_index],
                &classified,
                concurrency_retry_max_attempts,
            );
            let error_kind = format!("{:?}", classified.kind);
            let display_error_detail = compact_submit_error_detail(&error_kind, &message);
            let diagnostic_error_detail =
                format!("{message}\n\nsubmit_preflight={preflight_detail}");
            data.tasks[task_index].status = next_status.clone();
            data.tasks[task_index].last_error = message.clone();
            data.tasks[task_index].updated_at = finished.clone();
            let input_snapshot = TaskExecutionInputSnapshot {
                prompt: task.prompt.clone(),
                image_asset_ids: task.image_asset_ids.clone(),
                audio_asset_ids: task.audio_asset_ids.clone(),
                role_ids: task.role_ids.clone(),
                manual_mention_ids: task.manual_mention_ids.clone(),
                auto_match_roles: task.auto_match_roles,
                params: task.params.clone(),
                temp_image_asset_ids: task.temp_image_asset_ids.clone(),
            };
            data.tasks[task_index].attempts.push(TaskAttempt {
                id: format!("attempt_{}", Uuid::new_v4().simple()),
                started_at: started_at.clone(),
                finished_at: finished.clone(),
                status: next_status.clone(),
                command_preview: args.clone(),
                stdout: String::new(),
                stderr: truncate_log(&message),
                error_kind: error_kind.clone(),
                duration_seconds: duration_secs,
                error_detail: diagnostic_error_detail.clone(),
            });
            upsert_submit_execution_record(
                &mut data.tasks[task_index],
                TaskExecutionRecord {
                    id: format!("exec_{}", Uuid::new_v4().simple()),
                    submit_id: String::new(),
                    status: next_status,
                    started_at,
                    finished_at: finished,
                    input_snapshot,
                    command_preview: args,
                    query_records: vec![],
                    result_paths: vec![],
                    result_urls: vec![],
                    error_kind,
                    error_detail: display_error_detail,
                },
            );
            return Ok(data.tasks[task_index].clone());
        }
    };
    let raw = format!("{}\n{}", stdout, stderr);
    let parsed = parse_submit_output(&raw);
    let mut error_kind = String::new();
    let mut record_submit_id = String::new();

    let submit_failed = parsed
        .gen_status
        .as_deref()
        .map(|status| {
            let normalized = status.trim().to_ascii_lowercase();
            normalized.contains("fail") || normalized.contains("cancel")
        })
        .unwrap_or(false);
    let submit_failure_message = parsed
        .fail_reason
        .clone()
        .unwrap_or_else(|| raw.trim().to_string());

    if let Some(submit_id) = parsed.submit_id {
        record_submit_id = submit_id.clone();
        if submit_failed {
            let classified = classify_dreamina_error(&submit_failure_message, &data.settings);
            error_kind = format!("{:?}", classified.kind);
            let concurrency_retry_max_attempts = data.settings.concurrency_retry_max_attempts;
            let next_status = apply_submit_error_retry_state(
                &mut data.tasks[task_index],
                &classified,
                concurrency_retry_max_attempts,
            );
            data.tasks[task_index].submit_id.clear();
            data.tasks[task_index].submitted_at = None;
            data.tasks[task_index].status = next_status;
            data.tasks[task_index].last_error = submit_failure_message;
        } else {
            data.tasks[task_index].submit_id = submit_id;
            data.tasks[task_index].submitted_at = Some(started_at.clone());
            data.tasks[task_index].server_error_retry_count = 0;
            data.tasks[task_index].concurrency_retry_count = 0;
            data.tasks[task_index].status = if parsed.gen_status.as_deref() == Some("success") {
                "succeeded".to_string()
            } else if data.settings.auto_query_enabled {
                "querying".to_string()
            } else {
                "submitted".to_string()
            };
            data.tasks[task_index].last_error.clear();
        }
    } else {
        let message = parsed.fail_reason.unwrap_or_else(|| raw.trim().to_string());
        // 5xx 服务器错误（如 HTTP 500-599 或 Dreamina 自定义 50000-59999）：
        // 可能是平台侧瞬时故障，自动重试最多 MAX_SERVER_ERROR_RETRIES 次
        if parsed.error_code.map_or(false, |c| {
            (c >= 500 && c < 600) || (c >= 50000 && c < 60000)
        }) {
            data.tasks[task_index].server_error_retry_count += 1;
            error_kind = format!("{:?}", DreaminaErrorKind::Transient);
            if data.tasks[task_index].server_error_retry_count <= MAX_SERVER_ERROR_RETRIES {
                data.tasks[task_index].status = "retry_wait".to_string();
                data.tasks[task_index].next_run_at = Some(
                    (Utc::now()
                        + Duration::seconds(data.settings.concurrency_retry_delay_seconds as i64))
                    .to_rfc3339(),
                );
            } else {
                data.tasks[task_index].status = "failed".to_string();
            }
            data.tasks[task_index].last_error = message;
        } else {
            let classified = classify_dreamina_error(&message, &data.settings);
            error_kind = format!("{:?}", classified.kind);
            let concurrency_retry_max_attempts = data.settings.concurrency_retry_max_attempts;
            let next_status = apply_submit_error_retry_state(
                &mut data.tasks[task_index],
                &classified,
                concurrency_retry_max_attempts,
            );
            data.tasks[task_index].status = next_status;
            data.tasks[task_index].last_error = message;
        } // end else (non-5xx)
    }
    data.tasks[task_index].updated_at = now_rfc3339();
    let final_status = data.tasks[task_index].status.clone();
    let finished = now_rfc3339();
    let duration_secs = calc_duration_seconds(&started_at, &finished);
    let error_detail = data.tasks[task_index].last_error.clone();
    let diagnostic_error_detail = if error_detail.is_empty() {
        String::new()
    } else {
        format!("{error_detail}\n\nsubmit_preflight={preflight_detail}")
    };
    let display_error_detail = if error_detail.is_empty() {
        String::new()
    } else if final_status == "failed" {
        compact_failed_submit_error_detail(&error_kind, &error_detail)
    } else {
        compact_submit_error_detail(&error_kind, &error_detail)
    };
    let submit_id_now = if record_submit_id.is_empty() {
        data.tasks[task_index].submit_id.clone()
    } else {
        record_submit_id
    };
    data.tasks[task_index].attempts.push(TaskAttempt {
        id: format!("attempt_{}", Uuid::new_v4().simple()),
        started_at: started_at.clone(),
        finished_at: finished.clone(),
        status: final_status.clone(),
        command_preview: args.clone(),
        stdout: truncate_log(&stdout),
        stderr: truncate_log(&stderr),
        error_kind: error_kind.clone(),
        duration_seconds: duration_secs,
        error_detail: diagnostic_error_detail.clone(),
    });
    // 每次真实提交生成一条执行记录，携带输入快照
    let input_snapshot = TaskExecutionInputSnapshot {
        prompt: task.prompt.clone(),
        image_asset_ids: task.image_asset_ids.clone(),
        audio_asset_ids: task.audio_asset_ids.clone(),
        role_ids: task.role_ids.clone(),
        manual_mention_ids: task.manual_mention_ids.clone(),
        auto_match_roles: task.auto_match_roles,
        params: task.params.clone(),
        temp_image_asset_ids: task.temp_image_asset_ids.clone(),
    };
    upsert_submit_execution_record(
        &mut data.tasks[task_index],
        TaskExecutionRecord {
            id: format!("exec_{}", Uuid::new_v4().simple()),
            submit_id: submit_id_now,
            status: final_status,
            started_at,
            finished_at: finished,
            input_snapshot,
            command_preview: args,
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind,
            error_detail: display_error_detail,
        },
    );
    apply_planned_submit_completion(&mut data.tasks[task_index]);
    Ok(data.tasks[task_index].clone())
}

fn calc_duration_seconds(started_at: &str, finished_at: &str) -> f64 {
    let start = DateTime::parse_from_rfc3339(started_at)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(Utc::now());
    let end = DateTime::parse_from_rfc3339(finished_at)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(Utc::now());
    (end - start).num_seconds() as f64
}

#[derive(Debug, Clone)]
struct MentionAssetCandidate {
    kind: AssetKind,
    asset_id: String,
}

fn build_mention_asset_index(
    task: &TaskDraft,
    assets: &[Asset],
    roles: &[Role],
) -> HashMap<String, MentionAssetCandidate> {
    let asset_by_id = assets
        .iter()
        .map(|asset| (asset.id.as_str(), asset))
        .collect::<HashMap<_, _>>();
    let mut index = HashMap::new();

    for asset_id in task
        .image_asset_ids
        .iter()
        .chain(task.audio_asset_ids.iter())
    {
        if let Some(asset) = asset_by_id.get(asset_id.as_str()) {
            insert_asset_mention_labels(&mut index, asset, None);
        }
    }

    for asset in assets {
        insert_asset_mention_labels(&mut index, asset, None);
    }

    for (idx, asset_id) in task.image_asset_ids.iter().enumerate() {
        if let Some(asset) = asset_by_id.get(asset_id.as_str()) {
            insert_mention_label(
                &mut index,
                format!("图{}", idx + 1),
                asset.kind.clone(),
                asset.id.clone(),
            );
        }
    }
    for (idx, asset_id) in task.audio_asset_ids.iter().enumerate() {
        if let Some(asset) = asset_by_id.get(asset_id.as_str()) {
            insert_mention_label(
                &mut index,
                format!("音频{}", idx + 1),
                asset.kind.clone(),
                asset.id.clone(),
            );
        }
    }

    let storyboard_asset_ids = storyboard_asset_ids_for_mentions(task, &asset_by_id);
    for (idx, asset_id) in storyboard_asset_ids.iter().enumerate() {
        if let Some(asset) = asset_by_id.get(asset_id.as_str()) {
            insert_mention_label(
                &mut index,
                format!("图片{}", idx + 1),
                asset.kind.clone(),
                asset.id.clone(),
            );
            insert_mention_label(
                &mut index,
                format!("分镜图{}", idx + 1),
                asset.kind.clone(),
                asset.id.clone(),
            );
        }
    }

    for role in roles {
        for asset_id in &role.asset_ids {
            if let Some(asset) = asset_by_id.get(asset_id.as_str()) {
                insert_asset_mention_labels(&mut index, asset, Some(role));
            }
        }
    }

    index
}

fn storyboard_asset_ids_for_mentions(
    task: &TaskDraft,
    asset_by_id: &HashMap<&str, &Asset>,
) -> Vec<String> {
    let explicit_temp_ids = task
        .temp_image_asset_ids
        .iter()
        .filter(|id| {
            asset_by_id
                .get(id.as_str())
                .map(|asset| asset.kind == AssetKind::Image)
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !explicit_temp_ids.is_empty() {
        return explicit_temp_ids;
    }

    task.image_asset_ids
        .iter()
        .filter_map(|id| {
            asset_by_id.get(id.as_str()).and_then(|asset| {
                if asset.kind == AssetKind::Image && is_temporary_image_asset(asset) {
                    Some((*id).clone())
                } else {
                    None
                }
            })
        })
        .collect()
}

fn resolve_storyboard_fallback_candidate(
    mention: &str,
    task: &TaskDraft,
    asset_by_id: &HashMap<&str, &Asset>,
    current_image_asset_ids: &[String],
) -> Option<MentionAssetCandidate> {
    let requested_index = storyboard_mention_index(mention)?;
    let storyboard_asset_ids = storyboard_asset_ids_for_mentions(task, asset_by_id);
    let asset_id = storyboard_asset_ids
        .get(requested_index.saturating_sub(1))
        .or_else(|| {
            storyboard_asset_ids
                .iter()
                .find(|id| !current_image_asset_ids.contains(*id))
        })
        .or_else(|| storyboard_asset_ids.first())?;
    let asset = asset_by_id.get(asset_id.as_str())?;
    Some(MentionAssetCandidate {
        kind: asset.kind.clone(),
        asset_id: asset.id.clone(),
    })
}

fn storyboard_mention_index(mention: &str) -> Option<usize> {
    ["分镜图", "图片"]
        .iter()
        .find_map(|prefix| mention.strip_prefix(prefix))
        .filter(|value| !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn is_temporary_image_asset(asset: &Asset) -> bool {
    asset
        .tags
        .iter()
        .any(|tag| matches!(tag.as_str(), "temp_image" | "temporary" | "clipboard"))
        || asset.source_path == "clipboard"
}

fn insert_asset_mention_labels(
    index: &mut HashMap<String, MentionAssetCandidate>,
    asset: &Asset,
    role: Option<&Role>,
) {
    insert_mention_label(
        index,
        asset.name.clone(),
        asset.kind.clone(),
        asset.id.clone(),
    );
    insert_mention_label(
        index,
        asset.id.clone(),
        asset.kind.clone(),
        asset.id.clone(),
    );
    for alias in &asset.aliases {
        insert_mention_label(index, alias.clone(), asset.kind.clone(), asset.id.clone());
    }
    if let Some(role) = role {
        let role_names = std::iter::once(role.name.as_str())
            .chain(role.aliases.iter().map(String::as_str))
            .chain(role.tags.iter().map(String::as_str));
        for role_name in role_names {
            insert_mention_label(
                index,
                format!("{role_name}{}", asset.name),
                asset.kind.clone(),
                asset.id.clone(),
            );
            for alias in &asset.aliases {
                insert_mention_label(
                    index,
                    format!("{role_name}{alias}"),
                    asset.kind.clone(),
                    asset.id.clone(),
                );
            }
        }
    }
}

fn insert_mention_label(
    index: &mut HashMap<String, MentionAssetCandidate>,
    label: String,
    kind: AssetKind,
    asset_id: String,
) {
    let label = label.trim();
    if label.is_empty() {
        return;
    }
    index
        .entry(label.to_string())
        .or_insert(MentionAssetCandidate { kind, asset_id });
}

fn push_prompt_rewrite(
    rewrites: &mut Vec<PromptMentionRewrite>,
    original: String,
    replacement: String,
) {
    if rewrites.iter().any(|item| item.original == original) {
        return;
    }
    rewrites.push(PromptMentionRewrite {
        original,
        replacement,
    });
}

fn rewrite_prompt_mentions(prompt: &str, rewrites: &[PromptMentionRewrite]) -> String {
    let rewrite_by_original = rewrites
        .iter()
        .map(|item| (item.original.as_str(), item.replacement.as_str()))
        .collect::<HashMap<_, _>>();
    let mut output = String::with_capacity(prompt.len());
    let mut iter = prompt.char_indices().peekable();

    while let Some((start, ch)) = iter.next() {
        if ch != '@' {
            output.push(ch);
            continue;
        }

        let mention_start = start + ch.len_utf8();
        let mut end = mention_start;
        while let Some((idx, next)) = iter.peek().copied() {
            if next == '@' || next.is_whitespace() || is_mention_punctuation(next) {
                break;
            }
            iter.next();
            end = idx + next.len_utf8();
        }

        let raw = &prompt[mention_start..end];
        let cleaned = trim_mention_token(raw);
        if let Some(replacement) = rewrite_by_original.get(cleaned) {
            if let Some(pos) = raw.find(cleaned) {
                output.push('@');
                output.push_str(&raw[..pos]);
                output.push_str(replacement);
                output.push_str(&raw[pos + cleaned.len()..]);
            } else {
                output.push('@');
                output.push_str(replacement);
            }
        } else {
            output.push('@');
            output.push_str(raw);
        }
    }

    output
}

fn push_asset_id(
    asset_id: &str,
    expected_kind: AssetKind,
    asset_by_id: &HashMap<&str, &Asset>,
    image_asset_ids: &mut Vec<String>,
    audio_asset_ids: &mut Vec<String>,
) -> Result<(), SchedulerError> {
    let asset = asset_by_id
        .get(asset_id)
        .ok_or_else(|| SchedulerError::MissingAsset(asset_id.to_string()))?;
    if asset.kind == expected_kind {
        push_known_asset(asset, image_asset_ids, audio_asset_ids);
    }
    Ok(())
}

fn push_known_asset(
    asset: &Asset,
    image_asset_ids: &mut Vec<String>,
    audio_asset_ids: &mut Vec<String>,
) {
    match asset.kind {
        AssetKind::Image => push_unique(image_asset_ids, asset.id.clone()),
        AssetKind::Audio => push_unique(audio_asset_ids, asset.id.clone()),
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.trim().is_empty() && !values.contains(&value) {
        values.push(value);
    }
}

fn ids_to_paths(
    ids: &[String],
    asset_by_id: &HashMap<&str, &Asset>,
) -> Result<Vec<String>, SchedulerError> {
    ids.iter()
        .map(|id| {
            asset_by_id
                .get(id.as_str())
                .map(|asset| asset.stored_path.clone())
                .ok_or_else(|| SchedulerError::MissingAsset(id.clone()))
        })
        .collect()
}

fn validate_input_counts(
    image_asset_ids: &[String],
    audio_asset_ids: &[String],
) -> Result<(), SchedulerError> {
    if image_asset_ids.is_empty() {
        return Err(SchedulerError::MissingImageInput);
    }
    if image_asset_ids.len() > MAX_IMAGES {
        return Err(SchedulerError::TooManyImages);
    }
    if audio_asset_ids.len() > MAX_AUDIO {
        return Err(SchedulerError::TooManyAudio);
    }
    Ok(())
}

fn validate_video_params(params: &VideoParams) -> Result<(), SchedulerError> {
    if !SUPPORTED_MODELS.contains(&params.model_version.as_str()) {
        return Err(SchedulerError::UnsupportedModel(
            params.model_version.clone(),
        ));
    }
    if !SUPPORTED_RATIOS.contains(&params.ratio.as_str()) {
        return Err(SchedulerError::UnsupportedRatio(params.ratio.clone()));
    }
    if !(4..=15).contains(&params.duration) {
        return Err(SchedulerError::UnsupportedDuration);
    }
    if params.video_resolution != "720p" {
        return Err(SchedulerError::UnsupportedResolution);
    }
    Ok(())
}

fn extract_mentions(prompt: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    let mut iter = prompt.char_indices().peekable();
    while let Some((_start, ch)) = iter.next() {
        if ch != '@' {
            continue;
        }
        let mut token = String::new();
        while let Some((_idx, next)) = iter.peek().copied() {
            if next == '@' || next.is_whitespace() || is_mention_punctuation(next) {
                break;
            }
            iter.next();
            token.push(next);
        }
        let cleaned = trim_mention_token(&token);
        if !cleaned.is_empty() {
            push_unique(&mut mentions, cleaned.to_string());
        }
    }
    mentions
}

fn trim_mention_token(token: &str) -> &str {
    token.trim_matches(is_mention_punctuation).trim()
}

fn is_mention_punctuation(c: char) -> bool {
    "，。,.!?！？、；;：:（）()[]【】\"'".contains(c)
}

fn collect_result_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| "\"',，。[]{}()".contains(c));
        if cleaned.starts_with("http://") || cleaned.starts_with("https://") {
            push_unique(&mut urls, cleaned.to_string());
        }
    }
    urls
}

fn collect_result_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let cleaned = line
            .trim()
            .trim_matches(|c: char| "\"',，。[]{}()".contains(c));
        if cleaned.contains("http://") || cleaned.contains("https://") {
            continue;
        }
        let lower = cleaned.to_lowercase();
        if (lower.ends_with(".mp4")
            || lower.ends_with(".mov")
            || lower.ends_with(".webm")
            || lower.ends_with(".mkv"))
            && !cleaned.starts_with("http://")
            && !cleaned.starts_with("https://")
        {
            let value = cleaned
                .rsplit_once(':')
                .map(|(_, value)| value.trim())
                .unwrap_or(cleaned);
            if !value.is_empty() {
                push_unique(&mut paths, value.to_string());
            }
        }
    }
    paths
}

fn is_concurrency_limit(message: &str) -> bool {
    let text = message.to_lowercase();
    text.contains("exceedconcurrencylimit")
        || text.contains("concurrencylimit")
        || text.contains("ret=1310")
        || text.contains("ret = 1310")
        || message.contains("并发上限")
        || message.contains("并发限制")
}

fn dreamina_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(value) = std::env::var("DREAMINA_BIN") {
        if !value.trim().is_empty() {
            candidates.push(value);
        }
    }
    candidates.push("dreamina".to_string());
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(format!("{home}/.local/bin/dreamina"));
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        candidates.push(format!("{user_profile}\\.local\\bin\\dreamina.exe"));
    }
    candidates
}

fn run_dreamina_command(args: &[String]) -> Result<(String, String), String> {
    let cli = check_dreamina_cli_status();
    if !cli.available {
        return Err(cli.message);
    }
    let output = Command::new(&cli.path)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    Ok((
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn is_due(value: Option<&str>, now: DateTime<Utc>) -> bool {
    match value {
        None => true,
        Some(value) if value.trim().is_empty() => true,
        Some(value) => DateTime::parse_from_rfc3339(value)
            .map(|time| time.with_timezone(&Utc) <= now)
            .unwrap_or(true),
    }
}

fn truncate_log(value: &str) -> String {
    const MAX_LOG_CHARS: usize = 4000;
    if value.chars().count() <= MAX_LOG_CHARS {
        return value.to_string();
    }
    let mut result: String = value.chars().take(MAX_LOG_CHARS).collect();
    result.push_str("\n...日志已截断");
    result
}

/// Field-level truncation for structured log fields (shorter limit than full log).
fn truncate_log_field(value: &str) -> String {
    const MAX_FIELD_CHARS: usize = 4000;
    if value.chars().count() <= MAX_FIELD_CHARS {
        return value.to_string();
    }
    let mut result: String = value.chars().take(MAX_FIELD_CHARS).collect();
    result.push_str("...（已截断）");
    result
}

/// Append a structured log entry, auto-filling id/timestamp and executing retention.
fn append_log(data: &mut AppData, draft: LogEntryDraft) {
    let entry = LogEntry {
        id: format!("log_{}", Uuid::new_v4().simple()),
        timestamp: chrono::Utc::now().to_rfc3339(),
        level: draft.level,
        source: draft.source,
        category: draft.category,
        event_type: draft.event_type,
        message: draft.message,
        detail: draft.detail,
        task_id: draft.task_id,
        task_title: draft.task_title,
        submit_id: draft.submit_id,
        execution_record_id: draft.execution_record_id,
        error_detail: draft.error_detail,
        raw_output: draft.raw_output,
        stdout: draft.stdout,
        stderr: draft.stderr,
        module: draft.module,
        legacy_string: None,
    };
    data.logs.push(entry);
    apply_log_retention(data);
}

/// Append a task-context log entry, auto-injecting task_id/task_title/submit_id.
/// The caller should NOT set task_id/task_title in the draft — they will be overwritten.
fn append_task_log(data: &mut AppData, task: &ScheduledTask, mut draft: LogEntryDraft) {
    draft.task_id = Some(task.id.clone());
    draft.task_title = Some(task.title.clone());
    draft.submit_id = if task.submit_id.is_empty() {
        None
    } else {
        Some(task.submit_id.clone())
    };
    append_log(data, draft);
}

pub fn record_lifecycle_event(data: &mut AppData, event_type: &str, message: &str, detail: &str) {
    append_log(
        data,
        LogEntryDraft {
            level: LogLevel::Info,
            source: LogSource::System,
            category: "lifecycle".to_string(),
            event_type: event_type.to_string(),
            message: message.to_string(),
            detail: detail.to_string(),
            task_id: None,
            task_title: None,
            submit_id: None,
            execution_record_id: None,
            error_detail: None,
            raw_output: None,
            stdout: None,
            stderr: None,
            module: None,
        },
    );
}

pub fn record_scheduler_tick(data: &mut AppData, origin: &str, phase: &str) {
    append_log(
        data,
        LogEntryDraft {
            level: LogLevel::Debug,
            source: LogSource::Scheduler,
            category: "scheduler_tick".to_string(),
            event_type: "tick".to_string(),
            message: format!("调度 tick：{origin}"),
            detail: format!("phase={phase}"),
            task_id: None,
            task_title: None,
            submit_id: None,
            execution_record_id: None,
            error_detail: None,
            raw_output: None,
            stdout: None,
            stderr: None,
            module: None,
        },
    );
}

struct ProcessQueueGuard;

impl Drop for ProcessQueueGuard {
    fn drop(&mut self) {
        PROCESS_QUEUE_RUNNING.store(false, Ordering::Release);
    }
}

fn try_begin_process_queue() -> Option<ProcessQueueGuard> {
    PROCESS_QUEUE_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| ProcessQueueGuard)
}

struct StoreQueueLockGuard {
    path: PathBuf,
}

impl Drop for StoreQueueLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn try_acquire_store_queue_lock(store: &AppStore, origin: &str) -> Option<StoreQueueLockGuard> {
    let lock_path = store.root_dir.join("queue.lock");
    let parent = lock_path.parent()?;
    if fs::create_dir_all(parent).is_err() {
        return None;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut file) => {
            let _ = writeln!(file, "origin={origin}");
            let _ = writeln!(file, "pid={}", std::process::id());
            let _ = writeln!(file, "created_at={}", now_rfc3339());
            Some(StoreQueueLockGuard { path: lock_path })
        }
        Err(_) => None,
    }
}

/// 每条执行记录保留的 query_records（自动轮询历史）上限。
const MAX_QUERY_RECORDS_PER_EXECUTION: usize = 30;
/// 每个任务保留的 attempts（尝试历史）上限。
const MAX_ATTEMPTS_PER_TASK: usize = 50;

/// 丢弃 `v` 中最旧的若干项，仅保留最近 `max` 个；返回移除数量。
fn cap_history_vec<T>(v: &mut Vec<T>, max: usize) -> usize {
    if v.len() > max {
        let remove = v.len() - max;
        v.drain(0..remove);
        remove
    } else {
        0
    }
}

/// 裁剪无界增长的历史：每任务 attempts 留最近 50，每执行记录 query_records 留最近 30。
/// 只裁剪过程态轮询/尝试历史，不动结果、状态、成败等顶层字段。返回移除的总条数。
pub fn cap_execution_history(data: &mut AppData) -> usize {
    let mut removed = 0;
    for task in &mut data.tasks {
        removed += cap_history_vec(&mut task.attempts, MAX_ATTEMPTS_PER_TASK);
        for record in &mut task.execution_records {
            removed += cap_history_vec(&mut record.query_records, MAX_QUERY_RECORDS_PER_EXECUTION);
        }
    }
    removed
}

/// Trim old log entries to respect `settings.log_retention_count`.
fn apply_log_retention(data: &mut AppData) {
    let max_logs = data.settings.log_retention_count as usize;
    if max_logs > 0 && data.logs.len() > max_logs {
        let drain = data.logs.len() - max_logs;
        data.logs.drain(0..drain);
    }
}

pub fn process_queue_for_store_blocking(
    store: &AppStore,
    origin: &str,
) -> Result<Option<ScheduledTask>, String> {
    let _ = store.mutate(|data| {
        record_scheduler_tick(data, origin, "started");
        Ok(())
    });

    let Some(_guard) = try_begin_process_queue() else {
        let _ = store.mutate(|data| {
            record_scheduler_tick(data, origin, "skipped_busy");
            Ok(())
        });
        return Ok(None);
    };

    let Some(_store_lock) = try_acquire_store_queue_lock(store, origin) else {
        let _ = store.mutate(|data| {
            record_scheduler_tick(data, origin, "skipped_busy");
            Ok(())
        });
        return Ok(None);
    };

    // 锁外：探测下一步应执行的 CLI 调用
    let due = {
        let data = store.snapshot();
        peek_due_task_cli(&data)
    };
    let DueTaskCli {
        task_id,
        args,
        is_query,
    } = match due {
        Ok(None) => {
            let _ = store.mutate(|data| {
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Debug,
                        source: LogSource::Scheduler,
                        category: "queue".to_string(),
                        event_type: "no_due_task".to_string(),
                        message: "队列暂无到期任务".to_string(),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(())
            });
            return Ok(None);
        }
        Ok(Some(x)) => x,
        Err(error) => {
            let task = store
                .mutate(|data| {
                    if let Some(task) = data.tasks.iter_mut().find(|task| task.id == error.task_id)
                    {
                        task.status = "failed".to_string();
                        task.next_run_at = None;
                        task.last_error = error.message.clone();
                        task.updated_at = now_rfc3339();
                    }
                    append_log(
                        data,
                        LogEntryDraft {
                            level: LogLevel::Error,
                            source: LogSource::Scheduler,
                            category: "queue".to_string(),
                            event_type: "build_due_task_failed".to_string(),
                            message: format!("到期任务构建失败：{}", error.task_title),
                            detail: error.message.clone(),
                            task_id: Some(error.task_id.clone()),
                            task_title: Some(error.task_title.clone()),
                            submit_id: None,
                            execution_record_id: None,
                            error_detail: Some(error.message.clone()),
                            raw_output: None,
                            stdout: None,
                            stderr: None,
                            module: Some("scheduler".to_string()),
                        },
                    );
                    Ok(data
                        .tasks
                        .iter()
                        .find(|task| task.id == error.task_id)
                        .cloned())
                })
                .map_err(|error| error.to_string())?;
            return Ok(task);
        }
    };

    // 锁外：运行 CLI
    let cli_result = run_dreamina_command(&args);

    // 锁内：写回（使用 replay runner）
    let task = store
        .mutate(|data| {
            // scheduled → queued 迁移（仅 submit 路径需要）
            if !is_query {
                let now = Utc::now();
                let mut log_drafts: Vec<LogEntryDraft> = Vec::new();
                for t in &mut data.tasks {
                    if t.status == "scheduled" && is_due(t.next_run_at.as_deref(), now) {
                        log_drafts.push(LogEntryDraft {
                            level: LogLevel::Warn,
                            source: LogSource::Scheduler,
                            category: "queue".to_string(),
                            event_type: "expired_compensation".to_string(),
                            message: format!(
                                "预定已到期，恢复后补偿处理：{}（原计划 {}）",
                                t.title,
                                t.scheduled_at.as_deref().unwrap_or("-")
                            ),
                            detail: String::new(),
                            task_id: Some(t.id.clone()),
                            task_title: Some(t.title.clone()),
                            submit_id: None,
                            execution_record_id: None,
                            error_detail: None,
                            raw_output: None,
                            stdout: None,
                            stderr: None,
                            module: None,
                        });
                        t.status = "queued".to_string();
                        t.updated_at = now_rfc3339();
                    }
                }
                for draft in log_drafts {
                    append_log(data, draft);
                }
            }
            let task = if is_query {
                query_task_once_with_runner(data, &task_id, |_| cli_result.clone())?
            } else {
                submit_task_once_with_runner(data, &task_id, |_| cli_result.clone())?
            };
            append_task_log(
                data,
                &task,
                LogEntryDraft {
                    level: match task.status.as_str() {
                        "failed" => LogLevel::Error,
                        "succeeded" => LogLevel::Success,
                        _ => LogLevel::Info,
                    },
                    source: LogSource::Scheduler,
                    category: "queue".to_string(),
                    event_type: "execute".to_string(),
                    message: format!("队列执行：{} -> {}", task.title, task.status),
                    detail: String::new(),
                    task_id: None,
                    task_title: None,
                    submit_id: None,
                    execution_record_id: None,
                    error_detail: None,
                    raw_output: None,
                    stdout: None,
                    stderr: None,
                    module: None,
                },
            );
            Ok(task)
        })
        .map_err(|error| error.to_string())?;
    if task.status == "succeeded" && !task.result_urls.is_empty() {
        let results_dir = store.assets_dir().join("results");
        let urls = task.result_urls.clone();
        let tid = task.id.clone();
        let downloaded = download_result_urls(&urls, &results_dir);
        if !downloaded.is_empty() {
            let _ = store.mutate(|data| {
                if let Some(t) = data.tasks.iter_mut().find(|t| t.id == tid) {
                    for p in &downloaded {
                        if !t.result_paths.contains(p) {
                            t.result_paths.push(p.clone());
                        }
                    }
                }
                Ok(())
            });
        }
    }
    store
        .snapshot()
        .tasks
        .into_iter()
        .find(|t| t.id == task.id)
        .map(Some)
        .ok_or_else(|| "任务不存在".to_string())
}

fn start_background_scheduler(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        let store = app_handle.state::<AppStore>();
        let waker = app_handle.state::<SchedulerWaker>();

        // 单次快照供「空闲短路」与「等待时长」共用，避免一个 tick 内重复读盘。
        let snapshot = store.snapshot();

        // 完全空闲时跳过重函数：不写 started/no_due_task 噪音日志、不整份序列化落盘。
        if should_process_now(&snapshot) {
            if let Err(error) = process_queue_for_store_blocking(&store, "background") {
                let _ = store.mutate(|data| {
                    append_log(
                        data,
                        LogEntryDraft {
                            level: LogLevel::Error,
                            source: LogSource::Scheduler,
                            category: "scheduler_tick".to_string(),
                            event_type: "tick_error".to_string(),
                            message: "后台调度 tick 失败".to_string(),
                            detail: String::new(),
                            task_id: None,
                            task_title: None,
                            submit_id: None,
                            execution_record_id: None,
                            error_detail: Some(error),
                            raw_output: None,
                            stdout: None,
                            stderr: None,
                            module: None,
                        },
                    );
                    Ok(())
                });
            }
        }

        // 自适应等待：有活跃任务 30s，完全空闲 60s；入队 notify 可提前唤醒。
        let wait = compute_wait_duration(&snapshot.tasks);
        waker.wait(wait);
    });
}

fn log_lifecycle_from_manager<R, M>(manager: &M, event_type: &str, message: &str, detail: &str)
where
    R: tauri::Runtime,
    M: Manager<R>,
{
    let store = manager.state::<AppStore>();
    let _ = store.mutate(|data| {
        record_lifecycle_event(data, event_type, message, detail);
        Ok(())
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppStore::load(default_store_dir()))
        .manage(keep_awake::KeepAwakeGuard::new())
        .manage(SchedulerWaker::new())
        .setup(|app| {
            log_lifecycle_from_manager(app, "app_start", "应用启动", "");
            start_background_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                log_lifecycle_from_manager(window, "window_close_requested", "窗口请求关闭", "");
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_state,
            commands::get_state_signature,
            commands::get_host_platform,
            commands::check_dreamina_cli,
            commands::get_dreamina_credit,
            commands::import_temp_image_command,
            commands::import_asset,
            commands::save_clipboard_image_command,
            commands::paste_clipboard_image_command,
            commands::rename_asset,
            commands::import_role_media_command,
            commands::remove_role_media_command,
            commands::create_role_command,
            commands::update_role_command,
            commands::delete_role_command,
            commands::preview_task_command,
            commands::save_task_draft_command,
            commands::create_task_command,
            commands::submit_task_command,
            commands::query_task_command,
            commands::set_task_planned_submit_count_command,
            commands::process_queue_command,
            commands::generate_task_title_command,
            commands::test_ai_model_command,
            commands::generate_image_command,
            commands::query_image_task_command,
            commands::retry_query_image_task_command,
            commands::regenerate_image_command,
            commands::download_imagegen_image_command,
            commands::copy_imagegen_image_command,
            commands::delete_imagegen_history_item_command,
            commands::clear_imagegen_history_command,
            commands::update_settings_command,
            commands::pause_task_command,
            commands::resume_task_command,
            commands::reschedule_task_command,
            commands::open_result_dir_command,
            commands::download_result_url_command,
            commands::install_dreamina_cli_command,
            commands::login_dreamina_cli_command,
            commands::delete_task_command,
            commands::delete_execution_record_command,
            commands::update_task_command,
            commands::update_task_draft_command,
            commands::clear_logs_command,
            commands::record_lifecycle_event_command,
            commands::sync_keep_awake_command,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Dreamina Scheduler")
        .run(|app_handle, event| match event {
            tauri::RunEvent::Resumed => {
                log_lifecycle_from_manager(app_handle, "app_resumed", "应用事件循环恢复", "");
            }
            tauri::RunEvent::ExitRequested { .. } => {
                log_lifecycle_from_manager(app_handle, "app_exit_requested", "应用请求退出", "");
            }
            tauri::RunEvent::Exit => {
                log_lifecycle_from_manager(app_handle, "app_exit", "应用退出", "");
            }
            _ => {}
        });
}

pub mod commands {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine as _;
    use tauri::State;

    #[tauri::command]
    pub fn get_app_state(store: State<'_, AppStore>) -> AppData {
        store.snapshot()
    }

    /// 廉价变更签名（仅 stat，不读取整份状态）。前端轮询比对，未变则跳过 `get_app_state`。
    #[tauri::command]
    pub fn get_state_signature(store: State<'_, AppStore>) -> String {
        store.state_signature()
    }

    #[tauri::command]
    pub fn get_host_platform() -> HostPlatform {
        host_platform()
    }

    #[tauri::command]
    pub fn check_dreamina_cli() -> CliStatus {
        check_dreamina_cli_status()
    }

    #[tauri::command]
    pub async fn get_dreamina_credit() -> Result<CreditInfo, String> {
        tauri::async_runtime::spawn_blocking(|| {
            get_dreamina_credit_text()
                .map_err(|e| e.to_string())
                .map(|raw| parse_credit_info(&raw))
        })
        .await
        .map_err(|e| format!("任务错误：{e}"))?
    }

    #[tauri::command]
    pub fn rename_asset(
        store: State<'_, AppStore>,
        asset_id: String,
        new_name: String,
    ) -> Result<Asset, String> {
        store
            .mutate(|data| {
                let asset = data
                    .assets
                    .iter_mut()
                    .find(|a| a.id == asset_id)
                    .ok_or_else(|| SchedulerError::MissingAsset(asset_id.clone()))?;
                let old = asset.name.clone();
                asset.name = new_name.trim().to_string();
                let new_name_val = asset.name.clone();
                let asset_clone = asset.clone();
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Asset,
                        category: "asset".to_string(),
                        event_type: "rename".to_string(),
                        message: format!("重命名素材：{} -> {}", old, new_name_val),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(asset_clone)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn import_temp_image_command(
        store: State<'_, AppStore>,
        input: ImportAssetInput,
    ) -> Result<Asset, String> {
        let source = PathBuf::from(&input.path);
        let assets_dir = store.assets_dir().join("temp");
        store
            .mutate(|data| {
                purge_expired_temp_images(data, 10);
                let mut asset = Asset::from_path(&source, &assets_dir, input.name)
                    .map_err(|error| SchedulerError::Io(error.to_string()))?;
                asset.tags.push("temp_image".to_string());
                data.assets.push(asset.clone());
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Asset,
                        category: "asset".to_string(),
                        event_type: "import_temp_image".to_string(),
                        message: format!("导入临时图片：{}", asset.name),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(asset)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn import_asset(
        store: State<'_, AppStore>,
        input: ImportAssetInput,
    ) -> Result<Asset, String> {
        let source = PathBuf::from(input.path);
        let assets_dir = store.assets_dir();
        let asset = Asset::from_path(&source, &assets_dir, input.name)
            .map_err(|error| error.to_string())?;
        store
            .mutate(|data| {
                data.assets.push(asset.clone());
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Asset,
                        category: "asset".to_string(),
                        event_type: "import".to_string(),
                        message: format!("导入素材：{}", asset.name),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(asset.clone())
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn save_clipboard_image_command(
        store: State<'_, AppStore>,
        input: ClipboardImageInput,
    ) -> Result<Asset, String> {
        let assets_dir = store.assets_dir().join("clipboard");
        store
            .mutate(|data| {
                let asset = save_clipboard_image_asset(data, &assets_dir, input)?;
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Asset,
                        category: "asset".to_string(),
                        event_type: "paste_clipboard".to_string(),
                        message: format!("粘贴临时图片：{}", asset.name),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(asset)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn paste_clipboard_image_command(store: State<'_, AppStore>) -> Result<Asset, String> {
        let assets_dir = store.assets_dir().join("clipboard");
        store
            .mutate(|data| {
                let asset = paste_system_clipboard_image_asset(data, &assets_dir)?;
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Asset,
                        category: "asset".to_string(),
                        event_type: "paste_system_clipboard".to_string(),
                        message: format!("粘贴系统剪贴板图片：{}", asset.name),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(asset)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn import_role_media_command(
        store: State<'_, AppStore>,
        input: ImportRoleMediaInput,
    ) -> Result<Role, String> {
        let role_media_dir = store.assets_dir();
        store
            .mutate(|data| {
                let role = import_media_to_role(data, &role_media_dir, input)?;
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Role,
                        category: "role".to_string(),
                        event_type: "import_media".to_string(),
                        message: format!("导入角色媒体：{}", role.name),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(role)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn remove_role_media_command(
        store: State<'_, AppStore>,
        input: RemoveRoleMediaInput,
    ) -> Result<Role, String> {
        store
            .mutate(|data| {
                let role = remove_media_from_role(data, input)?;
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Role,
                        category: "role".to_string(),
                        event_type: "remove_media".to_string(),
                        message: format!("移除角色媒体：{}", role.name),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(role)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn create_role_command(
        store: State<'_, AppStore>,
        input: CreateRoleInput,
    ) -> Result<Role, String> {
        store
            .mutate(|data| {
                let role = upsert_role(data, input);
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Success,
                        source: LogSource::Role,
                        category: "role".to_string(),
                        event_type: "create".to_string(),
                        message: format!("新增角色：{}", role.name),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(role.clone())
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn update_role_command(
        store: State<'_, AppStore>,
        input: CreateRoleInput,
    ) -> Result<Role, String> {
        store
            .mutate(|data| {
                let role = upsert_role(data, input);
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Role,
                        category: "role".to_string(),
                        event_type: "update".to_string(),
                        message: format!("更新角色：{}", role.name),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(role)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn delete_role_command(store: State<'_, AppStore>, role_id: String) -> Result<(), String> {
        store
            .mutate(|data| {
                delete_role(data, &role_id)?;
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Role,
                        category: "role".to_string(),
                        event_type: "delete".to_string(),
                        message: format!("删除角色：{}", role_id),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(())
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn preview_task_command(
        store: State<'_, AppStore>,
        draft: TaskDraft,
    ) -> Result<Vec<String>, String> {
        store
            .mutate(|data| {
                let resolved = resolve_task_inputs(&draft, &data.assets, &data.roles)?;
                build_multimodal2video_args(&draft, &resolved)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn save_task_draft_command(
        store: State<'_, AppStore>,
        draft: TaskDraft,
    ) -> Result<ScheduledTask, String> {
        store
            .mutate(|data| {
                let task = create_draft_task(data, draft)?;
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "save_draft".to_string(),
                        message: format!("保存草稿：{}", task.title),
                        detail: String::new(),
                        task_id: Some(task.id.clone()),
                        task_title: Some(task.title.clone()),
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                data.tasks.push(task.clone());
                Ok(task)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn create_task_command(
        store: State<'_, AppStore>,
        waker: State<'_, SchedulerWaker>,
        draft: TaskDraft,
    ) -> Result<ScheduledTask, String> {
        let task = store
            .mutate(|data| {
                let task = create_task_with_preview(data, draft)?;
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Success,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "create".to_string(),
                        message: format!("创建任务：{}", task.title),
                        detail: String::new(),
                        task_id: Some(task.id.clone()),
                        task_title: Some(task.title.clone()),
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                data.tasks.push(task.clone());
                Ok(task)
            })
            .map_err(|error| error.to_string())?;
        // 新任务入队，立即唤醒可能正处于空闲长等待的调度线程。
        if has_active_tasks(std::slice::from_ref(&task)) {
            waker.notify();
        }
        Ok(task)
    }

    #[tauri::command]
    pub async fn submit_task_command(
        store: State<'_, AppStore>,
        task_id: String,
    ) -> Result<ScheduledTask, String> {
        // 锁外：从快照构建 CLI 参数
        let args = {
            let data = store.snapshot();
            let task = data
                .tasks
                .iter()
                .find(|t| t.id == task_id)
                .ok_or_else(|| format!("找不到任务：{task_id}"))?;
            let draft = TaskDraft {
                title: task.title.clone(),
                prompt: task.prompt.clone(),
                image_asset_ids: task.image_asset_ids.clone(),
                audio_asset_ids: task.audio_asset_ids.clone(),
                role_ids: task.role_ids.clone(),
                manual_mention_ids: task.manual_mention_ids.clone(),
                auto_match_roles: task.auto_match_roles,
                params: task.params.clone(),
                scheduled_at: task.scheduled_at.clone(),
                temp_image_asset_ids: task.temp_image_asset_ids.clone(),
                temp_image_paths: task.temp_image_paths.clone(),
            };
            let resolved = resolve_task_inputs(&draft, &data.assets, &data.roles)
                .map_err(|e| e.to_string())?;
            build_multimodal2video_args(&draft, &resolved).map_err(|e| e.to_string())?
        };
        // 锁外：运行 CLI
        let cli_result = tauri::async_runtime::spawn_blocking(move || run_dreamina_command(&args))
            .await
            .map_err(|e| format!("任务错误：{e}"))?;
        // 锁内：快速写回结果
        let task = store
            .mutate(|data| {
                let task =
                    submit_task_once_with_runner(data, &task_id, |_args| cli_result.clone())?;
                append_task_log(
                    data,
                    &task,
                    LogEntryDraft {
                        level: if task.status == "failed" {
                            LogLevel::Error
                        } else {
                            LogLevel::Info
                        },
                        source: LogSource::Worker,
                        category: "task".to_string(),
                        event_type: "submit".to_string(),
                        message: format!("提交任务：{} -> {}", task.title, task.status),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: if task.submit_id.is_empty() {
                            None
                        } else {
                            Some(task.submit_id.clone())
                        },
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(task)
            })
            .map_err(|e| e.to_string())?;
        // 锁外：下载结果（如有）
        if task.status == "succeeded" && !task.result_urls.is_empty() {
            let results_dir = store.assets_dir().join("results");
            let urls = task.result_urls.clone();
            let tid = task.id.clone();
            let downloaded = tauri::async_runtime::spawn_blocking(move || {
                download_result_urls(&urls, &results_dir)
            })
            .await
            .unwrap_or_default();
            if !downloaded.is_empty() {
                let _ = store.mutate(|data| {
                    if let Some(t) = data.tasks.iter_mut().find(|t| t.id == tid) {
                        for p in &downloaded {
                            if !t.result_paths.contains(p) {
                                t.result_paths.push(p.clone());
                            }
                        }
                    }
                    Ok(())
                });
                return store
                    .snapshot()
                    .tasks
                    .into_iter()
                    .find(|t| t.id == task.id)
                    .ok_or_else(|| "任务不存在".to_string());
            }
        }
        Ok(task)
    }

    #[tauri::command]
    pub async fn query_task_command(
        store: State<'_, AppStore>,
        task_id: String,
        submit_id: Option<String>,
    ) -> Result<ScheduledTask, String> {
        // 锁外：取 submit_id
        let submit_id = {
            let data = store.snapshot();
            let task = data
                .tasks
                .iter()
                .find(|t| t.id == task_id)
                .ok_or_else(|| format!("找不到任务：{task_id}"))?;
            submit_id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| task.submit_id.clone())
        };
        if submit_id.trim().is_empty() {
            return Err("任务没有 submit_id，无法查询".to_string());
        }
        // 锁外：运行 CLI
        let args = vec![
            "query_result".to_string(),
            format!("--submit_id={submit_id}"),
        ];
        let cli_result = tauri::async_runtime::spawn_blocking(move || run_dreamina_command(&args))
            .await
            .map_err(|e| format!("任务错误：{e}"))?;
        // 锁内：写回
        let task = store
            .mutate(|data| {
                // 手动查询：重置退避状态，允许重新进入自动轮询
                if let Some(t) = data.tasks.iter_mut().find(|t| t.id == task_id) {
                    if t.submit_id == submit_id {
                        reset_query_backoff(t);
                    }
                }
                let task =
                    manual_query_task_submit_id_with_runner(data, &task_id, &submit_id, |_args| {
                        cli_result.clone()
                    })?;
                append_task_log(
                    data,
                    &task,
                    LogEntryDraft {
                        level: if task.status == "failed" {
                            LogLevel::Error
                        } else {
                            LogLevel::Info
                        },
                        source: LogSource::Worker,
                        category: "task".to_string(),
                        event_type: "query".to_string(),
                        message: format!(
                            "查询任务：{} / {} -> {}",
                            task.title, submit_id, task.status
                        ),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: Some(submit_id.clone()),
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(task)
            })
            .map_err(|e| e.to_string())?;
        // 锁外：下载结果
        let result_urls = task
            .execution_records
            .iter()
            .find(|record| record.submit_id == submit_id)
            .map(|record| record.result_urls.clone())
            .unwrap_or_else(|| task.result_urls.clone());
        if !result_urls.is_empty() {
            let results_dir = store.assets_dir().join("results");
            let urls = result_urls.clone();
            let tid = task.id.clone();
            let queried_submit_id = submit_id.clone();
            let downloaded = tauri::async_runtime::spawn_blocking(move || {
                download_result_urls(&urls, &results_dir)
            })
            .await
            .unwrap_or_default();
            if !downloaded.is_empty() {
                let _ = store.mutate(|data| {
                    if let Some(t) = data.tasks.iter_mut().find(|t| t.id == tid) {
                        if t.submit_id == queried_submit_id {
                            for p in &downloaded {
                                if !t.result_paths.contains(p) {
                                    t.result_paths.push(p.clone());
                                }
                            }
                        }
                        if let Some(record) = t
                            .execution_records
                            .iter_mut()
                            .find(|record| record.submit_id == queried_submit_id)
                        {
                            for p in &downloaded {
                                if !record.result_paths.contains(p) {
                                    record.result_paths.push(p.clone());
                                }
                            }
                        }
                    }
                    Ok(())
                });
                return store
                    .snapshot()
                    .tasks
                    .into_iter()
                    .find(|t| t.id == task.id)
                    .ok_or_else(|| "任务不存在".to_string());
            }
        }
        Ok(task)
    }

    #[tauri::command]
    pub fn process_queue_command(
        store: State<'_, AppStore>,
    ) -> Result<Option<ScheduledTask>, String> {
        process_queue_for_store_blocking(&store, "manual")
    }

    #[tauri::command]
    pub fn record_lifecycle_event_command(
        store: State<'_, AppStore>,
        event_type: String,
        message: String,
        detail: String,
    ) -> Result<(), String> {
        store
            .mutate(|data| {
                record_lifecycle_event(data, &event_type, &message, &detail);
                Ok(())
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub async fn generate_task_title_command(
        store: State<'_, AppStore>,
        prompt: String,
    ) -> Result<String, String> {
        let settings = store.snapshot().settings;
        let config = settings
            .ai_model_configs
            .iter()
            .find(|config| config.id == settings.active_ai_model_id)
            .or_else(|| settings.ai_model_configs.first())
            .ok_or_else(|| "未配置 AI 模型".to_string())?
            .clone();
        let request =
            build_ai_title_request(&config, &prompt).map_err(|error| error.to_string())?;
        let url = request.url.clone();
        let api_key = config.api_key.trim().to_string();
        let body = request.body.to_string();
        let response_text =
            tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
                let response = reqwest::blocking::Client::new()
                    .post(&url)
                    .bearer_auth(&api_key)
                    .header("content-type", "application/json")
                    .body(body)
                    .send()
                    .map_err(|error| format!("AI 标题生成请求失败：{error}"))?;
                if !response.status().is_success() {
                    return Err(format!("AI 标题生成失败：HTTP {}", response.status()));
                }
                response
                    .text()
                    .map_err(|error| format!("AI 标题响应读取失败：{error}"))
            })
            .await
            .map_err(|e| format!("任务执行错误：{e}"))??;
        let payload = serde_json::from_str::<serde_json::Value>(&response_text)
            .map_err(|error| format!("AI 标题响应解析失败：{error}"))?;
        extract_generated_task_title(&payload).ok_or_else(|| "AI 标题响应中没有 title".to_string())
    }

    #[tauri::command]
    pub async fn test_ai_model_command(
        store: State<'_, AppStore>,
        api_key: String,
        base_url: String,
        model: String,
        api_mode: String,
    ) -> Result<String, String> {
        let config = AiModelConfig {
            id: "test".to_string(),
            name: "test".to_string(),
            api_key,
            base_url,
            model,
            api_mode,
        };
        let request =
            build_ai_title_request(&config, "用一句话介绍自己").map_err(|e| e.to_string())?;
        let url = request.url.clone();
        let log_url = url.clone();
        let log_mode = config.api_mode.clone();
        let log_model = config.model.clone();
        let key = config.api_key.trim().to_string();
        let body = request.body.to_string();
        let response_result =
            tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
                let response = reqwest::blocking::Client::new()
                    .post(&url)
                    .bearer_auth(&key)
                    .header("content-type", "application/json")
                    .body(body)
                    .send()
                    .map_err(|e| format!("请求失败：{e}"))?;
                if !response.status().is_success() {
                    return Err(format!(
                        "HTTP {}：{}",
                        response.status(),
                        response.text().unwrap_or_default()
                    ));
                }
                response.text().map_err(|e| format!("读取响应失败：{e}"))
            })
            .await
            .map_err(|e| format!("任务错误：{e}"))?;
        let response_text = match response_result {
            Ok(text) => text,
            Err(error) => {
                let log = format_ai_model_test_log(
                    &log_mode,
                    &log_model,
                    &log_url,
                    "",
                    None,
                    Some(&error),
                );
                let _ = store.mutate(|data| {
                    append_log(
                        data,
                        LogEntryDraft {
                            level: LogLevel::Error,
                            source: LogSource::AI,
                            category: "ai_model_test".to_string(),
                            event_type: "test_error".to_string(),
                            message: format!("AI 模型测试失败：{}", error),
                            detail: log,
                            task_id: None,
                            task_title: None,
                            submit_id: None,
                            execution_record_id: None,
                            error_detail: Some(error.clone()),
                            raw_output: None,
                            stdout: None,
                            stderr: None,
                            module: Some(format!("{}:{}", log_mode, log_model)),
                        },
                    );
                    Ok(())
                });
                return Err(error);
            }
        };
        let payload = match serde_json::from_str::<serde_json::Value>(&response_text) {
            Ok(payload) => payload,
            Err(error) => {
                let message = format!("解析响应失败：{error}");
                let log = format_ai_model_test_log(
                    &log_mode,
                    &log_model,
                    &log_url,
                    &response_text,
                    None,
                    Some(&message),
                );
                let _ = store.mutate(|data| {
                    append_log(
                        data,
                        LogEntryDraft {
                            level: LogLevel::Error,
                            source: LogSource::AI,
                            category: "ai_model_test".to_string(),
                            event_type: "parse_error".to_string(),
                            message: message.clone(),
                            detail: log,
                            task_id: None,
                            task_title: None,
                            submit_id: None,
                            execution_record_id: None,
                            error_detail: Some(message.clone()),
                            raw_output: Some(truncate_log_field(&response_text)),
                            stdout: None,
                            stderr: None,
                            module: Some(format!("{}:{}", log_mode, log_model)),
                        },
                    );
                    Ok(())
                });
                return Err(message);
            }
        };
        let parsed = extract_generated_task_title(&payload);
        let error = if parsed.is_some() {
            None
        } else {
            Some("模型已响应但未返回文本")
        };
        let log = format_ai_model_test_log(
            &log_mode,
            &log_model,
            &log_url,
            &response_text,
            parsed.as_deref(),
            error,
        );
        let _ = store.mutate(|data| {
            append_log(
                data,
                LogEntryDraft {
                    level: if error.is_some() {
                        LogLevel::Warn
                    } else {
                        LogLevel::Success
                    },
                    source: LogSource::AI,
                    category: "ai_model_test".to_string(),
                    event_type: "test_complete".to_string(),
                    message: format!(
                        "AI 模型测试完成：{}",
                        parsed.as_deref().unwrap_or("无文本输出")
                    ),
                    detail: log,
                    task_id: None,
                    task_title: None,
                    submit_id: None,
                    execution_record_id: None,
                    error_detail: error.map(|e| e.to_string()),
                    raw_output: Some(truncate_log_field(&response_text)),
                    stdout: None,
                    stderr: None,
                    module: Some(format!("{}:{}", log_mode, log_model)),
                },
            );
            Ok(())
        });
        parsed.ok_or_else(|| format!("模型已响应但未返回文本，原始响应：{response_text}"))
    }

    const IMAGEGEN_HISTORY_MAX: usize = 50;

    #[tauri::command]
    pub async fn generate_image_command(
        store: State<'_, AppStore>,
        prompt: String,
        size: String,
        reference_asset_ids: Option<Vec<String>>,
    ) -> Result<ImageGenHistoryItem, String> {
        let settings = store.snapshot().settings;
        let config = active_image_model_config(&settings)
            .cloned()
            .ok_or_else(|| "未配置图片生成模型，请先在设置中填写".to_string())?;
        if config.api_key.trim().is_empty() {
            return Err("图片模型 API Key 为空".to_string());
        }
        if config.model.trim().is_empty() {
            return Err("图片模型名称为空".to_string());
        }
        let ref_ids = reference_asset_ids.unwrap_or_default();
        println!("[generate_image] ref_ids={ref_ids:?}");
        let base_url = config.base_url.trim().trim_end_matches('/').to_string();
        let api_key = config.api_key.trim().to_string();
        let model = config.model.trim().to_string();
        let prompt_trimmed = prompt.trim().to_string();

        // 构建参考图 base64 data URI 数组
        let ref_data_uris: Vec<String> = if ref_ids.is_empty() {
            Vec::new()
        } else {
            let data = store.snapshot();
            ref_ids
                .iter()
                .map(|id| {
                    let asset = data
                        .assets
                        .iter()
                        .find(|a| &a.id == id)
                        .ok_or_else(|| format!("素材 {id} 不存在"))?;
                    println!("[generate_image]   ref id={id} path={}", asset.stored_path);
                    let bytes = std::fs::read(&asset.stored_path)
                        .map_err(|e| format!("读取参考图失败 {}：{}", asset.stored_path, e))?;
                    let _ = asset.mime.as_str();
                    // geekai.co 图生图接口接受纯 base64 字符串（不带 data: 前缀）
                    Ok::<String, String>(BASE64_STANDARD.encode(&bytes))
                })
                .collect::<Result<_, _>>()?
        };

        for (i, uri) in ref_data_uris.iter().enumerate() {
            println!("[generate_image]   ref[{}] base64_len={}", i, uri.len());
        }
        let url = format!("{base_url}/images/generations");
        println!(
            "[generate_image] mode=generations url={url} ref_count={}",
            ref_data_uris.len()
        );
        let mut body_value = serde_json::json!({
            "model": model.clone(),
            "prompt": prompt_trimmed.clone(),
            "n": 1,
            "size": size.clone(),
            "response_format": "url",
            "async": true,
        });
        if ref_data_uris.len() == 1 {
            body_value["image"] =
                serde_json::Value::String(ref_data_uris.into_iter().next().unwrap());
        } else if !ref_data_uris.is_empty() {
            // 多图必须用 images（复数）字段
            body_value["images"] = serde_json::Value::Array(
                ref_data_uris
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            );
        }
        let body = body_value.to_string();
        println!("[generate_image] request body size={} bytes", body.len());
        let ak = api_key.clone();
        let request_url = url.clone();
        let response_text: String =
            tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(IMAGE_SUBMIT_TIMEOUT_SECS))
                    .connect_timeout(std::time::Duration::from_secs(
                        IMAGE_SUBMIT_CONNECT_TIMEOUT_SECS,
                    ))
                    .build()
                    .map_err(|e| format!("创建 HTTP 客户端失败：{}", describe_error(&e)))?;
                let response = client
                    .post(&request_url)
                    .bearer_auth(&ak)
                    .header("content-type", "application/json")
                    .body(body)
                    .send()
                    .map_err(|e| format!("图片生成请求失败：{}", describe_error(&e)))?;
                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    let snippet = if body.len() > 500 {
                        format!("{}…", &body[..500])
                    } else {
                        body
                    };
                    return Err(format!("图片生成失败 HTTP {status}：{snippet}"));
                }
                response
                    .text()
                    .map_err(|e| format!("读取响应失败：{}", describe_error(&e)))
            })
            .await
            .map_err(|e| format!("任务错误：{e}"))??;
        println!(
            "[generate_image] async submit response: {}",
            truncate(&response_text, 500)
        );
        let payload = parse_imagegen_json_response(&response_text, &url)?;
        // 任务 ID：兼容多种字段名
        let task_id = extract_task_id(&payload)
            .ok_or_else(|| format!("提交成功但未找到任务 ID：{}", truncate(&response_text, 300)))?;
        println!("[generate_image] async task_id={task_id}");

        let item = ImageGenHistoryItem {
            id: format!("img_{}", Uuid::new_v4().simple()),
            prompt: prompt_trimmed.clone(),
            size: size.clone(),
            stored_path: String::new(),
            size_bytes: 0,
            mime: String::new(),
            reference_asset_ids: ref_ids.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            status: "pending".to_string(),
            task_id: Some(task_id),
            error: None,
        };
        let item_for_store = item.clone();
        store
            .mutate(|data| {
                data.imagegen_history.insert(0, item_for_store);
                if data.imagegen_history.len() > IMAGEGEN_HISTORY_MAX {
                    data.imagegen_history.truncate(IMAGEGEN_HISTORY_MAX);
                }
                Ok(())
            })
            .map_err(|e| format!("写入历史失败：{e}"))?;
        Ok(item)
    }

    /// 从异步生成响应中提取任务 ID。
    fn extract_task_id(v: &serde_json::Value) -> Option<String> {
        // 顶层 task_id / id
        for key in ["task_id", "taskId", "id"] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
        // data.task_id / data.id
        if let Some(d) = v.get("data") {
            for key in ["task_id", "taskId", "id"] {
                if let Some(s) = d.get(key).and_then(|x| x.as_str()) {
                    if !s.is_empty() {
                        return Some(s.to_string());
                    }
                }
            }
            // data 是数组，第一个元素
            if let Some(arr) = d.as_array() {
                if let Some(first) = arr.first() {
                    for key in ["task_id", "taskId", "id"] {
                        if let Some(s) = first.get(key).and_then(|x| x.as_str()) {
                            if !s.is_empty() {
                                return Some(s.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// 查询异步图片任务状态：若已完成则下载图片并更新历史项。
    /// 返回更新后的 ImageGenHistoryItem。
    #[tauri::command]
    pub async fn query_image_task_command(
        store: State<'_, AppStore>,
        history_id: String,
    ) -> Result<ImageGenHistoryItem, String> {
        // 取出当前历史项
        let snapshot = store.snapshot();
        let item = snapshot
            .imagegen_history
            .iter()
            .find(|i| i.id == history_id)
            .cloned()
            .ok_or_else(|| format!("历史记录 {history_id} 不存在"))?;
        if item.status != "pending" {
            return Ok(item);
        }
        let task_id = item
            .task_id
            .clone()
            .ok_or_else(|| "历史项缺少 task_id".to_string())?;
        let config = active_image_model_config(&snapshot.settings)
            .cloned()
            .ok_or_else(|| "未配置图片生成模型".to_string())?;
        let base_url = config.base_url.trim().trim_end_matches('/').to_string();
        let api_key = config.api_key.trim().to_string();
        let url = format!("{base_url}/images/{task_id}");
        let ak = api_key.clone();
        let request_url = url.clone();
        let response_text: String =
            tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .connect_timeout(std::time::Duration::from_secs(15))
                    .build()
                    .map_err(|e| format!("创建 HTTP 客户端失败：{}", describe_error(&e)))?;
                let response = client
                    .get(&request_url)
                    .bearer_auth(&ak)
                    .send()
                    .map_err(|e| format!("查询任务请求失败：{}", describe_error(&e)))?;
                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    let snippet = if body.len() > 500 {
                        format!("{}…", &body[..500])
                    } else {
                        body
                    };
                    return Err(format!("查询任务失败 HTTP {status}：{snippet}"));
                }
                response
                    .text()
                    .map_err(|e| format!("读取响应失败：{}", describe_error(&e)))
            })
            .await
            .map_err(|e| format!("任务错误：{e}"))??;
        println!(
            "[query_image_task] {history_id} -> {}",
            truncate(&response_text, 400)
        );
        let payload = parse_imagegen_json_response(&response_text, &url)?;

        // 解析状态（兼容 status / task_status 字段名）
        let status_raw = payload
            .get("status")
            .or_else(|| payload.get("task_status"))
            .or_else(|| payload.get("data").and_then(|d| d.get("status")))
            .or_else(|| payload.get("data").and_then(|d| d.get("task_status")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let is_completed = matches!(
            status_raw.as_str(),
            "completed" | "succeeded" | "succeed" | "success" | "finished" | "done"
        );
        let is_failed = matches!(
            status_raw.as_str(),
            "failed" | "error" | "canceled" | "cancelled"
        );

        if is_failed {
            let err_msg = payload
                .get("error")
                .or_else(|| payload.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("生成失败")
                .to_string();
            let updated = update_history_item(&store, &history_id, |it| {
                it.status = "failed".to_string();
                it.error = Some(err_msg.clone());
            })?;
            return Ok(updated);
        }

        if !is_completed {
            // 仍在 pending — 直接返回当前 item（不更新）
            return Ok(item);
        }

        // 已完成：找图片 URL 或 b64
        let first_data = payload
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .or_else(|| payload.get("data"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let bytes: Vec<u8> = if let Some(b64) = first_data.get("b64_json").and_then(|v| v.as_str())
        {
            BASE64_STANDARD
                .decode(b64.as_bytes())
                .map_err(|e| format!("base64 解码失败：{e}"))?
        } else if let Some(image_url) = first_data
            .get("url")
            .or_else(|| payload.get("url"))
            .and_then(|v| v.as_str())
        {
            let image_url = image_url.to_string();
            println!("[query_image_task] downloading image: {image_url}");
            tauri::async_runtime::spawn_blocking(move || -> Result<Vec<u8>, String> {
                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(60))
                    .build()
                    .map_err(|e| format!("创建下载客户端失败：{}", describe_error(&e)))?;
                let r = client
                    .get(&image_url)
                    .send()
                    .map_err(|e| format!("下载图片失败：{}", describe_error(&e)))?;
                if !r.status().is_success() {
                    return Err(format!("下载图片失败 HTTP {}", r.status()));
                }
                let bytes = r
                    .bytes()
                    .map(|b| b.to_vec())
                    .map_err(|e| format!("读取图片失败：{}", describe_error(&e)))?;
                println!("[query_image_task] download ok, {} bytes", bytes.len());
                Ok(bytes)
            })
            .await
            .map_err(|e| format!("任务错误：{e}"))??
        } else {
            return Err(format!(
                "响应中未找到图片数据：{}",
                truncate(&response_text, 300)
            ));
        };

        // 落盘
        let dir = store.imagegen_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("创建图片缓存目录失败：{}", describe_error(&e)))?;
        let file_name = format!("img_{}.png", Uuid::new_v4().simple());
        let stored_path = dir.join(&file_name);
        std::fs::write(&stored_path, &bytes)
            .map_err(|e| format!("写入图片文件失败：{}", describe_error(&e)))?;
        let stored_path_str = stored_path.to_string_lossy().to_string();
        let size_bytes = bytes.len() as u64;
        let updated = update_history_item(&store, &history_id, |it| {
            it.status = "completed".to_string();
            it.stored_path = stored_path_str.clone();
            it.size_bytes = size_bytes;
            it.mime = "image/png".to_string();
            it.error = None;
        })?;
        Ok(updated)
    }

    fn update_history_item<F: FnOnce(&mut ImageGenHistoryItem)>(
        store: &State<'_, AppStore>,
        id: &str,
        f: F,
    ) -> Result<ImageGenHistoryItem, String> {
        let mut updated: Option<ImageGenHistoryItem> = None;
        store
            .mutate(|data| {
                if let Some(it) = data.imagegen_history.iter_mut().find(|i| i.id == id) {
                    f(it);
                    updated = Some(it.clone());
                }
                Ok(())
            })
            .map_err(|e| format!("更新历史失败：{e}"))?;
        updated.ok_or_else(|| format!("历史记录 {id} 不存在"))
    }

    #[tauri::command]
    pub async fn retry_query_image_task_command(
        store: State<'_, AppStore>,
        history_id: String,
    ) -> Result<ImageGenHistoryItem, String> {
        let item = store
            .snapshot()
            .imagegen_history
            .iter()
            .find(|item| item.id == history_id)
            .cloned()
            .ok_or_else(|| format!("历史记录 {history_id} 不存在"))?;
        if item.task_id.is_none() {
            return Err("该图片历史记录没有异步任务 ID，无法重新查询".to_string());
        }
        let _ = update_history_item(&store, &history_id, |it| {
            it.status = "pending".to_string();
            it.error = None;
        })?;
        query_image_task_command(store, history_id).await
    }

    #[tauri::command]
    pub async fn regenerate_image_command(
        store: State<'_, AppStore>,
        history_id: String,
    ) -> Result<ImageGenHistoryItem, String> {
        let item = store
            .snapshot()
            .imagegen_history
            .iter()
            .find(|item| item.id == history_id)
            .cloned()
            .ok_or_else(|| format!("历史记录 {history_id} 不存在"))?;
        let reference_asset_ids = if item.reference_asset_ids.is_empty() {
            None
        } else {
            Some(item.reference_asset_ids)
        };
        generate_image_command(store, item.prompt, item.size, reference_asset_ids).await
    }

    #[tauri::command]
    pub fn download_imagegen_image_command(
        store: State<'_, AppStore>,
        history_id: String,
    ) -> Result<String, String> {
        let item = store
            .snapshot()
            .imagegen_history
            .iter()
            .find(|item| item.id == history_id)
            .cloned()
            .ok_or_else(|| "历史记录不存在".to_string())?;
        if item.status != "completed" || item.stored_path.trim().is_empty() {
            return Err("图片尚未生成完成，无法下载".to_string());
        }
        let source = PathBuf::from(&item.stored_path);
        if !source.exists() {
            return Err(format!("图片文件不存在：{}", item.stored_path));
        }
        let dir = store.assets_dir().join("results");
        fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败：{e}"))?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("png");
        let target = dir.join(format!("imagegen_{}.{}", item.id, extension));
        fs::copy(&source, &target).map_err(|e| format!("保存图片失败：{e}"))?;
        Ok(target.to_string_lossy().to_string())
    }

    #[tauri::command]
    pub fn copy_imagegen_image_command(
        store: State<'_, AppStore>,
        history_id: String,
    ) -> Result<(), String> {
        let item = store
            .snapshot()
            .imagegen_history
            .iter()
            .find(|item| item.id == history_id)
            .cloned()
            .ok_or_else(|| "历史记录不存在".to_string())?;
        if item.status != "completed" {
            return Err("图片尚未生成完成，无法复制".to_string());
        }
        if item.stored_path.is_empty() {
            return Err("图片本地路径为空，无法复制".to_string());
        }
        let bytes = fs::read(&item.stored_path)
            .map_err(|error| format!("读取图片失败：{}", describe_error(&error)))?;
        let (width, height, rgba) = decode_png_rgba(&bytes)
            .map_err(|error| format!("解析图片失败：{}", describe_error(&error)))?;
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("无法访问系统剪贴板：{error}"))?;
        clipboard
            .set_image(arboard::ImageData {
                width,
                height,
                bytes: Cow::Owned(rgba),
            })
            .map_err(|error| format!("写入系统剪贴板失败：{error}"))?;
        Ok(())
    }

    #[tauri::command]
    pub fn delete_imagegen_history_item_command(
        store: State<'_, AppStore>,
        id: String,
    ) -> Result<(), String> {
        store
            .mutate(|data| {
                if let Some(pos) = data.imagegen_history.iter().position(|i| i.id == id) {
                    let removed = data.imagegen_history.remove(pos);
                    if !removed.stored_path.is_empty() {
                        let _ = std::fs::remove_file(&removed.stored_path);
                    }
                }
                Ok(())
            })
            .map_err(|e| format!("删除历史失败：{e}"))
    }

    #[tauri::command]
    pub fn clear_imagegen_history_command(store: State<'_, AppStore>) -> Result<(), String> {
        store
            .mutate(|data| {
                for item in data.imagegen_history.drain(..) {
                    if !item.stored_path.is_empty() {
                        let _ = std::fs::remove_file(&item.stored_path);
                    }
                }
                Ok(())
            })
            .map_err(|e| format!("清空历史失败：{e}"))
    }

    fn describe_error<E: std::error::Error>(err: &E) -> String {
        let mut parts = vec![err.to_string()];
        let mut source = err.source();
        while let Some(e) = source {
            parts.push(e.to_string());
            source = e.source();
        }
        parts.join(" -> ")
    }

    fn truncate(s: &str, max: usize) -> String {
        if s.len() <= max {
            s.to_string()
        } else {
            format!("{}…", &s[..max])
        }
    }

    #[tauri::command]
    pub fn update_settings_command(
        store: State<'_, AppStore>,
        input: UpdateSettingsInput,
    ) -> Result<SchedulerSettings, String> {
        store
            .mutate(|data| {
                let mut settings = SchedulerSettings {
                    concurrency_limit_policy: ConcurrencyLimitPolicy::SilentRetry,
                    concurrency_retry_delay_seconds: input.concurrency_retry_delay_seconds,
                    concurrency_retry_max_attempts: input.concurrency_retry_max_attempts,
                    auto_query_enabled: input.auto_query_enabled,
                    poll_interval_seconds: input.poll_interval_seconds,
                    log_retention_count: input.log_retention_count,
                    mac_install_command: input.mac_install_command,
                    windows_install_command: input.windows_install_command,
                    ai_model_configs: input.ai_model_configs,
                    active_ai_model_id: input.active_ai_model_id,
                    prevent_sleep: input.prevent_sleep,
                    image_model_configs: input.image_model_configs,
                    active_image_model_id: input.active_image_model_id,
                    image_model_config: input.image_model_config,
                };
                normalize_image_model_settings(&mut settings);
                data.settings = settings;
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Settings,
                        category: "settings".to_string(),
                        event_type: "update".to_string(),
                        message: "更新设置".to_string(),
                        detail: format_image_model_settings_log(&data.settings),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(data.settings.clone())
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn pause_task_command(
        store: State<'_, AppStore>,
        task_id: String,
    ) -> Result<ScheduledTask, String> {
        store
            .mutate(|data| {
                let task = pause_task(data, &task_id)?;
                append_task_log(
                    data,
                    &task,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "pause".to_string(),
                        message: format!("暂停任务：{}", task.title),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(task)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn resume_task_command(
        store: State<'_, AppStore>,
        task_id: String,
        mode: String,
    ) -> Result<ScheduledTask, String> {
        store
            .mutate(|data| {
                let task = resume_task(data, &task_id, &mode)?;
                append_task_log(
                    data,
                    &task,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "resume".to_string(),
                        message: format!("恢复任务：{} -> {}", task.title, task.status),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(task)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn reschedule_task_command(
        store: State<'_, AppStore>,
        task_id: String,
        new_scheduled_at: String,
    ) -> Result<ScheduledTask, String> {
        store
            .mutate(|data| {
                let task = reschedule_task(data, &task_id, &new_scheduled_at)?;
                append_task_log(
                    data,
                    &task,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "reschedule".to_string(),
                        message: format!("重新排期：{} -> {}", task.title, new_scheduled_at),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(task)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn set_task_planned_submit_count_command(
        store: State<'_, AppStore>,
        task_id: String,
        planned_submit_count: u32,
    ) -> Result<ScheduledTask, String> {
        store
            .mutate(|data| {
                let task = set_task_planned_submit_count(data, &task_id, planned_submit_count)?;
                append_task_log(
                    data,
                    &task,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "planned_submit_count".to_string(),
                        message: format!(
                            "设置目标成功候选数：{} -> {}",
                            task.title, task.planned_submit_count
                        ),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(task)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn open_result_dir_command(path: String) -> Result<(), String> {
        let p = PathBuf::from(&path);
        if !p.exists() {
            return Err(format!("路径不存在：{path}"));
        }
        let parent = if p.is_file() {
            p.parent().unwrap_or(p.as_path()).to_path_buf()
        } else {
            p
        };
        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(&parent)
                .output()
                .map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "windows")]
        {
            Command::new("explorer")
                .arg(&parent)
                .output()
                .map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "linux")]
        {
            Command::new("xdg-open")
                .arg(&parent)
                .output()
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[tauri::command]
    pub async fn download_result_url_command(
        store: State<'_, AppStore>,
        url: String,
    ) -> Result<String, String> {
        let results_dir = store.assets_dir().join("results");
        tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
            fs::create_dir_all(&results_dir).map_err(|e| e.to_string())?;
            let ext = url
                .split('?')
                .next()
                .unwrap_or("")
                .rsplit('.')
                .next()
                .unwrap_or("mp4");
            let safe_ext = if [
                "mp4", "mov", "webm", "mkv", "png", "jpg", "jpeg", "webp", "gif",
            ]
            .contains(&ext)
            {
                ext
            } else {
                "mp4"
            };
            let id = format!("result_{}", Uuid::new_v4().simple());
            let local_path = results_dir.join(format!("{id}.{safe_ext}"));
            let response = reqwest::blocking::get(&url).map_err(|e| format!("下载失败：{e}"))?;
            if !response.status().is_success() {
                return Err(format!("下载失败：HTTP {}", response.status()));
            }
            let bytes = response.bytes().map_err(|e| format!("读取响应失败：{e}"))?;
            fs::write(&local_path, &bytes).map_err(|e| format!("写入文件失败：{e}"))?;
            Ok(local_path.to_string_lossy().to_string())
        })
        .await
        .map_err(|e| format!("任务错误：{e}"))?
    }

    #[tauri::command]
    pub async fn install_dreamina_cli_command(
        store: State<'_, AppStore>,
    ) -> Result<String, String> {
        let settings = store.snapshot().settings;
        let plan =
            build_install_plan(&settings, std::env::consts::OS).map_err(|e| e.to_string())?;
        let (msg, stdout_log, stderr_log) = tauri::async_runtime::spawn_blocking(
            move || -> Result<(String, String, String), String> {
                let output = Command::new(&plan.program)
                    .args(&plan.args)
                    .output()
                    .map_err(|e| format!("安装命令执行失败：{e}"))?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if !output.status.success() {
                    return Err(format!("安装失败：{}", truncate_log(&stderr)));
                }
                let cli = check_dreamina_cli_status();
                let msg = if cli.available {
                    format!("安装成功，CLI 路径：{}", cli.path)
                } else {
                    format!("安装命令已执行，但 CLI 仍不可用：{}", cli.message)
                };
                Ok((msg, stdout, stderr))
            },
        )
        .await
        .map_err(|e| format!("任务错误：{e}"))??;
        let _ = store.mutate(|data| {
            append_log(
                data,
                LogEntryDraft {
                    level: LogLevel::Success,
                    source: LogSource::CLI,
                    category: "cli".to_string(),
                    event_type: "install".to_string(),
                    message: format!("CLI 安装：{}", msg),
                    detail: String::new(),
                    task_id: None,
                    task_title: None,
                    submit_id: None,
                    execution_record_id: None,
                    error_detail: None,
                    raw_output: None,
                    stdout: Some(truncate_log_field(&stdout_log)),
                    stderr: Some(truncate_log_field(&stderr_log)),
                    module: None,
                },
            );
            Ok(())
        });
        Ok(msg)
    }

    #[tauri::command]
    pub async fn login_dreamina_cli_command(
        store: State<'_, AppStore>,
        headless: bool,
    ) -> Result<String, String> {
        let cli = check_dreamina_cli_status();
        if !cli.available {
            return Err(cli.message);
        }
        let plan = build_login_plan(&cli.path, headless);
        let (msg, stdout_log, stderr_log) = tauri::async_runtime::spawn_blocking(
            move || -> Result<(String, String, String), String> {
                let output = Command::new(&plan.program)
                    .args(&plan.args)
                    .output()
                    .map_err(|e| format!("登录命令执行失败：{e}"))?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if !output.status.success() {
                    return Err(format!("登录失败：{}", truncate_log(&stderr)));
                }
                let msg = match get_dreamina_credit_text() {
                    Ok(text) => format!("登录完成，额度检测：{}", truncate_log(&text)),
                    Err(error) => format!("登录命令已完成，但额度检测失败：{error}"),
                };
                Ok((msg, stdout, stderr))
            },
        )
        .await
        .map_err(|e| format!("任务错误：{e}"))??;
        let _ = store.mutate(|data| {
            append_log(
                data,
                LogEntryDraft {
                    level: LogLevel::Success,
                    source: LogSource::CLI,
                    category: "cli".to_string(),
                    event_type: "login".to_string(),
                    message: format!("CLI 登录：{}", msg),
                    detail: String::new(),
                    task_id: None,
                    task_title: None,
                    submit_id: None,
                    execution_record_id: None,
                    error_detail: None,
                    raw_output: None,
                    stdout: Some(truncate_log_field(&stdout_log)),
                    stderr: Some(truncate_log_field(&stderr_log)),
                    module: None,
                },
            );
            Ok(())
        });
        Ok(msg)
    }

    #[tauri::command]
    pub fn delete_task_command(store: State<'_, AppStore>, task_id: String) -> Result<(), String> {
        store
            .mutate(|data| {
                let before = data.tasks.len();
                data.tasks.retain(|task| task.id != task_id);
                if data.tasks.len() == before {
                    return Err(SchedulerError::Io(format!("找不到任务：{task_id}")));
                }
                append_log(
                    data,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "delete".to_string(),
                        message: format!("删除任务：{task_id}"),
                        detail: String::new(),
                        task_id: Some(task_id.clone()),
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(())
            })
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub fn delete_execution_record_command(
        store: State<'_, AppStore>,
        task_id: String,
        execution_id: String,
    ) -> Result<ScheduledTask, String> {
        store
            .mutate(|data| delete_execution_record_from_data(data, &task_id, &execution_id))
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub fn update_task_command(
        store: State<'_, AppStore>,
        waker: State<'_, SchedulerWaker>,
        task_id: String,
        draft: TaskDraft,
    ) -> Result<ScheduledTask, String> {
        let task = store
            .mutate(|data| {
                let task = update_task_from_data(data, &task_id, draft, "task")?;
                append_task_log(
                    data,
                    &task,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "update".to_string(),
                        message: format!("更新任务：{}", task.title),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(task)
            })
            .map_err(|error| error.to_string())?;
        // 若更新后任务变为活跃（如改期/重新排队），唤醒调度线程；多余唤醒会被空闲短路回收。
        if has_active_tasks(std::slice::from_ref(&task)) {
            waker.notify();
        }
        Ok(task)
    }

    #[tauri::command]
    pub fn update_task_draft_command(
        store: State<'_, AppStore>,
        task_id: String,
        draft: TaskDraft,
    ) -> Result<ScheduledTask, String> {
        store
            .mutate(|data| {
                let task = update_task_from_data(data, &task_id, draft, "draft")?;
                append_task_log(
                    data,
                    &task,
                    LogEntryDraft {
                        level: LogLevel::Info,
                        source: LogSource::Scheduler,
                        category: "task".to_string(),
                        event_type: "update_draft".to_string(),
                        message: format!("更新草稿：{}", task.title),
                        detail: String::new(),
                        task_id: None,
                        task_title: None,
                        submit_id: None,
                        execution_record_id: None,
                        error_detail: None,
                        raw_output: None,
                        stdout: None,
                        stderr: None,
                        module: None,
                    },
                );
                Ok(task)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn sync_keep_awake_command(
        store: State<'_, AppStore>,
        guard: State<'_, keep_awake::KeepAwakeGuard>,
    ) -> bool {
        let data = store.snapshot();
        if data.settings.prevent_sleep && needs_keep_awake(&data.tasks) {
            guard.acquire();
        } else {
            guard.release();
        }
        guard.is_active()
    }

    #[tauri::command]
    pub fn clear_logs_command(store: State<'_, AppStore>) -> Result<(), String> {
        store
            .mutate(|data| {
                data.logs.clear();
                Ok(())
            })
            .map_err(|error| error.to_string())
    }
}

fn collect_json_strings(value: &serde_json::Value, text: &mut String) {
    match value {
        serde_json::Value::String(value) => {
            text.push('\n');
            text.push_str(value);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_strings(item, text);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if matches!(value, serde_json::Value::String(_)) {
                    text.push('\n');
                    text.push_str(key);
                    text.push_str(": ");
                }
                collect_json_strings(value, text);
            }
        }
        _ => {}
    }
}

fn first_field(text: &str, field: &str) -> Option<String> {
    let quoted = format!("\"{field}\"");
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.contains(&quoted) {
            if let Some((_, rest)) = trimmed.split_once(':') {
                let value = rest
                    .trim()
                    .trim_matches(',')
                    .trim_matches('"')
                    .trim()
                    .to_string();
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
        if let Some(rest) = trimmed.strip_prefix(field) {
            let value = rest
                .trim_start_matches([':', '='])
                .trim()
                .trim_matches('"')
                .to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn find_json_string_field(value: &serde_json::Value, field: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(value)) = map.get(field) {
                if !value.trim().is_empty() {
                    return Some(value.trim().to_string());
                }
            }
            map.values()
                .find_map(|value| find_json_string_field(value, field))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|value| find_json_string_field(value, field)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task_with_execution_records() -> ScheduledTask {
        ScheduledTask {
            id: "task-1".to_string(),
            title: "测试任务".to_string(),
            prompt: "prompt".to_string(),
            image_asset_ids: vec![],
            audio_asset_ids: vec![],
            role_ids: vec![],
            manual_mention_ids: vec![],
            auto_match_roles: false,
            params: VideoParams::default(),
            status: "querying".to_string(),
            scheduled_at: None,
            next_run_at: None,
            submit_id: "sub-2".to_string(),
            attempt_count: 0,
            concurrency_retry_count: 0,
            last_error: String::new(),
            command_preview: vec![],
            attempts: vec![],
            result_paths: vec![],
            result_urls: vec![],
            created_at: "2026-04-30T10:00:00Z".to_string(),
            updated_at: "2026-04-30T10:00:00Z".to_string(),
            finished_at: String::new(),
            submitted_at: Some("2026-04-30T10:00:00Z".to_string()),
            queue_info: None,
            temp_image_asset_ids: vec![],
            temp_image_paths: vec![],
            execution_records: vec![
                TaskExecutionRecord {
                    id: "rec-1".to_string(),
                    submit_id: "sub-1".to_string(),
                    status: "succeeded".to_string(),
                    started_at: "2026-04-30T09:00:00Z".to_string(),
                    finished_at: "2026-04-30T09:05:00Z".to_string(),
                    input_snapshot: TaskExecutionInputSnapshot::default(),
                    command_preview: vec![],
                    query_records: vec![],
                    result_paths: vec!["/old.mp4".to_string()],
                    result_urls: vec![],
                    error_kind: String::new(),
                    error_detail: String::new(),
                },
                TaskExecutionRecord {
                    id: "rec-2".to_string(),
                    submit_id: "sub-2".to_string(),
                    status: "querying".to_string(),
                    started_at: "2026-04-30T10:00:00Z".to_string(),
                    finished_at: String::new(),
                    input_snapshot: TaskExecutionInputSnapshot::default(),
                    command_preview: vec![],
                    query_records: vec![],
                    result_paths: vec![],
                    result_urls: vec![],
                    error_kind: String::new(),
                    error_detail: String::new(),
                },
            ],
            last_auto_query_at: None,
            auto_query_stopped: false,
            consecutive_no_result_queries: 0,
            server_error_retry_count: 0,
            planned_submit_count: 1,
        }
    }

    #[test]
    fn query_specific_execution_record_uses_its_submit_id_without_overwriting_current_task_status()
    {
        let mut data = AppData {
            tasks: vec![make_task_with_execution_records()],
            ..AppData::default()
        };

        let task = query_task_submit_id_once_with_runner(&mut data, "task-1", "sub-1", |args| {
            assert_eq!(
                args,
                &["query_result".to_string(), "--submit_id=sub-1".to_string(),]
            );
            Ok((
                r#"{"gen_status":"SUCCESS","result_urls":["https://example.com/old-new.mp4"]}"#
                    .to_string(),
                String::new(),
            ))
        })
        .expect("指定执行记录查询应成功");

        assert_eq!(task.status, "querying");
        assert_eq!(task.submit_id, "sub-2");
        let old_record = task
            .execution_records
            .iter()
            .find(|record| record.submit_id == "sub-1")
            .expect("应找到第一次执行记录");
        assert_eq!(old_record.status, "succeeded");
        assert_eq!(
            old_record.result_urls,
            vec!["https://example.com/old-new.mp4"]
        );
        assert_eq!(old_record.query_records.len(), 1);

        let current_record = task
            .execution_records
            .iter()
            .find(|record| record.submit_id == "sub-2")
            .expect("应找到第二次执行记录");
        assert_eq!(current_record.status, "querying");
        assert!(current_record.query_records.is_empty());
    }

    #[test]
    fn reschedule_draft_task_prepares_it_for_delayed_generation() {
        let mut task = make_task_with_execution_records();
        task.status = "draft".to_string();
        task.submit_id.clear();
        task.execution_records.clear();
        let scheduled_at = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let mut data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };

        let updated = reschedule_task(&mut data, "task-1", &scheduled_at)
            .expect("草稿任务应允许设置定时生成");

        assert_eq!(updated.status, "scheduled");
        assert_eq!(updated.scheduled_at, Some(scheduled_at.clone()));
        assert_eq!(updated.next_run_at, Some(scheduled_at));
    }

    #[test]
    fn legacy_image_model_config_is_normalized_into_image_model_configs() {
        let mut settings: SchedulerSettings = serde_json::from_value(serde_json::json!({
            "concurrency_limit_policy": "SilentRetry",
            "concurrency_retry_delay_seconds": 300,
            "concurrency_retry_max_attempts": 8,
            "auto_query_enabled": true,
            "poll_interval_seconds": 60,
            "log_retention_count": 500,
            "mac_install_command": "curl -fsSL https://jimeng.jianying.com/cli | bash",
            "windows_install_command": "",
            "ai_model_configs": default_ai_model_configs(),
            "active_ai_model_id": default_active_ai_model_id(),
            "prevent_sleep": true,
            "image_model_config": {
                "base_url": "https://legacy.example/v1",
                "api_key": "legacy-key",
                "model": "legacy-image-model"
            }
        }))
        .expect("legacy settings should deserialize");

        normalize_image_model_settings(&mut settings);

        assert_eq!(settings.image_model_configs.len(), 1);
        assert_eq!(settings.active_image_model_id, "default-image-openai");
        assert_eq!(settings.image_model_configs[0].id, "default-image-openai");
        assert_eq!(settings.image_model_configs[0].name, "OpenAI 图片默认");
        assert_eq!(
            settings.image_model_configs[0].base_url,
            "https://legacy.example/v1"
        );
        assert_eq!(settings.image_model_configs[0].api_key, "legacy-key");
        assert_eq!(settings.image_model_configs[0].model, "legacy-image-model");
    }

    #[test]
    fn active_image_model_config_prefers_selected_config() {
        let mut settings = SchedulerSettings::default();
        settings.image_model_configs = vec![
            ImageModelConfig {
                id: "image-a".to_string(),
                name: "图片 A".to_string(),
                base_url: "https://a.example/v1".to_string(),
                api_key: "key-a".to_string(),
                model: "model-a".to_string(),
            },
            ImageModelConfig {
                id: "image-b".to_string(),
                name: "图片 B".to_string(),
                base_url: "https://b.example/v1".to_string(),
                api_key: "key-b".to_string(),
                model: "model-b".to_string(),
            },
        ];
        settings.active_image_model_id = "image-b".to_string();

        let selected = active_image_model_config(&settings).expect("selected config exists");

        assert_eq!(selected.id, "image-b");
        assert_eq!(selected.model, "model-b");
    }

    #[test]
    fn generate_image_submit_timeouts_are_five_minutes() {
        let source = std::fs::read_to_string("src/lib.rs").expect("read src/lib.rs");
        assert!(
            source.contains("const IMAGE_SUBMIT_TIMEOUT_SECS: u64 = 300;"),
            "IMAGE_SUBMIT_TIMEOUT_SECS should be 300 seconds"
        );
        assert!(
            source.contains("const IMAGE_SUBMIT_CONNECT_TIMEOUT_SECS: u64 = 300;"),
            "IMAGE_SUBMIT_CONNECT_TIMEOUT_SECS should be 300 seconds"
        );
        let start = source
            .find("pub async fn generate_image_command(")
            .expect("generate_image_command exists");
        let end = source[start..]
            .find("\"[generate_image] async submit response:")
            .map(|offset| start + offset)
            .expect("generate_image_command submit log exists");
        let function_body = &source[start..end];

        assert!(
            function_body
                .contains(".timeout(std::time::Duration::from_secs(IMAGE_SUBMIT_TIMEOUT_SECS))"),
            "generate_image_command submit timeout should use IMAGE_SUBMIT_TIMEOUT_SECS"
        );
        assert!(
            function_body.contains(".connect_timeout(std::time::Duration::from_secs(")
                && function_body.contains("IMAGE_SUBMIT_CONNECT_TIMEOUT_SECS"),
            "generate_image_command connect timeout should use IMAGE_SUBMIT_CONNECT_TIMEOUT_SECS"
        );
    }

    // ── 退避函数辅助 ─────────────────────────────────────────────────────────

    /// 创建带退避字段的测试用任务
    fn make_backoff_task(
        consecutive: u32,
        last_query: Option<&str>,
        stopped: bool,
    ) -> ScheduledTask {
        ScheduledTask {
            id: "backoff-test".to_string(),
            title: "退避测试".to_string(),
            prompt: "test prompt".to_string(),
            image_asset_ids: vec![],
            audio_asset_ids: vec![],
            role_ids: vec![],
            manual_mention_ids: vec![],
            auto_match_roles: false,
            params: VideoParams::default(),
            status: "querying".to_string(),
            scheduled_at: None,
            next_run_at: None,
            submit_id: "sub-123".to_string(),
            attempt_count: 0,
            concurrency_retry_count: 0,
            last_error: String::new(),
            command_preview: vec![],
            attempts: vec![],
            result_paths: vec![],
            result_urls: vec![],
            created_at: "2026-05-01T00:00:00Z".to_string(),
            updated_at: "2026-05-01T00:00:00Z".to_string(),
            finished_at: String::new(),
            submitted_at: Some("2026-05-01T00:00:00Z".to_string()),
            queue_info: None,
            temp_image_asset_ids: vec![],
            temp_image_paths: vec![],
            execution_records: vec![],
            last_auto_query_at: last_query.map(|s| s.to_string()),
            auto_query_stopped: stopped,
            consecutive_no_result_queries: consecutive,
            server_error_retry_count: 0,
            planned_submit_count: 1,
        }
    }

    // ── compute_wait_duration ──────────────────────────────────────────────

    fn make_task_with_status(status: &str) -> ScheduledTask {
        let mut task = make_queued_task_for_submit("wait-test");
        task.status = status.to_string();
        task
    }

    #[test]
    fn wait_duration_empty_is_idle_60s() {
        assert_eq!(compute_wait_duration(&[]), StdDuration::from_secs(60));
    }

    #[test]
    fn wait_duration_only_inactive_is_idle_60s() {
        let tasks = vec![
            make_task_with_status("succeeded"),
            make_task_with_status("failed"),
        ];
        assert_eq!(compute_wait_duration(&tasks), StdDuration::from_secs(60));
    }

    #[test]
    fn wait_duration_with_queued_is_active_30s() {
        let tasks = vec![make_task_with_status("queued")];
        assert_eq!(compute_wait_duration(&tasks), StdDuration::from_secs(30));
    }

    #[test]
    fn wait_duration_with_querying_is_active_30s() {
        let tasks = vec![make_task_with_status("querying")];
        assert_eq!(compute_wait_duration(&tasks), StdDuration::from_secs(30));
    }

    // ── SchedulerWaker ─────────────────────────────────────────────────────

    #[test]
    fn waker_notify_wakes_waiter_well_before_timeout() {
        use std::sync::Arc;
        use std::time::Instant;
        let waker = Arc::new(SchedulerWaker::new());
        let w2 = waker.clone();
        let start = Instant::now();
        let handle = std::thread::spawn(move || {
            w2.wait(StdDuration::from_secs(10));
        });
        // 让等待线程先进入 wait
        std::thread::sleep(StdDuration::from_millis(50));
        waker.notify();
        handle.join().unwrap();
        assert!(
            start.elapsed() < StdDuration::from_secs(2),
            "notify 应在远小于 10s 超时前唤醒等待线程"
        );
    }

    #[test]
    fn waker_notify_before_wait_is_not_lost() {
        use std::time::Instant;
        let waker = SchedulerWaker::new();
        waker.notify(); // 唤醒先于 wait 发生
        let start = Instant::now();
        waker.wait(StdDuration::from_secs(10)); // 必须因 pending 立即返回
        assert!(
            start.elapsed() < StdDuration::from_secs(1),
            "pending 已置位时 wait 应立即返回，不应丢失唤醒"
        );
    }

    // ── state_signature（廉价变更签名）─────────────────────────────────────

    #[test]
    fn state_signature_changes_after_mutate_and_is_stable_when_idle() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                data.tasks.push(make_queued_task_for_submit("sig-1"));
                Ok(())
            })
            .expect("mutate");
        let s1 = store.state_signature();
        // 再次写入（新增任务改变体积）→ 签名应变化
        store
            .mutate(|data| {
                data.tasks.push(make_queued_task_for_submit("sig-2"));
                Ok(())
            })
            .expect("mutate");
        let s2 = store.state_signature();
        assert_ne!(s1, s2, "写入后签名应变化");
        // 不写入时连续取签名应稳定
        assert_eq!(store.state_signature(), s2, "空闲时签名应稳定不变");
    }

    // ── cap_execution_history（历史裁剪）───────────────────────────────────

    fn cap_attempt(id: usize) -> TaskAttempt {
        TaskAttempt {
            id: format!("att-{id}"),
            started_at: String::new(),
            finished_at: String::new(),
            status: "queried".to_string(),
            command_preview: vec![],
            stdout: String::new(),
            stderr: String::new(),
            error_kind: String::new(),
            duration_seconds: 0.0,
            error_detail: String::new(),
        }
    }

    #[test]
    fn cap_execution_history_keeps_most_recent_and_reports_removed() {
        let mut task = make_queued_task_for_submit("cap-1");
        task.attempts = (0..60).map(cap_attempt).collect();
        task.execution_records = vec![TaskExecutionRecord {
            id: "rec".to_string(),
            submit_id: "s".to_string(),
            status: "querying".to_string(),
            started_at: String::new(),
            finished_at: String::new(),
            input_snapshot: TaskExecutionInputSnapshot::default(),
            command_preview: vec![],
            query_records: (0..45).map(cap_attempt).collect(),
            result_paths: vec![],
            result_urls: vec![],
            error_kind: String::new(),
            error_detail: String::new(),
        }];
        let mut data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };
        let removed = cap_execution_history(&mut data);
        // attempts 留最近 50（丢最旧 10）
        assert_eq!(data.tasks[0].attempts.len(), 50);
        assert_eq!(data.tasks[0].attempts[0].id, "att-10");
        // query_records 留最近 30（丢最旧 15）
        assert_eq!(data.tasks[0].execution_records[0].query_records.len(), 30);
        assert_eq!(
            data.tasks[0].execution_records[0].query_records[0].id,
            "att-15"
        );
        assert_eq!(removed, 10 + 15);
    }

    #[test]
    fn cap_execution_history_noop_under_cap_returns_zero() {
        let mut task = make_queued_task_for_submit("cap-2");
        task.attempts = (0..5).map(cap_attempt).collect();
        let mut data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };
        assert_eq!(cap_execution_history(&mut data), 0);
        assert_eq!(data.tasks[0].attempts.len(), 5);
    }

    #[test]
    fn load_compacts_and_trims_oversized_existing_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        // 手工写入 pretty + 超额 query_records 的 state.json（绕过 normalize 的裁剪）
        let mut task = make_queued_task_for_submit("big");
        task.execution_records = vec![TaskExecutionRecord {
            id: "rec".to_string(),
            submit_id: "s".to_string(),
            status: "querying".to_string(),
            started_at: String::new(),
            finished_at: String::new(),
            input_snapshot: TaskExecutionInputSnapshot::default(),
            command_preview: vec![],
            query_records: (0..45).map(cap_attempt).collect(),
            result_paths: vec![],
            result_urls: vec![],
            error_kind: String::new(),
            error_detail: String::new(),
        }];
        let data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };
        let path = temp.path().join("state.json");
        std::fs::write(&path, serde_json::to_string_pretty(&data).expect("ser")).expect("write");
        let big_size = std::fs::metadata(&path).expect("meta").len();

        // load 应触发一次性压实（紧凑 + 裁剪）落盘
        let _store = AppStore::load(temp.path().to_path_buf());

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(!after.contains('\n'), "应压成紧凑 JSON");
        assert!(
            (after.len() as u64) < big_size,
            "文件应明显变小：{} -> {}",
            big_size,
            after.len()
        );
        // 回读确认 query_records 已裁剪到 30
        let reloaded = AppStore::load(temp.path().to_path_buf()).snapshot();
        assert_eq!(reloaded.tasks[0].execution_records[0].query_records.len(), 30);
    }

    // ── persist 紧凑序列化 ─────────────────────────────────────────────────

    #[test]
    fn persist_writes_compact_json_not_pretty() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                data.tasks.push(make_queued_task_for_submit("compact-1"));
                Ok(())
            })
            .expect("mutate");
        let content =
            std::fs::read_to_string(temp.path().join("state.json")).expect("read state.json");
        assert!(
            !content.contains('\n'),
            "state.json 应为紧凑 JSON（无换行/缩进），实际包含换行"
        );
        // 紧凑写入后仍能正确回读
        let reloaded = AppStore::load(temp.path().to_path_buf()).snapshot();
        assert_eq!(reloaded.tasks.len(), 1);
        assert_eq!(reloaded.tasks[0].id, "compact-1");
    }

    // ── should_process_now（空闲短路）─────────────────────────────────────

    #[test]
    fn should_process_now_false_when_fully_idle() {
        let data = AppData::default();
        assert!(!should_process_now(&data));
    }

    #[test]
    fn should_process_now_false_with_only_finished_tasks() {
        let data = AppData {
            tasks: vec![make_task_with_status("succeeded")],
            ..AppData::default()
        };
        assert!(!should_process_now(&data));
    }

    #[test]
    fn should_process_now_true_with_active_task() {
        let data = AppData {
            tasks: vec![make_task_with_status("queued")],
            ..AppData::default()
        };
        assert!(should_process_now(&data));
    }

    // ── backoff_interval_secs ──────────────────────────────────────────────

    #[test]
    fn backoff_interval_0_returns_0() {
        assert_eq!(backoff_interval_secs(0), 0);
    }

    #[test]
    fn backoff_interval_1_returns_60() {
        assert_eq!(backoff_interval_secs(1), 60);
    }

    #[test]
    fn backoff_interval_2_returns_120() {
        assert_eq!(backoff_interval_secs(2), 120);
    }

    #[test]
    fn backoff_interval_3_returns_300() {
        assert_eq!(backoff_interval_secs(3), 300);
    }

    #[test]
    fn backoff_interval_4_returns_600() {
        assert_eq!(backoff_interval_secs(4), 600);
    }

    #[test]
    fn backoff_interval_5_or_more_returns_600() {
        assert_eq!(backoff_interval_secs(5), 600);
        assert_eq!(backoff_interval_secs(100), 600);
    }

    // ── is_backoff_due ─────────────────────────────────────────────────────

    #[test]
    fn is_backoff_due_consecutive_zero_returns_true() {
        let task = make_backoff_task(0, None, false);
        assert!(is_backoff_due(&task, Utc::now()));
    }

    #[test]
    fn is_backoff_due_no_last_auto_query_at_returns_true() {
        let task = make_backoff_task(1, None, false);
        assert!(is_backoff_due(&task, Utc::now()));
    }

    #[test]
    fn is_backoff_due_interval_elapsed_returns_true() {
        let past = (Utc::now() - Duration::minutes(5)).to_rfc3339();
        let task = make_backoff_task(2, Some(&past), false); // 2→120s
        assert!(is_backoff_due(&task, Utc::now()));
    }

    #[test]
    fn is_backoff_due_interval_not_elapsed_returns_false() {
        let recent = Utc::now().to_rfc3339();
        let task = make_backoff_task(2, Some(&recent), false); // 2→120s, just now
        assert!(!is_backoff_due(&task, Utc::now()));
    }

    #[test]
    fn is_backoff_due_with_malformed_timestamp_returns_true() {
        let task = make_backoff_task(1, Some("not-a-timestamp"), false);
        assert!(is_backoff_due(&task, Utc::now()));
    }

    // ── is_past_max_wait ───────────────────────────────────────────────────

    fn make_past_max_task(submitted_at: Option<&str>) -> ScheduledTask {
        let mut task = make_backoff_task(0, None, false);
        task.submitted_at = submitted_at.map(|s| s.to_string());
        task
    }

    #[test]
    fn is_past_max_wait_none_submitted_at_returns_false() {
        assert!(!is_past_max_wait(&make_past_max_task(None), Utc::now()));
    }

    #[test]
    fn is_past_max_wait_empty_submitted_at_returns_false() {
        assert!(!is_past_max_wait(&make_past_max_task(Some("")), Utc::now()));
    }

    #[test]
    fn is_past_max_wait_under_4_hours_returns_false() {
        let recent = (Utc::now() - Duration::hours(2)).to_rfc3339();
        assert!(!is_past_max_wait(
            &make_past_max_task(Some(&recent)),
            Utc::now()
        ));
    }

    #[test]
    fn is_past_max_wait_over_4_hours_returns_true() {
        let past = (Utc::now() - Duration::hours(5)).to_rfc3339();
        assert!(is_past_max_wait(
            &make_past_max_task(Some(&past)),
            Utc::now()
        ));
    }

    #[test]
    fn is_past_max_wait_exactly_4_hours_returns_true() {
        let past = (Utc::now() - Duration::hours(4)).to_rfc3339();
        assert!(is_past_max_wait(
            &make_past_max_task(Some(&past)),
            Utc::now()
        ));
    }

    #[test]
    fn is_past_max_wait_malformed_submitted_at_returns_false() {
        assert!(!is_past_max_wait(
            &make_past_max_task(Some("invalid-date")),
            Utc::now()
        ));
    }

    // ── manual query bypasses 4-hour cap ───────────────────────────────────

    fn make_long_pending_task() -> ScheduledTask {
        let submitted = (Utc::now() - Duration::hours(6)).to_rfc3339();
        let mut task = make_backoff_task(0, None, true);
        task.id = "task-stale".to_string();
        task.title = "Stale".to_string();
        task.submit_id = "sub-stale".to_string();
        task.status = "submitted".to_string();
        task.submitted_at = Some(submitted);
        task.last_error = "自动查询已停止".to_string();
        task
    }

    #[test]
    fn auto_query_past_4_hours_keeps_task_stopped() {
        let mut data = AppData {
            tasks: vec![make_long_pending_task()],
            ..AppData::default()
        };
        // 有远端任务状态但已超过 4 小时：停止自动查询，改为手动查询
        let task =
            query_task_submit_id_once_with_runner(&mut data, "task-stale", "sub-stale", |_args| {
                Ok((
                    String::from(r#"{"created":1777777777,"task_status":"running","data":[]}"#),
                    String::new(),
                ))
            })
            .expect("auto query should succeed");
        assert_eq!(task.status, "submitted");
        assert!(task.auto_query_stopped);
        assert!(task.last_error.contains("已等待超过"));
    }

    #[test]
    fn manual_query_past_4_hours_bypasses_cap() {
        let mut data = AppData {
            tasks: vec![make_long_pending_task()],
            ..AppData::default()
        };
        // 模拟手动查询前的 reset
        if let Some(t) = data.tasks.iter_mut().find(|t| t.id == "task-stale") {
            reset_query_backoff(t);
        }
        // No-result CLI output: should NOT re-trigger 4 小时上限
        let task = manual_query_task_submit_id_with_runner(
            &mut data,
            "task-stale",
            "sub-stale",
            |_args| Ok((String::from("{}"), String::new())),
        )
        .expect("manual query should succeed");
        assert_eq!(task.status, "querying");
        assert!(!task.auto_query_stopped);
        assert!(
            !task.last_error.contains("已等待超过"),
            "manual query should not surface the 4-hour stop message: {}",
            task.last_error
        );
    }

    // ── 5xx submit retry ──────────────────────────────────────────────────

    fn make_test_image_asset() -> Asset {
        Asset {
            id: "img-test-01".to_string(),
            kind: AssetKind::Image,
            name: "test.png".to_string(),
            aliases: vec![],
            tags: vec![],
            stored_path: "/tmp/test.png".to_string(),
            source_path: "/tmp/test.png".to_string(),
            mime: "image/png".to_string(),
            size_bytes: 1024,
            duration_seconds: None,
            created_at: "2026-05-01T00:00:00Z".to_string(),
            content_hash: None,
        }
    }

    fn make_queued_task_for_submit(id: &str) -> ScheduledTask {
        ScheduledTask {
            id: id.to_string(),
            title: "重试测试".to_string(),
            prompt: "test".to_string(),
            image_asset_ids: vec!["img-test-01".to_string()],
            audio_asset_ids: vec![],
            role_ids: vec![],
            manual_mention_ids: vec![],
            auto_match_roles: false,
            params: VideoParams::default(),
            status: "queued".to_string(),
            scheduled_at: None,
            next_run_at: None,
            submit_id: String::new(),
            attempt_count: 0,
            concurrency_retry_count: 0,
            server_error_retry_count: 0,
            planned_submit_count: 1,
            last_error: String::new(),
            command_preview: vec![],
            attempts: vec![],
            result_paths: vec![],
            result_urls: vec![],
            created_at: "2026-05-01T00:00:00Z".to_string(),
            updated_at: "2026-05-01T00:00:00Z".to_string(),
            finished_at: String::new(),
            submitted_at: None,
            queue_info: None,
            temp_image_asset_ids: vec![],
            temp_image_paths: vec![],
            execution_records: vec![],
            last_auto_query_at: None,
            auto_query_stopped: false,
            consecutive_no_result_queries: 0,
        }
    }

    #[test]
    fn submit_5xx_first_time_goes_to_retry_wait() {
        let mut data = AppData {
            tasks: vec![make_queued_task_for_submit("t5xx")],
            assets: vec![make_test_image_asset()],
            ..AppData::default()
        };
        let task = submit_task_once_with_runner(&mut data, "t5xx", |_args| {
            Ok((
                r#"{"code": 50001, "message": "服务器内部错误"}"#.to_string(),
                String::new(),
            ))
        })
        .expect("should not Err");
        assert_eq!(task.status, "retry_wait", "第 1 次 5xx 应进入 retry_wait");
        assert_eq!(task.server_error_retry_count, 1);
    }

    #[test]
    fn submit_5xx_second_time_still_retry_wait() {
        let mut data = AppData {
            tasks: vec![make_queued_task_for_submit("t5xx2")],
            assets: vec![make_test_image_asset()],
            ..AppData::default()
        };
        // 模拟已经历 1 轮并发等待
        data.tasks[0].server_error_retry_count = 1;
        data.tasks[0].status = "queued".to_string();
        let task = submit_task_once_with_runner(&mut data, "t5xx2", |_args| {
            Ok((
                r#"{"code": 50001, "message": "服务器内部错误"}"#.to_string(),
                String::new(),
            ))
        })
        .expect("should not Err");
        assert_eq!(task.status, "retry_wait", "第 2 次 5xx 仍应 retry_wait");
        assert_eq!(task.server_error_retry_count, 2);
    }

    #[test]
    fn submit_5xx_third_time_marks_failed() {
        let mut data = AppData {
            tasks: vec![make_queued_task_for_submit("t5xx3")],
            assets: vec![make_test_image_asset()],
            ..AppData::default()
        };
        // 模拟已经历 2 轮并发等待
        data.tasks[0].server_error_retry_count = 2;
        data.tasks[0].status = "queued".to_string();
        let task = submit_task_once_with_runner(&mut data, "t5xx3", |_args| {
            Ok((
                r#"{"code": 50001, "message": "服务器内部错误"}"#.to_string(),
                String::new(),
            ))
        })
        .expect("should not Err");
        assert_eq!(task.status, "failed", "第 3 次 5xx 超过上限应标 failed");
        assert_eq!(task.server_error_retry_count, 3);
    }

    #[test]
    fn submit_success_resets_server_error_retry_count() {
        let mut data = AppData {
            tasks: vec![make_queued_task_for_submit("t5xx-ok")],
            assets: vec![make_test_image_asset()],
            ..AppData::default()
        };
        data.tasks[0].server_error_retry_count = 1;
        data.tasks[0].status = "queued".to_string();
        let task = submit_task_once_with_runner(&mut data, "t5xx-ok", |_args| {
            Ok((r#"{"submit_id": "ok-id-123"}"#.to_string(), String::new()))
        })
        .expect("should not Err");
        assert_eq!(task.server_error_retry_count, 0, "成功提交后计数应重置为 0");
        assert_eq!(task.submit_id, "ok-id-123");
    }

    #[test]
    fn submit_with_submit_id_and_exceed_concurrency_limit_enters_retry_wait() {
        let mut data = AppData {
            tasks: vec![make_queued_task_for_submit("t-concurrency-submit-id")],
            assets: vec![make_test_image_asset()],
            ..AppData::default()
        };
        let task = submit_task_once_with_runner(
            &mut data,
            "t-concurrency-submit-id",
            |_args| Ok((
                r#"{"submit_id":"fake-sub-123","gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit"}"#.to_string(),
                String::new(),
            )),
        )
        .expect("should not Err");
        assert_eq!(
            task.status, "retry_wait",
            "ExceedConcurrencyLimit should wait for retry even if submit_id is present"
        );
        assert_eq!(
            task.submit_id, "",
            "failed submit_id should not become current query target"
        );
        assert_eq!(
            task.concurrency_retry_count, 1,
            "submit-stage ExceedConcurrencyLimit should schedule retry"
        );
        assert!(task.next_run_at.is_some());
        assert!(
            task.last_error.contains("ExceedConcurrencyLimit"),
            "failure reason should be preserved: {}",
            task.last_error
        );
        assert_eq!(task.execution_records.len(), 1);
        assert_eq!(task.execution_records[0].submit_id, "fake-sub-123");
        assert_eq!(task.execution_records[0].status, "retry_wait");
        assert!(
            task.execution_records[0].query_records.is_empty(),
            "should not enter query loop"
        );
    }

    // ── parse_submit_output: error_code suppresses submit_id ──────────────

    #[test]
    fn parse_submit_output_error_code_suppresses_submit_id() {
        // API 返回错误码时，即使响应里含 submit_id 也应被忽略
        let out = parse_submit_output(
            r#"{"code": 50001, "message": "服务器内部错误", "data": {"submit_id": "fake-id-abc"}}"#,
        );
        assert_eq!(
            out.submit_id, None,
            "submit_id in error response must be suppressed"
        );
        assert_eq!(out.error_code, Some(50001));
        assert!(out.fail_reason.is_some());
    }

    #[test]
    fn parse_submit_output_success_extracts_submit_id_normally() {
        let out = parse_submit_output(r#"{"submit_id": "real-sub-123"}"#);
        assert_eq!(out.submit_id.as_deref(), Some("real-sub-123"));
        assert_eq!(out.error_code, None);
    }

    #[test]
    fn parse_submit_output_code_200_does_not_suppress_submit_id() {
        let out = parse_submit_output(r#"{"code": 200, "submit_id": "ok-sub-456"}"#);
        assert_eq!(out.submit_id.as_deref(), Some("ok-sub-456"));
        assert_eq!(out.error_code, None);
    }

    #[test]
    fn asset_from_path_normalizes_spaces_in_name_for_mentions() {
        let unique = format!("dreamina-space-name-{}", Uuid::new_v4().simple());
        let temp_root = std::env::temp_dir().join(unique);
        let source_dir = temp_root.join("source files");
        let assets_dir = temp_root.join("role media");
        fs::create_dir_all(&source_dir).expect("create source dir");
        let source_path = source_dir.join("hero main shot.png");
        fs::write(&source_path, b"fake-png").expect("write source image");

        let asset =
            Asset::from_path(&source_path, &assets_dir, None).expect("asset import should succeed");

        assert_eq!(asset.name, "hero_main_shot");

        let _ = fs::remove_dir_all(&temp_root);
    }

    // ── error_code extraction and failed classification ────────────────────

    #[test]
    fn parse_query_output_extracts_error_code() {
        let out = parse_query_output(r#"{"code": 40004, "message": "任务不存在或已过期"}"#);
        assert_eq!(out.error_code, Some(40004));
        assert_eq!(out.fail_reason.as_deref(), Some("任务不存在或已过期"));
        assert!(out.gen_status.is_none());
    }

    #[test]
    fn parse_query_output_filters_code_zero_and_200() {
        let out0 = parse_query_output(r#"{"code": 0, "gen_status": "processing"}"#);
        assert_eq!(out0.error_code, None);
        let out200 = parse_query_output(r#"{"code": 200, "gen_status": "processing"}"#);
        assert_eq!(out200.error_code, None);
    }

    #[test]
    fn query_with_api_error_code_marks_task_failed_immediately() {
        let mut task = make_task_with_execution_records();
        task.id = "task-err".to_string();
        task.submit_id = "sub-err".to_string();
        task.status = "querying".to_string();
        task.submitted_at = Some((Utc::now() - Duration::minutes(5)).to_rfc3339());
        task.execution_records.push(TaskExecutionRecord {
            id: "rec-err".to_string(),
            submit_id: "sub-err".to_string(),
            status: "querying".to_string(),
            started_at: now_rfc3339(),
            finished_at: String::new(),
            input_snapshot: TaskExecutionInputSnapshot::default(),
            command_preview: vec![],
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: String::new(),
            error_detail: String::new(),
        });
        let mut data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };
        // Dreamina returns error code 40004 (task not found)
        let result =
            query_task_submit_id_once_with_runner(&mut data, "task-err", "sub-err", |_args| {
                Ok((
                    r#"{"code": 40004, "message": "任务不存在或已过期"}"#.to_string(),
                    String::new(),
                ))
            })
            .expect("query should complete without Rust error");
        assert_eq!(
            result.status, "failed",
            "error code >= 400 should mark task as failed"
        );
        assert!(
            result.last_error.contains("任务不存在"),
            "error message should be stored in last_error: {}",
            result.last_error
        );
    }

    // ── update_query_backoff ───────────────────────────────────────────────

    #[test]
    fn update_query_backoff_increments_count_and_sets_timestamp() {
        let mut task = make_backoff_task(2, None, false);
        let before = Utc::now();
        update_query_backoff(&mut task);
        assert_eq!(task.consecutive_no_result_queries, 3);
        let ts = task.last_auto_query_at.expect("应设置时间戳");
        let parsed = DateTime::parse_from_rfc3339(&ts).expect("时间戳应合法");
        assert!(parsed.with_timezone(&Utc) >= before);
    }

    // ── reset_query_backoff ────────────────────────────────────────────────

    #[test]
    fn reset_query_backoff_resets_all_fields() {
        let mut task = make_backoff_task(5, Some("2026-05-01T10:00:00Z"), true);
        reset_query_backoff(&mut task);
        assert_eq!(task.consecutive_no_result_queries, 0);
        assert_eq!(task.last_auto_query_at, None);
        assert!(!task.auto_query_stopped);
    }

    // ── recover_tasks_on_load ──────────────────────────────────────────────

    fn make_migration_task(status: &str, last_error: &str) -> ScheduledTask {
        ScheduledTask {
            id: "migrate-task".to_string(),
            title: "迁移测试".to_string(),
            prompt: "test".to_string(),
            image_asset_ids: vec![],
            audio_asset_ids: vec![],
            role_ids: vec![],
            manual_mention_ids: vec![],
            auto_match_roles: false,
            params: VideoParams::default(),
            status: status.to_string(),
            scheduled_at: None,
            next_run_at: None,
            submit_id: "sub-456".to_string(),
            attempt_count: 0,
            concurrency_retry_count: 0,
            last_error: last_error.to_string(),
            command_preview: vec![],
            attempts: vec![],
            result_paths: vec![],
            result_urls: vec![],
            created_at: "2026-05-01T00:00:00Z".to_string(),
            updated_at: "2026-05-01T00:00:00Z".to_string(),
            finished_at: String::new(),
            submitted_at: Some("2026-05-01T00:00:00Z".to_string()),
            queue_info: None,
            temp_image_asset_ids: vec![],
            temp_image_paths: vec![],
            execution_records: vec![],
            last_auto_query_at: None,
            auto_query_stopped: false,
            consecutive_no_result_queries: 0,
            server_error_retry_count: 0,
            planned_submit_count: 1,
        }
    }

    #[test]
    fn recover_tasks_on_load_converts_query_timeout_to_submitted_with_auto_query_stopped() {
        let mut data = AppData {
            tasks: vec![make_migration_task("query_timeout", "")],
            ..AppData::default()
        };
        recover_tasks_on_load(&mut data);
        let task = &data.tasks[0];
        assert_eq!(task.status, "submitted");
        assert!(task.auto_query_stopped);
        assert!(task.last_error.contains("自动查询已超时"));
    }

    #[test]
    fn recover_tasks_on_load_preserves_existing_last_error_for_query_timeout() {
        let mut data = AppData {
            tasks: vec![make_migration_task("query_timeout", "自定义错误")],
            ..AppData::default()
        };
        recover_tasks_on_load(&mut data);
        let task = &data.tasks[0];
        assert_eq!(task.status, "submitted");
        assert!(task.auto_query_stopped);
        // 自定义错误不包含"查询超时"，应保留原值
        assert_eq!(task.last_error, "自定义错误");
    }

    #[test]
    fn recover_tasks_on_load_does_not_affect_normal_status_tasks() {
        let mut data = AppData {
            tasks: vec![ScheduledTask {
                id: "normal-task".to_string(),
                title: "正常任务".to_string(),
                prompt: "test".to_string(),
                image_asset_ids: vec![],
                audio_asset_ids: vec![],
                role_ids: vec![],
                manual_mention_ids: vec![],
                auto_match_roles: false,
                params: VideoParams::default(),
                status: "succeeded".to_string(),
                scheduled_at: None,
                next_run_at: None,
                submit_id: String::new(),
                attempt_count: 0,
                concurrency_retry_count: 0,
                last_error: String::new(),
                command_preview: vec![],
                attempts: vec![],
                result_paths: vec!["/result.mp4".to_string()],
                result_urls: vec![],
                created_at: "2026-05-01T00:00:00Z".to_string(),
                updated_at: "2026-05-01T00:00:00Z".to_string(),
                finished_at: "2026-05-01T01:00:00Z".to_string(),
                submitted_at: Some("2026-05-01T00:00:00Z".to_string()),
                queue_info: None,
                temp_image_asset_ids: vec![],
                temp_image_paths: vec![],
                execution_records: vec![],
                last_auto_query_at: None,
                auto_query_stopped: false,
                consecutive_no_result_queries: 0,
                server_error_retry_count: 0,
                planned_submit_count: 1,
            }],
            ..AppData::default()
        };
        recover_tasks_on_load(&mut data);
        let task = &data.tasks[0];
        assert_eq!(task.status, "succeeded");
        assert!(!task.auto_query_stopped);
    }
}
