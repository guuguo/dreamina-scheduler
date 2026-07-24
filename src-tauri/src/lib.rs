mod keep_awake;
mod sqlite_store;
use chrono::{DateTime, Duration, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use sha2::{Digest, Sha256};
pub use sqlite_store::AppStore;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration as StdDuration, UNIX_EPOCH},
};
use tauri::Manager;
use thiserror::Error;
use uuid::Uuid;

const SUPPORTED_RATIOS: &[&str] = &["1:1", "3:4", "16:9", "4:3", "9:16", "21:9"];
const SUPPORTED_MODELS: &[&str] = &["seedance2.0", "seedance2.0fast"];
const MAX_IMAGES: usize = 9;
const MAX_AUDIO: usize = 3;
/// 自动查询最长等待时间，超过后停止自动查询，改为手动。
const MAX_WAIT_HOURS: i64 = 6;
const MAX_NO_REMOTE_QUEUE_INFO_MINUTES: i64 = 10;
/// 5xx 服务器错误自动重试上限次数
const MAX_SERVER_ERROR_RETRIES: u32 = 2;
/// 上传 EOF / connection reset 等提交阶段瞬时错误，最多自动重试次数。
const MAX_TRANSIENT_SUBMIT_RETRIES: u32 = 3;
/// 队列空闲后，因提交瞬时错误失败的任务额外补试次数。
const MAX_IDLE_TRANSIENT_FAILED_RETRIES: u32 = 3;
/// 队列空闲后，因并发限制被旧策略标失败的任务额外补试次数。
const MAX_IDLE_CONCURRENCY_FAILED_RETRIES: u32 = 3;
/// 生成阶段 pre-TNS 检查失败后，自动重新提交的次数。
const MAX_GENERATION_PRECHECK_RETRIES: u32 = 2;
/// 只自动捞起近期并发限制失败，避免把几周前的历史失败任务重新提交。
const MAX_CONCURRENCY_FAILURE_RECOVERY_HOURS: i64 = 24;
const IMAGE_SUBMIT_TIMEOUT_SECS: u64 = 300;
const IMAGE_SUBMIT_CONNECT_TIMEOUT_SECS: u64 = 300;
const DREAMINA_CLI_TIMEOUT_SECS: u64 = 360;
const SCHEDULER_TICK_INTERVAL_SECS: u64 = 30;
const DEFAULT_CONCURRENCY_RETRY_DELAY_SECONDS: u64 = 300;
const LEGACY_CONCURRENCY_RETRY_DELAY_SECONDS: u64 = 30;
const FAST_FALLBACK_MODEL_VERSION: &str = "seedance2.0fast";
static PROCESS_QUEUE_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ModelQueueKind {
    Standard,
    Fast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubmitQueueSelection {
    task_id: String,
    target_queue_kind: ModelQueueKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneStatus {
    pub queue_kind: String,
    pub model_version: String,
    pub enabled: bool,
    pub is_active: bool,
    pub is_cooling_down: bool,
    pub cooldown_reason: String,
    pub current_task_id: String,
    pub current_task_title: String,
    pub submit_id: String,
    pub queue_position: Option<u64>,
    pub queue_length: Option<u64>,
    pub next_check_at: String,
    pub waiting_task_count: u32,
}

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
    #[serde(default)]
    pub last_used_at: Option<String>,
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
            last_used_at: None,
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
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "m4a" => "audio/mp4",
            "aac" => "audio/aac",
            _ => "image/png",
        }
        .to_string()
    } else {
        input.mime
    };
    let is_audio = mime.starts_with("audio/");
    let asset = Asset {
        id,
        kind: if is_audio {
            AssetKind::Audio
        } else {
            AssetKind::Image
        },
        name: if is_audio {
            "粘贴音频".to_string()
        } else {
            "粘贴图片".to_string()
        },
        aliases: vec![],
        tags: if is_audio {
            vec![
                "clipboard".to_string(),
                "temporary".to_string(),
                "temp_audio".to_string(),
            ]
        } else {
            vec![
                "clipboard".to_string(),
                "temporary".to_string(),
                "temp_image".to_string(),
            ]
        },
        stored_path: stored_path.to_string_lossy().to_string(),
        source_path: "clipboard".to_string(),
        mime,
        size_bytes: metadata.len(),
        duration_seconds: None,
        created_at: now_rfc3339(),
        content_hash: Some(content_hash.clone()),
        last_used_at: None,
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
    #[serde(default)]
    pub series: String,
    pub description: String,
    #[serde(default)]
    pub disabled: bool,
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
    #[serde(default)]
    pub prompt_doc: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchQueuePlanItem {
    pub task_id: String,
    #[serde(default)]
    pub scheduled_at: Option<String>,
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
    #[serde(default)]
    pub queued_at: Option<String>,
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
    #[serde(default)]
    pub prompt_doc: Option<serde_json::Value>,
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
            queued_at: Some(now.clone()),
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
            prompt_doc: value.prompt_doc,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[serde(default, rename = "taskPriorities", alias = "task_priorities")]
    pub task_priorities: HashMap<String, u8>,
    #[serde(default, deserialize_with = "deserialize_logs_compat")]
    pub logs: Vec<LogEntry>,
    #[serde(default)]
    pub imagegen_history: Vec<ImageGenHistoryItem>,
    #[serde(skip, default = "HashMap::new")]
    pub asset_hash_index: HashMap<String, String>,
    #[serde(default, rename = "laneStatus", alias = "lane_status")]
    pub lane_status: Vec<LaneStatus>,
}

impl Default for AppData {
    fn default() -> Self {
        Self {
            settings: SchedulerSettings::default(),
            assets: vec![],
            roles: vec![],
            tasks: vec![],
            task_priorities: HashMap::new(),
            logs: vec![],
            imagegen_history: vec![],
            asset_hash_index: HashMap::new(),
            lane_status: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[serde(default, alias = "alternateFastModel")]
    pub alternate_fast_model: Option<bool>,
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

/// MCP 原位更新请求。仅允许替换尚未执行任务的生成输入，避免用“重新入队”制造重复任务。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpQueuedTaskUpdateInput {
    #[serde(alias = "taskId")]
    pub task_id: String,
    #[serde(flatten)]
    pub task: McpVideoTaskInput,
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
    #[serde(default, alias = "alternateFastModel")]
    pub alternate_fast_model: bool,
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

pub fn parse_mcp_queued_task_update_input(
    value: JsonValue,
) -> Result<McpQueuedTaskUpdateInput, SchedulerError> {
    let JsonValue::Object(mut map) = value else {
        return Err(SchedulerError::Io("MCP 原位更新参数必须是对象".to_string()));
    };
    let task_id = ["task_id", "taskId"]
        .iter()
        .find_map(|key| map.remove(*key))
        .and_then(|value| value.as_str().map(str::to_string))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| SchedulerError::Io("task_id 不可为空".to_string()))?;
    Ok(McpQueuedTaskUpdateInput {
        task_id,
        task: parse_mcp_video_task_input(JsonValue::Object(map))?,
    })
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
    #[serde(default)]
    pub series: String,
    pub description: String,
    #[serde(default)]
    pub disabled: bool,
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
    #[serde(default)]
    pub log_retention_days: Option<u32>,
    pub mac_install_command: String,
    pub windows_install_command: String,
    #[serde(default = "default_ai_model_configs")]
    pub ai_model_configs: Vec<AiModelConfig>,
    #[serde(default = "default_active_ai_model_id")]
    pub active_ai_model_id: String,
    #[serde(default = "default_true")]
    pub prevent_sleep: bool,
    #[serde(default = "default_true")]
    pub standard_lane_enabled: bool,
    #[serde(default = "default_true")]
    pub fast_lane_enabled: bool,
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

fn load_app_data_from_disk(root_dir: &Path) -> Result<AppData, SchedulerError> {
    let data_path = root_dir.join("state.json");
    let content = match fs::read_to_string(&data_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut data = AppData::default();
            normalize_loaded_app_data(&mut data);
            return Ok(data);
        }
        Err(error) => return Err(SchedulerError::Io(error.to_string())),
    };
    let mut data: AppData = match serde_json::from_str(&content) {
        Ok(data) => data,
        Err(error) => {
            let value = serde_json::from_str::<JsonValue>(&content)
                .map_err(|parse_error| SchedulerError::Io(parse_error.to_string()))?;
            load_app_data_resilient(value, &error.to_string())
        }
    };
    normalize_loaded_app_data(&mut data);
    Ok(data)
}

fn parse_json_field<T>(object: &mut JsonMap<String, JsonValue>, key: &str) -> T
where
    T: DeserializeOwned + Default,
{
    object
        .remove(key)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn parse_logs_field(object: &mut JsonMap<String, JsonValue>) -> Vec<LogEntry> {
    #[derive(Deserialize, Default)]
    struct LogsOnly {
        #[serde(default, deserialize_with = "deserialize_logs_compat")]
        logs: Vec<LogEntry>,
    }

    object
        .remove("logs")
        .and_then(|logs| {
            serde_json::from_value::<LogsOnly>(serde_json::json!({ "logs": logs })).ok()
        })
        .map(|parsed| parsed.logs)
        .unwrap_or_default()
}

fn load_app_data_resilient(value: JsonValue, root_error: &str) -> AppData {
    let Some(mut object) = value.as_object().cloned() else {
        return AppData::default();
    };
    let mut data = AppData {
        settings: parse_json_field(&mut object, "settings"),
        assets: parse_json_field(&mut object, "assets"),
        roles: parse_json_field(&mut object, "roles"),
        tasks: vec![],
        task_priorities: object
            .remove("taskPriorities")
            .or_else(|| object.remove("task_priorities"))
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        logs: parse_logs_field(&mut object),
        imagegen_history: parse_json_field(&mut object, "imagegen_history"),
        asset_hash_index: HashMap::new(),
        lane_status: object
            .remove("laneStatus")
            .or_else(|| object.remove("lane_status"))
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
    };

    if let Some(JsonValue::Array(tasks)) = object.remove("tasks") {
        for (index, task_value) in tasks.into_iter().enumerate() {
            match serde_json::from_value::<ScheduledTask>(task_value.clone()) {
                Ok(task) => data.tasks.push(task),
                Err(error) => {
                    let mut task = isolate_schema_error_task(task_value, index, &error.to_string());
                    append_schema_error_log(&mut data, &task, root_error, &error.to_string());
                    task.next_run_at = None;
                    data.tasks.push(task);
                }
            }
        }
    }

    data
}

fn append_schema_error_log(
    data: &mut AppData,
    task: &ScheduledTask,
    root_error: &str,
    task_error: &str,
) {
    append_log(
        data,
        LogEntryDraft {
            level: LogLevel::Error,
            source: LogSource::System,
            category: "state".to_string(),
            event_type: "task_schema_error_isolated".to_string(),
            message: format!("启动校验隔离异常任务：{}", task.title),
            detail: format!(
                "task_id={}\nroot_error={}\ntask_error={}",
                task.id, root_error, task_error
            ),
            task_id: Some(task.id.clone()),
            task_title: Some(task.title.clone()),
            submit_id: if task.submit_id.trim().is_empty() {
                None
            } else {
                Some(task.submit_id.clone())
            },
            execution_record_id: None,
            error_detail: Some(task_error.to_string()),
            raw_output: None,
            stdout: None,
            stderr: None,
            module: Some("state_loader".to_string()),
        },
    );
}

fn isolate_schema_error_task(value: JsonValue, index: usize, task_error: &str) -> ScheduledTask {
    let mut sanitized = value.clone();
    let mut repaired_fields = Vec::new();
    sanitize_task_command_preview_fields(&mut sanitized, &mut repaired_fields);
    let mut task = serde_json::from_value::<ScheduledTask>(sanitized)
        .unwrap_or_else(|_| schema_error_placeholder_task(&value, index));
    task.status = "schema_error".to_string();
    task.next_run_at = None;
    task.auto_query_stopped = true;
    task.updated_at = now_rfc3339();
    let fields = if repaired_fields.is_empty() {
        "未知字段".to_string()
    } else {
        repaired_fields.join(", ")
    };
    task.last_error = format!(
        "启动校验发现任务数据字段异常，已隔离该任务，不影响其他任务。字段：{fields}；错误：{task_error}"
    );
    task
}

fn sanitize_task_command_preview_fields(value: &mut JsonValue, repaired_fields: &mut Vec<String>) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    coerce_command_preview_list(
        object,
        "command_preview",
        "command_preview",
        repaired_fields,
    );
    if let Some(JsonValue::Array(attempts)) = object.get_mut("attempts") {
        for (index, attempt) in attempts.iter_mut().enumerate() {
            if let Some(attempt_object) = attempt.as_object_mut() {
                coerce_command_preview_list(
                    attempt_object,
                    "command_preview",
                    &format!("attempts[{index}].command_preview"),
                    repaired_fields,
                );
            }
        }
    }
    if let Some(JsonValue::Array(records)) = object.get_mut("execution_records") {
        for (record_index, record) in records.iter_mut().enumerate() {
            let Some(record_object) = record.as_object_mut() else {
                continue;
            };
            coerce_command_preview_list(
                record_object,
                "command_preview",
                &format!("execution_records[{record_index}].command_preview"),
                repaired_fields,
            );
            if let Some(JsonValue::Array(queries)) = record_object.get_mut("query_records") {
                for (query_index, query) in queries.iter_mut().enumerate() {
                    if let Some(query_object) = query.as_object_mut() {
                        coerce_command_preview_list(
                            query_object,
                            "command_preview",
                            &format!(
                                "execution_records[{record_index}].query_records[{query_index}].command_preview"
                            ),
                            repaired_fields,
                        );
                    }
                }
            }
        }
    }
}

fn coerce_command_preview_list(
    object: &mut JsonMap<String, JsonValue>,
    key: &str,
    path: &str,
    repaired_fields: &mut Vec<String>,
) {
    let Some(value) = object.get_mut(key) else {
        return;
    };
    if !value.is_array() {
        *value = JsonValue::Array(vec![]);
        repaired_fields.push(path.to_string());
    }
}

fn schema_error_placeholder_task(value: &JsonValue, index: usize) -> ScheduledTask {
    let object = value.as_object();
    let get_string = |key: &str| {
        object
            .and_then(|map| map.get(key))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let now = now_rfc3339();
    ScheduledTask {
        id: {
            let id = get_string("id");
            if id.trim().is_empty() {
                format!("task_schema_error_{index}")
            } else {
                id
            }
        },
        title: {
            let title = get_string("title");
            if title.trim().is_empty() {
                format!("异常任务 {}", index + 1)
            } else {
                title
            }
        },
        prompt: get_string("prompt"),
        image_asset_ids: vec![],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: object
            .and_then(|map| map.get("params"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        status: "schema_error".to_string(),
        scheduled_at: None,
        next_run_at: None,
        queued_at: None,
        submit_id: get_string("submit_id"),
        attempt_count: 0,
        concurrency_retry_count: 0,
        last_error: String::new(),
        command_preview: vec![],
        attempts: vec![],
        result_paths: vec![],
        result_urls: vec![],
        created_at: {
            let created_at = get_string("created_at");
            if created_at.trim().is_empty() {
                now.clone()
            } else {
                created_at
            }
        },
        updated_at: now,
        finished_at: String::new(),
        submitted_at: None,
        queue_info: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        execution_records: vec![],
        last_auto_query_at: None,
        auto_query_stopped: true,
        consecutive_no_result_queries: 0,
        server_error_retry_count: 0,
        planned_submit_count: 1,
        prompt_doc: None,
    }
}

fn normalize_loaded_app_data(data: &mut AppData) {
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    normalize_concurrency_retry_settings(&mut data.settings);
    normalize_image_model_settings(&mut data.settings);
    backfill_execution_records_from_attempts(data);
    compact_retry_execution_records_for_display(data);
    // Sort query_records chronologically before capping (cap drains oldest from front)
    dedupe_all_query_records(data);
    sort_all_query_records(data);
    recover_tasks_on_load(data);
    backfill_draft_command_previews(data);
    apply_log_retention(data);
    cap_execution_history(data);
    rebuild_asset_hash_index(data);
}

fn normalize_concurrency_retry_settings(settings: &mut SchedulerSettings) {
    if settings.concurrency_retry_delay_seconds == LEGACY_CONCURRENCY_RETRY_DELAY_SECONDS {
        settings.concurrency_retry_delay_seconds = DEFAULT_CONCURRENCY_RETRY_DELAY_SECONDS;
    }
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
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
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
    #[serde(default = "default_true")]
    pub standard_lane_enabled: bool,
    #[serde(default = "default_true")]
    pub fast_lane_enabled: bool,
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
fn default_log_retention_days() -> u32 {
    3
}
fn default_mac_install_command() -> String {
    "curl -fsSL https://jimeng.jianying.com/cli | bash".to_string()
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            concurrency_limit_policy: ConcurrencyLimitPolicy::SilentRetry,
            concurrency_retry_delay_seconds: DEFAULT_CONCURRENCY_RETRY_DELAY_SECONDS,
            concurrency_retry_max_attempts: 8,
            auto_query_enabled: true,
            poll_interval_seconds: 60,
            log_retention_count: 500,
            log_retention_days: 3,
            mac_install_command: default_mac_install_command(),
            windows_install_command: String::new(),
            ai_model_configs: default_ai_model_configs(),
            active_ai_model_id: default_active_ai_model_id(),
            prevent_sleep: true,
            standard_lane_enabled: true,
            fast_lane_enabled: true,
            image_model_configs: default_image_model_configs(),
            active_image_model_id: default_active_image_model_id(),
            image_model_config: Some(ImageModelConfig::default()),
        }
    }
}

/// 检查当前是否存在需要防休眠的任务（排队/预定/等待重试/提交中/查询中/非停止的提交后查询）。
pub fn needs_keep_awake(tasks: &[ScheduledTask]) -> bool {
    tasks.iter().any(|t| {
        (matches!(
            t.status.as_str(),
            "queued" | "scheduled" | "retry_wait" | "submitting" | "submitted" | "querying"
        ) || t.execution_records.iter().any(is_active_execution_record))
            && !t.auto_query_stopped
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

/// 根据远端队列状态/位置自适应的自动查询间隔（秒）。
/// 越接近完成查得越勤，不再随查询次数退避增长：
/// - 生成中 Generating         → 60s
/// - 排队中 位置 ≤ 100         → 180s
/// - 排队中 位置 101–1000      → 600s
/// - 排队中 位置 > 1000        → 1200s
/// - 尚无队列信息（刚提交）     → 60s（尽快探到队列位置）
pub fn query_interval_secs(task: &ScheduledTask) -> u64 {
    let Some(qi) = task.queue_info.as_ref() else {
        return 60;
    };
    let is_generating = qi
        .queue_status
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("generating"))
        .unwrap_or(false);
    if is_generating {
        return 60;
    }
    match qi.queue_idx {
        Some(idx) if idx <= 100 => 180,
        Some(idx) if idx <= 1000 => 600,
        Some(_) => 1200,
        // 排队中但拿不到位置 → 保守 3 分钟
        None => 180,
    }
}

/// 判断任务是否到了下一次自动查询时间（按 `query_interval_secs` 的自适应间隔）。
pub fn is_query_due(task: &ScheduledTask, now: DateTime<Utc>) -> bool {
    let Some(ref last_at) = task.last_auto_query_at else {
        return true;
    };
    let interval = Duration::seconds(query_interval_secs(task) as i64);
    DateTime::parse_from_rfc3339(last_at)
        .map(|t| now.signed_duration_since(t.with_timezone(&Utc)) >= interval)
        .unwrap_or(true)
}

fn record_query_due(record: &TaskExecutionRecord, now: DateTime<Utc>) -> bool {
    let Some(last_query) = record.query_records.last() else {
        return true;
    };
    DateTime::parse_from_rfc3339(&last_query.finished_at)
        .or_else(|_| DateTime::parse_from_rfc3339(&last_query.started_at))
        .map(|time| now.signed_duration_since(time.with_timezone(&Utc)) >= Duration::seconds(60))
        .unwrap_or(true)
}

fn query_output_has_explicit_remote_progress(parsed: &QueryOutput) -> bool {
    if parsed.queue_info.is_some()
        || !parsed.result_paths.is_empty()
        || !parsed.result_urls.is_empty()
        || parsed.fail_reason.is_some()
        || parsed.error_code.is_some()
    {
        return true;
    }
    parsed
        .gen_status
        .as_deref()
        .map(|status| {
            matches!(
                status.to_ascii_lowercase().as_str(),
                "running"
                    | "processing"
                    | "generating"
                    | "success"
                    | "succeeded"
                    | "failed"
                    | "cancelled"
            )
        })
        .unwrap_or(false)
}

fn execution_tracking_window_expired(record: &TaskExecutionRecord, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(record.started_at.trim())
        .map(|time| {
            now.signed_duration_since(time.with_timezone(&Utc))
                >= Duration::minutes(MAX_NO_REMOTE_QUEUE_INFO_MINUTES)
        })
        .unwrap_or(false)
}

fn latest_record_query_has_explicit_remote_progress(record: &TaskExecutionRecord) -> bool {
    record.query_records.last().is_some_and(|attempt| {
        let raw = format!("{}\n{}", attempt.stdout, attempt.stderr);
        query_output_has_explicit_remote_progress(&parse_query_output(&raw))
    })
}

fn latest_record_query_was_waitable_error(record: &TaskExecutionRecord) -> bool {
    record.query_records.last().is_some_and(|attempt| {
        attempt.status == "retry_wait"
            && matches!(
                attempt.error_kind.as_str(),
                "NetworkUnavailable" | "Transient" | "ConcurrencyLimit"
            )
    })
}

fn expire_stale_historical_execution_records(
    task: &mut ScheduledTask,
    now: DateTime<Utc>,
) -> Vec<String> {
    let current_submit_id = task.submit_id.trim().to_string();
    let finished_at = now.to_rfc3339();
    let mut expired_submit_ids = Vec::new();
    for record in &mut task.execution_records {
        if record.submit_id == current_submit_id
            || !is_active_execution_record(record)
            || record.query_records.is_empty()
            || !execution_tracking_window_expired(record, now)
            || latest_record_query_has_explicit_remote_progress(record)
            || latest_record_query_was_waitable_error(record)
        {
            continue;
        }
        record.status = "query_timeout".to_string();
        record.finished_at = finished_at.clone();
        record.error_kind = "RemoteTrackingExpired".to_string();
        record.error_detail = format!(
            "远端超过 {} 分钟仅返回 querying 且无排队或生成进度，已停止自动追踪并释放本地车道；不代表生成失败。",
            MAX_NO_REMOTE_QUEUE_INFO_MINUTES
        );
        expired_submit_ids.push(record.submit_id.clone());
    }
    expired_submit_ids
}

fn is_active_execution_record(record: &TaskExecutionRecord) -> bool {
    matches!(record.status.as_str(), "querying" | "submitted")
        && !record.submit_id.trim().is_empty()
        && record.finished_at.trim().is_empty()
}

fn normalize_active_execution_records(task: &mut ScheduledTask) {
    for record in &mut task.execution_records {
        if matches!(record.status.as_str(), "querying" | "submitted")
            && !record.submit_id.trim().is_empty()
        {
            record.finished_at.clear();
        }
    }
}

fn reconcile_task_with_active_execution(task: &mut ScheduledTask) {
    if task.status == "succeeded"
        || (matches!(task.status.as_str(), "querying" | "submitted")
            && !task.submit_id.trim().is_empty())
    {
        return;
    }
    let Some(record) = task
        .execution_records
        .iter()
        .filter(|record| is_active_execution_record(record))
        .max_by(|left, right| left.started_at.cmp(&right.started_at))
    else {
        return;
    };
    let status = record.status.clone();
    let submit_id = record.submit_id.clone();
    let submitted_at = record.started_at.clone();
    let queue_info = execution_record_queue_info(record);

    let was_failed = task.status == "failed";
    task.status = status;
    task.submit_id = submit_id;
    task.submitted_at = Some(submitted_at);
    task.finished_at.clear();
    if was_failed {
        task.last_error.clear();
    }
    task.next_run_at = None;
    task.queue_info = queue_info;
    task.auto_query_stopped = false;
}

fn is_fast_execution_record(record: &TaskExecutionRecord) -> bool {
    record.input_snapshot.params.model_version == FAST_FALLBACK_MODEL_VERSION
        || record
            .command_preview
            .iter()
            .any(|arg| arg == &format!("--model_version={FAST_FALLBACK_MODEL_VERSION}"))
}

fn record_sort_time(record: &TaskExecutionRecord) -> &str {
    if record.finished_at.trim().is_empty() {
        record.started_at.as_str()
    } else {
        record.finished_at.as_str()
    }
}

fn next_due_execution_query_target(data: &AppData, now: DateTime<Utc>) -> Option<(String, String)> {
    for task in data.tasks.iter().filter(|task| !task.auto_query_stopped) {
        if let Some(record) = task.execution_records.iter().find(|record| {
            record.submit_id != task.submit_id
                && is_active_execution_record(record)
                && record_query_due(record, now)
        }) {
            return Some((task.id.clone(), record.submit_id.clone()));
        }
    }
    None
}

fn next_due_current_query_target(data: &AppData, now: DateTime<Utc>) -> Option<(String, String)> {
    for task in data.tasks.iter().filter(|task| {
        matches!(task.status.as_str(), "querying" | "submitted") && !task.auto_query_stopped
    }) {
        if !task.submit_id.trim().is_empty() && is_query_due(task, now) {
            return Some((task.id.clone(), task.submit_id.clone()));
        }
    }
    None
}

fn has_query_history_for_submit(task: &ScheduledTask, submit_id: &str) -> bool {
    if task.attempts.iter().any(|attempt| {
        attempt
            .command_preview
            .iter()
            .any(|arg| arg == &format!("--submit_id={submit_id}"))
    }) {
        return true;
    }
    task.execution_records
        .iter()
        .any(|record| record.submit_id == submit_id && !record.query_records.is_empty())
}

fn latest_query_history_time_for_submit(task: &ScheduledTask, submit_id: &str) -> Option<String> {
    let submit_arg = format!("--submit_id={submit_id}");
    task.execution_records
        .iter()
        .filter(|record| record.submit_id == submit_id)
        .flat_map(|record| record.query_records.iter())
        .chain(
            task.attempts
                .iter()
                .filter(|attempt| attempt.command_preview.iter().any(|arg| arg == &submit_arg)),
        )
        .filter_map(|attempt| {
            let value = if attempt.finished_at.trim().is_empty() {
                attempt.started_at.trim()
            } else {
                attempt.finished_at.trim()
            };
            DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|time| (time.with_timezone(&Utc), value.to_string()))
        })
        .max_by_key(|(time, _)| *time)
        .map(|(_, value)| value)
}

fn next_due_initial_current_query_target(
    data: &AppData,
    now: DateTime<Utc>,
) -> Option<(String, String)> {
    for task in data.tasks.iter().filter(|task| {
        matches!(task.status.as_str(), "querying" | "submitted")
            && !task.auto_query_stopped
            && task.last_auto_query_at.is_none()
    }) {
        if !task.submit_id.trim().is_empty()
            && !has_query_history_for_submit(task, task.submit_id.trim())
            && is_query_due(task, now)
        {
            return Some((task.id.clone(), task.submit_id.clone()));
        }
    }
    None
}

/// 判断任务是否超过最长等待时间，需要停止自动查询。
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
    if query_output_has_explicit_remote_progress(parsed) {
        return true;
    }
    let Some(value) = serde_json::from_str::<serde_json::Value>(raw).ok() else {
        return false;
    };
    find_json_string_field(&value, "task_status")
        .map(|status| {
            matches!(
                status.to_ascii_lowercase().as_str(),
                "queueing" | "running" | "processing" | "generating" | "success" | "succeeded"
            )
        })
        .unwrap_or(false)
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
    NetworkUnavailable,
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
    let mut mentioned_image_asset_ids = Vec::new();
    let mut mentioned_audio_asset_ids = Vec::new();

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
                        &mentioned_image_asset_ids,
                    )
                });
            if let Some(candidate) = candidate {
                match candidate.kind {
                    AssetKind::Image => {
                        push_unique(&mut mentioned_image_asset_ids, candidate.asset_id.clone());
                        let index = mentioned_image_asset_ids
                            .iter()
                            .position(|id| id == &candidate.asset_id)
                            .unwrap_or(mentioned_image_asset_ids.len() - 1)
                            + 1;
                        push_prompt_rewrite(&mut prompt_rewrites, mention, format!("图{index}"));
                    }
                    AssetKind::Audio => {
                        push_unique(&mut mentioned_audio_asset_ids, candidate.asset_id.clone());
                        let index = mentioned_audio_asset_ids
                            .iter()
                            .position(|id| id == &candidate.asset_id)
                            .unwrap_or(mentioned_audio_asset_ids.len() - 1)
                            + 1;
                        push_prompt_rewrite(&mut prompt_rewrites, mention, format!("音频{index}"));
                    }
                }
            } else {
                push_unique(&mut unresolved_mentions, mention);
            }
        }
    }
    if !mentioned_image_asset_ids.is_empty() {
        image_asset_ids = mentioned_image_asset_ids;
    }
    if !mentioned_audio_asset_ids.is_empty() {
        audio_asset_ids = mentioned_audio_asset_ids;
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
        "NetworkUnavailable" => {
            "网络暂不可用或登录刷新失败，已等待网络恢复后自动重试。".to_string()
        }
        "Transient" => "提交时遇到临时网络或平台错误，已自动排队等待下次重试。".to_string(),
        _ => message.trim().to_string(),
    }
}

fn compact_failed_submit_error_detail(kind: &str, message: &str) -> String {
    match kind {
        "ConcurrencyLimit" => "并发任务仍在生成中，已自动排队等待下次重试。".to_string(),
        "NetworkUnavailable" => {
            "网络暂不可用或登录刷新失败，已等待网络恢复后自动重试。".to_string()
        }
        "Transient" => "提交时遇到临时网络或平台错误，自动重试已达上限，已标记失败。".to_string(),
        _ => message.trim().to_string(),
    }
}

fn compact_query_retry_error_detail(kind: &str, message: &str) -> String {
    match kind {
        "ConcurrencyLimit" => "并发任务仍在生成中，已等待下次自动查询。".to_string(),
        "NetworkUnavailable" => {
            "网络暂不可用或登录刷新失败，已等待网络恢复后自动查询。".to_string()
        }
        "Transient" => "查询时遇到临时网络或平台错误，已等待下次自动查询。".to_string(),
        "QueryUnavailable" => "本地查询暂时未取得结果，已保留远端任务，稍后自动查询。".to_string(),
        _ => message.trim().to_string(),
    }
}

fn is_waitable_query_error(kind: &DreaminaErrorKind, next_status: &str) -> bool {
    next_status == "retry_wait"
        && matches!(
            kind,
            DreaminaErrorKind::ConcurrencyLimit
                | DreaminaErrorKind::NetworkUnavailable
                | DreaminaErrorKind::Transient
        )
}

fn should_merge_retry_execution_record(status: &str, error_kind: &str) -> bool {
    matches!(status, "retry_wait" | "failed")
        && matches!(
            error_kind,
            "ConcurrencyLimit" | "NetworkUnavailable" | "Transient"
        )
}

fn execution_record_model_version(record: &TaskExecutionRecord) -> &str {
    record.input_snapshot.params.model_version.as_str()
}

fn upsert_submit_execution_record(task: &mut ScheduledTask, record: TaskExecutionRecord) {
    if should_merge_retry_execution_record(&record.status, &record.error_kind) {
        let model_version = execution_record_model_version(&record);
        if let Some(existing) = task.execution_records.iter_mut().rev().find(|item| {
            matches!(item.status.as_str(), "retry_wait" | "failed")
                && item.error_kind == record.error_kind
                && execution_record_model_version(item) == model_version
        }) {
            existing.finished_at = record.finished_at;
            existing.status = record.status;
            existing.submit_id = record.submit_id;
            existing.input_snapshot = record.input_snapshot;
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
            let model_version = execution_record_model_version(&record).to_string();
            let existing_index = compacted
                .iter()
                .rposition(|existing: &TaskExecutionRecord| {
                    should_merge_retry_execution_record(&existing.status, &existing.error_kind)
                        && existing.error_kind == record.error_kind
                        && execution_record_model_version(existing) == model_version
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

fn is_network_unavailable_error(lower_text: &str) -> bool {
    let authsdk_refresh_failed = lower_text.contains("authsdk")
        && lower_text.contains("refresh failed")
        && (lower_text.contains("protocol transport") || lower_text.contains("do request"));
    authsdk_refresh_failed
        || lower_text.contains("network is unreachable")
        || lower_text.contains("no such host")
        || lower_text.contains("could not resolve host")
        || lower_text.contains("temporary failure in name resolution")
        || lower_text.contains("name resolution failed")
}

pub fn classify_dreamina_error(
    message: &str,
    settings: &SchedulerSettings,
) -> ClassifiedDreaminaError {
    let text = message.trim().to_string();
    let lower_text = text.to_ascii_lowercase();
    let transient_upload_gateway_error = lower_text.contains("upload")
        && (lower_text.contains("bad gateway") || lower_text.contains("backend service failed"));
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
    if is_network_unavailable_error(&lower_text) {
        return ClassifiedDreaminaError {
            kind: DreaminaErrorKind::NetworkUnavailable,
            next_status: "retry_wait".to_string(),
            retry_after_seconds: Some(settings.concurrency_retry_delay_seconds),
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
        || transient_upload_gateway_error
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
        .and_then(|qi| qi.as_object())
        .map(|qi| QueueInfo {
            queue_idx: qi.get("queue_idx").and_then(|v| v.as_u64()),
            priority: qi.get("priority").and_then(|v| v.as_u64()),
            queue_status: qi
                .get("queue_status")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            queue_length: qi.get("queue_length").and_then(|v| v.as_u64()),
        });
    QueryOutput {
        gen_status: parsed
            .as_ref()
            .and_then(|value| find_json_string_field(value, "gen_status"))
            .or_else(|| {
                parsed
                    .as_ref()
                    .and_then(|value| find_json_string_field(value, "task_status"))
            })
            .or_else(|| first_field(&text, "gen_status"))
            .or_else(|| first_field(&text, "task_status"))
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
        series: input.series,
        description: input.description,
        disabled: input.disabled,
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
        prompt_doc: None,
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

/// 通过 MCP 原位替换尚未执行的队列任务。
///
/// 任务 ID 和执行历史保持不变；图片、音频会由调度器资产层重新导入，绝不由调用方直接改状态文件。
pub fn update_queued_mcp_video_task(
    data: &mut AppData,
    assets_dir: &Path,
    input: McpQueuedTaskUpdateInput,
) -> Result<McpQueuedVideoTask, SchedulerError> {
    let task_index = data
        .tasks
        .iter()
        .position(|task| task.id == input.task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{}", input.task_id)))?;
    let previous = data.tasks[task_index].clone();
    if !is_never_executed_queued_task(&previous) {
        return Err(SchedulerError::Io(format!(
            "任务 {} 已执行或不在 queued 状态，禁止原位替换",
            input.task_id
        )));
    }

    preflight_mcp_asset_paths(&input.task.image_paths, AssetKind::Image)?;
    preflight_mcp_asset_paths(&input.task.audio_paths, AssetKind::Audio)?;
    let mcp_assets_dir = assets_dir.join("mcp");
    let mut imported_assets = Vec::new();
    let mut image_asset_ids = Vec::new();
    let mut audio_asset_ids = Vec::new();
    for path in &input.task.image_paths {
        let asset = match import_mcp_asset_from_path(&mcp_assets_dir, path) {
            Ok(asset) => asset,
            Err(error) => {
                cleanup_imported_assets(&imported_assets);
                return Err(error);
            }
        };
        if asset.kind != AssetKind::Image {
            return Err(SchedulerError::UnsupportedAssetType(asset.mime));
        }
        image_asset_ids.push(asset.id.clone());
        imported_assets.push(asset);
    }
    for path in &input.task.audio_paths {
        let asset = match import_mcp_asset_from_path(&mcp_assets_dir, path) {
            Ok(asset) => asset,
            Err(error) => {
                cleanup_imported_assets(&imported_assets);
                return Err(error);
            }
        };
        if asset.kind != AssetKind::Audio {
            return Err(SchedulerError::UnsupportedAssetType(asset.mime));
        }
        audio_asset_ids.push(asset.id.clone());
        imported_assets.push(asset);
    }
    if image_asset_ids.is_empty() {
        return Err(SchedulerError::MissingImageInput);
    }

    let mut params = previous.params.clone();
    if let Some(orientation) = input.task.orientation.as_deref() {
        params.ratio = mcp_orientation_to_ratio(Some(orientation))?;
    }
    if let Some(model) = input.task.model.as_deref() {
        params.model_version = mcp_model_to_version(Some(model))?;
    }
    if let Some(duration) = input.task.duration {
        params.duration = duration;
    }
    if let Some(resolution) = input.task.video_resolution.as_deref() {
        params.video_resolution = resolution.trim().to_string();
    }
    validate_video_params(&params)?;
    let draft = TaskDraft {
        title: if input.task.title.trim().is_empty() {
            previous.title.clone()
        } else {
            input.task.title
        },
        prompt: input.task.prompt,
        image_asset_ids,
        audio_asset_ids,
        role_ids: previous.role_ids.clone(),
        manual_mention_ids: previous.manual_mention_ids.clone(),
        auto_match_roles: previous.auto_match_roles,
        params,
        scheduled_at: previous.scheduled_at.clone(),
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        prompt_doc: None,
    };

    let mut preview_data = data.clone();
    preview_data.assets.extend(imported_assets.clone());
    let update_result = (|| {
        let (preview, _) = build_optional_draft_preview_from_parts(
            &draft,
            &preview_data.assets,
            &preview_data.roles,
        )?;
        data.assets.extend(imported_assets.clone());
        let updated = update_task_from_data(data, &input.task_id, draft, "task")?;
        Ok::<_, SchedulerError>((updated, preview))
    })();
    let (updated, preview) = match update_result {
        Ok(value) => value,
        Err(error) => {
            cleanup_imported_assets(&imported_assets);
            return Err(error);
        }
    };
    debug_assert_eq!(updated.command_preview, preview);
    append_task_log(
        data,
        &updated,
        LogEntryDraft {
            level: LogLevel::Info,
            source: LogSource::Scheduler,
            category: "task".to_string(),
            event_type: "mcp_update_queued".to_string(),
            message: format!("MCP 原位更新未执行任务：{}", updated.title),
            detail: String::new(),
            task_id: None,
            task_title: None,
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
        task: updated,
        imported_assets,
    })
}

/// 通过 MCP 原位替换失败或等待重试任务的生成输入，并保持为失败草稿。
///
/// 该入口只保存下一次手动重试要使用的新版草稿，不会重新排队或唤醒调度器；
/// 任务 ID、submit ID、尝试次数、执行记录、结果和错误记录全部保留。
pub fn update_failed_mcp_video_task_draft(
    data: &mut AppData,
    assets_dir: &Path,
    input: McpQueuedTaskUpdateInput,
) -> Result<McpQueuedVideoTask, SchedulerError> {
    let task_index = data
        .tasks
        .iter()
        .position(|task| task.id == input.task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{}", input.task_id)))?;
    let previous = data.tasks[task_index].clone();
    if !matches!(previous.status.as_str(), "failed" | "retry_wait") {
        return Err(SchedulerError::Io(format!(
            "任务 {} 当前状态为 {}，只允许原位更新 failed 或 retry_wait 任务草稿",
            input.task_id, previous.status
        )));
    }

    preflight_mcp_asset_paths(&input.task.image_paths, AssetKind::Image)?;
    preflight_mcp_asset_paths(&input.task.audio_paths, AssetKind::Audio)?;
    let mcp_assets_dir = assets_dir.join("mcp");
    let mut imported_assets = Vec::new();
    let mut image_asset_ids = Vec::new();
    let mut audio_asset_ids = Vec::new();
    for path in &input.task.image_paths {
        let asset = match import_mcp_asset_from_path(&mcp_assets_dir, path) {
            Ok(asset) => asset,
            Err(error) => {
                cleanup_imported_assets(&imported_assets);
                return Err(error);
            }
        };
        if asset.kind != AssetKind::Image {
            cleanup_imported_assets(&imported_assets);
            return Err(SchedulerError::UnsupportedAssetType(asset.mime));
        }
        image_asset_ids.push(asset.id.clone());
        imported_assets.push(asset);
    }
    for path in &input.task.audio_paths {
        let asset = match import_mcp_asset_from_path(&mcp_assets_dir, path) {
            Ok(asset) => asset,
            Err(error) => {
                cleanup_imported_assets(&imported_assets);
                return Err(error);
            }
        };
        if asset.kind != AssetKind::Audio {
            cleanup_imported_assets(&imported_assets);
            return Err(SchedulerError::UnsupportedAssetType(asset.mime));
        }
        audio_asset_ids.push(asset.id.clone());
        imported_assets.push(asset);
    }
    if image_asset_ids.is_empty() {
        return Err(SchedulerError::MissingImageInput);
    }

    let mut params = previous.params.clone();
    if let Some(orientation) = input.task.orientation.as_deref() {
        params.ratio = mcp_orientation_to_ratio(Some(orientation))?;
    }
    if let Some(model) = input.task.model.as_deref() {
        params.model_version = mcp_model_to_version(Some(model))?;
    }
    if let Some(duration) = input.task.duration {
        params.duration = duration;
    }
    if let Some(resolution) = input.task.video_resolution.as_deref() {
        params.video_resolution = resolution.trim().to_string();
    }
    validate_video_params(&params)?;
    let draft = TaskDraft {
        title: if input.task.title.trim().is_empty() {
            previous.title.clone()
        } else {
            input.task.title
        },
        prompt: input.task.prompt,
        image_asset_ids,
        audio_asset_ids,
        role_ids: previous.role_ids.clone(),
        manual_mention_ids: previous.manual_mention_ids.clone(),
        auto_match_roles: previous.auto_match_roles,
        params,
        scheduled_at: previous.scheduled_at.clone(),
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        prompt_doc: None,
    };

    let mut preview_data = data.clone();
    preview_data.assets.extend(imported_assets.clone());
    let update_result = (|| {
        let (preview, _) = build_optional_draft_preview_from_parts(
            &draft,
            &preview_data.assets,
            &preview_data.roles,
        )?;
        data.assets.extend(imported_assets.clone());
        let mut updated = update_task_from_data(data, &input.task_id, draft, "draft")?;
        if previous.status == "retry_wait" {
            let task = data
                .tasks
                .iter_mut()
                .find(|task| task.id == input.task_id)
                .expect("task exists after draft update");
            task.status = "failed".to_string();
            task.next_run_at = None;
            updated = task.clone();
        }
        Ok::<_, SchedulerError>((updated, preview))
    })();
    let (updated, preview) = match update_result {
        Ok(value) => value,
        Err(error) => {
            cleanup_imported_assets(&imported_assets);
            return Err(error);
        }
    };
    debug_assert_eq!(updated.command_preview, preview);
    append_task_log(
        data,
        &updated,
        LogEntryDraft {
            level: LogLevel::Info,
            source: LogSource::Scheduler,
            category: "task".to_string(),
            event_type: "mcp_update_failed_draft".to_string(),
            message: format!("MCP 原位保存失败任务新版草稿：{}", updated.title),
            detail: "未重新排队，等待手动重试".to_string(),
            task_id: None,
            task_title: None,
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
        task: updated,
        imported_assets,
    })
}

fn is_never_executed_queued_task(task: &ScheduledTask) -> bool {
    task.status == "queued"
        && task.attempt_count == 0
        && task.attempts.is_empty()
        && task.submit_id.trim().is_empty()
        && task.submitted_at.is_none()
        && task.finished_at.trim().is_empty()
        && task.result_paths.is_empty()
        && task.result_urls.is_empty()
        && task.execution_records.is_empty()
}

fn preflight_mcp_asset_paths(
    paths: &[String],
    expected_kind: AssetKind,
) -> Result<(), SchedulerError> {
    for path in paths {
        let source = PathBuf::from(path);
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let actual_kind = match extension.as_str() {
            "png" | "jpg" | "jpeg" | "webp" => AssetKind::Image,
            "mp3" | "wav" | "m4a" | "aac" => AssetKind::Audio,
            "mp4" | "mov" | "webm" | "mkv" => return Err(SchedulerError::UnsupportedVideoAsset),
            _ => return Err(SchedulerError::UnsupportedAssetType(extension)),
        };
        if actual_kind != expected_kind {
            return Err(SchedulerError::UnsupportedAssetType(path.clone()));
        }
        let metadata =
            fs::metadata(&source).map_err(|error| SchedulerError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(SchedulerError::Io(format!(
                "素材不是文件：{}",
                source.display()
            )));
        }
    }
    Ok(())
}

fn cleanup_imported_assets(assets: &[Asset]) {
    for asset in assets {
        let _ = fs::remove_file(&asset.stored_path);
    }
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
    let alternate_fast_model = input
        .defaults
        .alternate_fast_model
        .unwrap_or(input.alternate_fast_model);
    for (index, item) in input.items.into_iter().enumerate() {
        let item_has_explicit_model = item.model.is_some();
        let mut merged = merge_mcp_video_defaults(item, &input.start_at, &input.defaults);
        if alternate_fast_model && !item_has_explicit_model {
            merged.model = Some(if index % 2 == 0 {
                "standard".to_string()
            } else {
                "fast".to_string()
            });
        }
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
            let error_detail =
                submit_execution_record_error_detail(&status, &error_kind, &attempt.error_detail);
            let before_len = task.execution_records.len();
            upsert_submit_execution_record(
                task,
                TaskExecutionRecord {
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
                },
            );
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
            if !query_record_exists(&record.query_records, attempt) {
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
                    if !task.finished_at.trim().is_empty() {
                        record.finished_at = task.finished_at.clone();
                    }
                    record.error_kind.clear();
                    record.error_detail.clear();
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
        normalize_active_execution_records(task);
        if !expire_stale_historical_execution_records(task, Utc::now()).is_empty() {
            task.updated_at = now_rfc3339();
        }
        reconcile_task_with_active_execution(task);
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
            let now = now_rfc3339();
            task.status = "queued".to_string();
            task.queued_at = Some(now.clone());
            task.updated_at = now;
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
                let current_submit_id = task.submit_id.trim().to_string();
                let latest_query_at =
                    latest_query_history_time_for_submit(task, &current_submit_id);
                // 无结果：标记为 submitted；如果已有查询历史，保留轮询节奏，避免每次进程启动都抢占调度。
                task.status = "submitted".to_string();
                task.last_error.clear();
                task.auto_query_stopped = false;
                if let Some(latest_query_at) = latest_query_at {
                    task.last_auto_query_at = Some(latest_query_at);
                    if task.consecutive_no_result_queries == 0 {
                        task.consecutive_no_result_queries = 1;
                    }
                } else {
                    reset_query_backoff(task);
                }
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

    // 清理 task.attempts 中关联被删记录的 entry，防止 backfill 重建
    let remaining_submit_ids: Vec<String> = data.tasks[task_index]
        .execution_records
        .iter()
        .map(|r| r.submit_id.clone())
        .collect();
    data.tasks[task_index].attempts.retain(|a| {
        let cmd = a.command_preview.first().map(|s| s.as_str()).unwrap_or("");
        if cmd == "multimodal2video" {
            let raw = format!("{}\n{}", a.stdout, a.stderr);
            let parsed = parse_submit_output(&raw);
            let sid = parsed.submit_id.unwrap_or_default();
            remaining_submit_ids.contains(&sid)
        } else if cmd == "query_result" {
            let sid = query_submit_id_from_attempt(a);
            remaining_submit_ids.contains(&sid)
        } else {
            true
        }
    });

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
        prompt_doc: task.prompt_doc.clone(),
    }
}

fn touch_asset_last_used(data: &mut AppData, image_ids: &[String], audio_ids: &[String]) {
    let now = now_rfc3339();
    for asset in &mut data.assets {
        if image_ids.contains(&asset.id) || audio_ids.contains(&asset.id) {
            asset.last_used_at = Some(now.clone());
        }
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

fn model_queue_kind(model_version: &str) -> ModelQueueKind {
    if model_version == FAST_FALLBACK_MODEL_VERSION {
        ModelQueueKind::Fast
    } else {
        ModelQueueKind::Standard
    }
}

fn task_model_queue_kind(task: &ScheduledTask) -> ModelQueueKind {
    model_queue_kind(&task.params.model_version)
}

fn execution_record_model_queue_kind(record: &TaskExecutionRecord) -> ModelQueueKind {
    model_queue_kind(&record.input_snapshot.params.model_version)
}

fn latest_retry_wait_execution_record(task: &ScheduledTask) -> Option<&TaskExecutionRecord> {
    task.execution_records
        .iter()
        .filter(|record| record.status == "retry_wait")
        .max_by(|left, right| record_sort_time(left).cmp(record_sort_time(right)))
        .or_else(|| {
            task.execution_records
                .iter()
                .max_by(|left, right| record_sort_time(left).cmp(record_sort_time(right)))
        })
}

fn submit_queue_kind_for_task(task: &ScheduledTask) -> ModelQueueKind {
    if task.status == "retry_wait" {
        return latest_retry_wait_execution_record(task)
            .map(execution_record_model_queue_kind)
            .unwrap_or_else(|| task_model_queue_kind(task));
    }
    if matches!(
        task.status.as_str(),
        "submitting" | "submitted" | "querying"
    ) && !task.submit_id.trim().is_empty()
    {
        if let Some(record) = task
            .execution_records
            .iter()
            .find(|record| record.submit_id == task.submit_id)
        {
            return execution_record_model_queue_kind(record);
        }
    }
    task_model_queue_kind(task)
}

fn lane_enabled(settings: &SchedulerSettings, kind: ModelQueueKind) -> bool {
    match kind {
        ModelQueueKind::Standard => settings.standard_lane_enabled,
        ModelQueueKind::Fast => settings.fast_lane_enabled,
    }
}

fn set_lane_enabled(
    data: &mut AppData,
    kind: ModelQueueKind,
    enabled: bool,
) -> Result<(), SchedulerError> {
    let (standard_enabled, fast_enabled) = match kind {
        ModelQueueKind::Standard => (enabled, data.settings.fast_lane_enabled),
        ModelQueueKind::Fast => (data.settings.standard_lane_enabled, enabled),
    };
    if !standard_enabled && !fast_enabled {
        return Err(SchedulerError::Io("至少保留一条车道".to_string()));
    }
    data.settings.standard_lane_enabled = standard_enabled;
    data.settings.fast_lane_enabled = fast_enabled;
    Ok(())
}

fn parse_lane_kind(value: &str) -> Result<ModelQueueKind, SchedulerError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "standard" | "seedance2.0" => Ok(ModelQueueKind::Standard),
        "fast" | "seedance2.0fast" => Ok(ModelQueueKind::Fast),
        other => Err(SchedulerError::Io(format!("未知车道：{other}"))),
    }
}

fn active_remote_queue_kinds(data: &AppData) -> HashSet<ModelQueueKind> {
    let mut kinds = HashSet::new();
    for task in &data.tasks {
        if task.status == "submitting"
            || ((task.status == "querying" || task.status == "submitted")
                && !task.submit_id.trim().is_empty()
                && !task.auto_query_stopped)
        {
            kinds.insert(submit_queue_kind_for_task(task));
        }
        for record in task
            .execution_records
            .iter()
            .filter(|record| is_active_execution_record(record) && !task.auto_query_stopped)
        {
            kinds.insert(execution_record_model_queue_kind(record));
        }
    }
    kinds
}

fn retry_wait_task_has_concurrency_cooldown(
    task: &ScheduledTask,
    now: DateTime<Utc>,
    queue_kind: ModelQueueKind,
) -> bool {
    if task.status != "retry_wait"
        || submit_queue_kind_for_task(task) != queue_kind
        || is_due(task.next_run_at.as_deref(), now)
    {
        return false;
    }
    is_concurrency_limit(&task.last_error)
        || latest_retry_wait_execution_record(task)
            .map(|record| {
                record.error_kind == "ConcurrencyLimit"
                    || is_concurrency_limit(&record.error_detail)
            })
            .unwrap_or(false)
}

fn has_concurrency_cooldown_for_queue(
    data: &AppData,
    now: DateTime<Utc>,
    queue_kind: ModelQueueKind,
) -> bool {
    data.tasks
        .iter()
        .any(|task| retry_wait_task_has_concurrency_cooldown(task, now, queue_kind))
}

fn model_queue_available(
    data: &AppData,
    now: DateTime<Utc>,
    active_kinds: &HashSet<ModelQueueKind>,
    queue_kind: ModelQueueKind,
) -> bool {
    lane_enabled(&data.settings, queue_kind)
        && !active_kinds.contains(&queue_kind)
        && !has_concurrency_cooldown_for_queue(data, now, queue_kind)
}

fn task_next_query_at(task: &ScheduledTask, now: DateTime<Utc>) -> String {
    task.last_auto_query_at
        .as_ref()
        .and_then(|last_at| {
            let interval = query_interval_secs(task);
            DateTime::parse_from_rfc3339(last_at)
                .ok()
                .map(|t| t.with_timezone(&Utc) + Duration::seconds(interval as i64))
        })
        .map(|next| next.to_rfc3339())
        .unwrap_or_else(|| now.to_rfc3339())
}

fn record_next_query_at(record: &TaskExecutionRecord, now: DateTime<Utc>) -> String {
    record
        .query_records
        .last()
        .and_then(|last_query| {
            DateTime::parse_from_rfc3339(&last_query.finished_at)
                .or_else(|_| DateTime::parse_from_rfc3339(&last_query.started_at))
                .ok()
                .map(|t| t.with_timezone(&Utc) + Duration::seconds(60))
        })
        .map(|next| next.to_rfc3339())
        .unwrap_or_else(|| now.to_rfc3339())
}

fn execution_record_queue_info(record: &TaskExecutionRecord) -> Option<QueueInfo> {
    record
        .query_records
        .iter()
        .rev()
        .find_map(|attempt| parse_query_output(&attempt.stdout).queue_info)
}

fn active_lane_occupant(
    data: &AppData,
    now: DateTime<Utc>,
    queue_kind: ModelQueueKind,
) -> Option<(String, String, String, Option<u64>, Option<u64>, String)> {
    if let Some(task) =
        data.tasks
            .iter()
            .filter(|task| {
                (task.status == "querying" || task.status == "submitted")
                    && !task.submit_id.trim().is_empty()
                    && !task.auto_query_stopped
                    && submit_queue_kind_for_task(task) == queue_kind
                    && task.queue_info.as_ref().is_some_and(|queue| {
                        queue.queue_idx.is_some() || queue.queue_length.is_some()
                    })
            })
            .max_by(|left, right| left.last_auto_query_at.cmp(&right.last_auto_query_at))
    {
        let qi = task.queue_info.as_ref();
        return Some((
            task.id.clone(),
            task.title.clone(),
            task.submit_id.clone(),
            qi.and_then(|q| q.queue_idx),
            qi.and_then(|q| q.queue_length),
            task_next_query_at(task, now),
        ));
    }

    for task in &data.tasks {
        if task.auto_query_stopped {
            continue;
        }
        if let Some((record, qi)) = task.execution_records.iter().find_map(|record| {
            if !is_active_execution_record(record)
                || execution_record_model_queue_kind(record) != queue_kind
            {
                return None;
            }
            let qi = execution_record_queue_info(record)?;
            (qi.queue_idx.is_some() || qi.queue_length.is_some()).then_some((record, qi))
        }) {
            return Some((
                task.id.clone(),
                task.title.clone(),
                record.submit_id.clone(),
                qi.queue_idx,
                qi.queue_length,
                record_next_query_at(record, now),
            ));
        }
    }

    if let Some(task) = data.tasks.iter().find(|task| {
        (task.status == "submitting"
            || ((task.status == "querying" || task.status == "submitted")
                && !task.submit_id.trim().is_empty()
                && !task.auto_query_stopped))
            && submit_queue_kind_for_task(task) == queue_kind
    }) {
        let qi = task.queue_info.as_ref();
        return Some((
            task.id.clone(),
            task.title.clone(),
            task.submit_id.clone(),
            qi.and_then(|q| q.queue_idx),
            qi.and_then(|q| q.queue_length),
            task_next_query_at(task, now),
        ));
    }

    for task in &data.tasks {
        if task.auto_query_stopped {
            continue;
        }
        if let Some(record) = task.execution_records.iter().find(|record| {
            is_active_execution_record(record)
                && execution_record_model_queue_kind(record) == queue_kind
        }) {
            let qi = execution_record_queue_info(record);
            return Some((
                task.id.clone(),
                task.title.clone(),
                record.submit_id.clone(),
                qi.as_ref().and_then(|q| q.queue_idx),
                qi.as_ref().and_then(|q| q.queue_length),
                record_next_query_at(record, now),
            ));
        }
    }

    None
}

fn compute_lane_status(data: &AppData, now: DateTime<Utc>) -> Vec<LaneStatus> {
    let active_kinds = active_remote_queue_kinds(data);
    let queue_kinds = [ModelQueueKind::Standard, ModelQueueKind::Fast];

    queue_kinds
        .iter()
        .map(|&kind| {
            let model_version = match kind {
                ModelQueueKind::Standard => "seedance2.0",
                ModelQueueKind::Fast => FAST_FALLBACK_MODEL_VERSION,
            };
            let kind_str = match kind {
                ModelQueueKind::Standard => "standard",
                ModelQueueKind::Fast => "fast",
            };

            let enabled = lane_enabled(&data.settings, kind);
            let is_active = enabled && active_kinds.contains(&kind);
            let is_cooling_down = enabled && has_concurrency_cooldown_for_queue(data, now, kind);
            let active_occupant = active_lane_occupant(data, now, kind);

            let (
                current_task_id,
                current_task_title,
                submit_id,
                queue_position,
                queue_length,
                active_next_check_at,
            ) = active_occupant.unwrap_or_else(|| {
                (
                    String::new(),
                    String::new(),
                    String::new(),
                    None,
                    None,
                    now.to_rfc3339(),
                )
            });

            let cooldown_tasks: Vec<&ScheduledTask> = data
                .tasks
                .iter()
                .filter(|task| retry_wait_task_has_concurrency_cooldown(task, now, kind))
                .collect();

            // Compute next_check_at
            let next_check_at = if is_active {
                active_next_check_at
            } else if is_cooling_down {
                // Cooling down: next check = earliest concurrency cooldown end
                cooldown_tasks
                    .iter()
                    .filter_map(|t| t.next_run_at.as_ref())
                    .min()
                    .cloned()
                    .unwrap_or_else(|| now.to_rfc3339())
            } else {
                // Available/looking: next check = earliest due task for this lane
                data.tasks
                    .iter()
                    .filter(|t| {
                        (t.status == "queued"
                            || t.status == "retry_wait"
                            || t.status == "scheduled")
                            && submit_queue_kind_for_task(t) == kind
                    })
                    .filter_map(|t| t.next_run_at.as_ref())
                    .min()
                    .cloned()
                    .unwrap_or_else(|| now.to_rfc3339())
            };

            // Count waiting tasks for this lane
            let waiting_task_count = data
                .tasks
                .iter()
                .filter(|t| {
                    matches!(
                        t.status.as_str(),
                        "queued" | "retry_wait" | "scheduled" | "draft"
                    ) && submit_queue_kind_for_task(t) == kind
                })
                .count() as u32;

            // Cooldown reason
            let cooldown_reason = if is_cooling_down {
                let hit_count = cooldown_tasks.len();
                if hit_count > 0 {
                    format!("并发限制，{} 个任务等待重试", hit_count)
                } else {
                    "并发限制冷却中".to_string()
                }
            } else {
                String::new()
            };

            LaneStatus {
                queue_kind: kind_str.to_string(),
                model_version: model_version.to_string(),
                enabled,
                is_active,
                is_cooling_down,
                cooldown_reason,
                current_task_id,
                current_task_title,
                submit_id,
                queue_position,
                queue_length,
                next_check_at,
                waiting_task_count,
            }
        })
        .collect()
}

fn is_due_for_submit(task: &ScheduledTask, now: DateTime<Utc>) -> bool {
    task.status == "queued"
        || (task.status == "retry_wait" && is_due(task.next_run_at.as_deref(), now))
        || (task.status == "scheduled" && is_due(task.next_run_at.as_deref(), now))
}

fn non_empty_task_time(value: Option<&String>) -> Option<String> {
    value
        .filter(|time| !time.trim().is_empty())
        .map(|time| time.to_string())
}

fn queue_started_sort_key(task: &ScheduledTask) -> String {
    non_empty_task_time(task.queued_at.as_ref())
        .or_else(|| non_empty_task_time(Some(&task.updated_at)))
        .unwrap_or_else(|| task.created_at.clone())
}

fn due_sort_key(task: &ScheduledTask) -> String {
    non_empty_task_time(task.next_run_at.as_ref())
        .or_else(|| non_empty_task_time(task.scheduled_at.as_ref()))
        .unwrap_or_else(|| queue_started_sort_key(task))
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
    if task.status != "failed"
        || !needs_more_successful_submits(task)
        || task
            .execution_records
            .iter()
            .any(is_active_execution_record)
    {
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
                && (record.error_kind == "ConcurrencyLimit"
                    || is_concurrency_limit(&task.last_error))
        })
        .unwrap_or(false)
}

fn recover_failed_concurrency_execution_record(task: &mut ScheduledTask) {
    if let Some(record) = task.execution_records.iter_mut().rev().find(|record| {
        record.status == "failed"
            && (record.error_kind == "ConcurrencyLimit"
                || is_concurrency_limit(&record.error_detail))
    }) {
        record.status = "retry_wait".to_string();
        record.error_kind = "ConcurrencyLimit".to_string();
        record.error_detail = compact_submit_error_detail("ConcurrencyLimit", &record.error_detail);
        record.finished_at = now_rfc3339();
    }
}

#[cfg(test)]
fn next_due_submit_task_id(data: &AppData, now: DateTime<Utc>) -> Option<String> {
    next_due_submit_task_id_for_queue(data, now, None)
}

fn next_due_submit_task_id_for_queue(
    data: &AppData,
    now: DateTime<Utc>,
    queue_kind: Option<ModelQueueKind>,
) -> Option<String> {
    data.tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| {
            needs_more_successful_submits(task)
                && queue_kind
                    .map(|kind| task_is_due_for_target_queue(task, now, kind))
                    .unwrap_or_else(|| is_due_for_submit(task, now))
        })
        .min_by(|(left_index, left), (right_index, right)| {
            compare_submit_candidates(data, *left_index, left, *right_index, right)
        })
        .map(|(_, task)| task.id.clone())
}

fn next_idle_failed_retry_task_id_for_queue(
    data: &AppData,
    now: DateTime<Utc>,
    queue_kind: Option<ModelQueueKind>,
) -> Option<String> {
    let retry_delay_seconds = data.settings.concurrency_retry_delay_seconds;
    data.tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| {
            let _ = queue_kind;
            is_idle_failed_retry_due(task, now, retry_delay_seconds)
        })
        .min_by(|(left_index, left), (right_index, right)| {
            compare_submit_candidates(data, *left_index, left, *right_index, right).then_with(
                || {
                    retryable_failed_execution_count(left, "Transient")
                        .cmp(&retryable_failed_execution_count(right, "Transient"))
                },
            )
        })
        .map(|(_, task)| task.id.clone())
}

fn task_queue_priority(data: &AppData, task_id: &str) -> u8 {
    data.task_priorities
        .get(task_id)
        .copied()
        .unwrap_or(0)
        .min(2)
}

fn is_generation_precheck_retry_wait(task: &ScheduledTask) -> bool {
    task.status == "retry_wait"
        && latest_retry_wait_execution_record(task)
            .map(|record| {
                record.error_kind == "GenerationPrecheck"
                    || is_generation_precheck_failure(&record.error_detail)
            })
            .unwrap_or_else(|| is_generation_precheck_failure(&task.last_error))
}

fn compare_submit_candidates(
    data: &AppData,
    left_index: usize,
    left: &ScheduledTask,
    right_index: usize,
    right: &ScheduledTask,
) -> std::cmp::Ordering {
    let left_review_retry = is_generation_precheck_retry_wait(left);
    let right_review_retry = is_generation_precheck_retry_wait(right);
    let left_priority = task_queue_priority(data, &left.id);
    let right_priority = task_queue_priority(data, &right.id);
    left_review_retry
        .cmp(&right_review_retry)
        .then_with(|| right_priority.cmp(&left_priority))
        .then_with(|| successful_execution_count(left).cmp(&successful_execution_count(right)))
        .then_with(|| {
            if left_priority == 0 && right_priority == 0 {
                left.id.cmp(&right.id)
            } else {
                queue_started_sort_key(left).cmp(&queue_started_sort_key(right))
            }
        })
        .then_with(|| due_sort_key(left).cmp(&due_sort_key(right)))
        .then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left_index.cmp(&right_index))
}

fn set_task_queue_priority(
    data: &mut AppData,
    task_id: &str,
    priority: u8,
) -> Result<u8, SchedulerError> {
    if !data.tasks.iter().any(|task| task.id == task_id) {
        return Err(SchedulerError::Io(format!("找不到任务：{task_id}")));
    }
    let priority = priority.min(2);
    if priority == 0 {
        data.task_priorities.remove(task_id);
    } else {
        data.task_priorities.insert(task_id.to_string(), priority);
    }
    Ok(priority)
}

fn next_submit_task_id_for_queue(
    data: &AppData,
    now: DateTime<Utc>,
    queue_kind: Option<ModelQueueKind>,
) -> Option<String> {
    next_due_submit_task_id_for_queue(data, now, queue_kind)
        .or_else(|| next_idle_failed_retry_task_id_for_queue(data, now, queue_kind))
}

fn next_submit_task_id_for_available_queues(
    data: &AppData,
    now: DateTime<Utc>,
    active_kinds: &HashSet<ModelQueueKind>,
) -> Option<SubmitQueueSelection> {
    for kind in [ModelQueueKind::Standard, ModelQueueKind::Fast] {
        if !model_queue_available(data, now, active_kinds, kind) {
            continue;
        }
        if let Some(task_id) = next_submit_task_id_for_queue(data, now, Some(kind)) {
            return Some(SubmitQueueSelection {
                task_id,
                target_queue_kind: kind,
            });
        }
    }

    None
}

fn task_is_due_for_target_queue(
    task: &ScheduledTask,
    now: DateTime<Utc>,
    target_queue_kind: ModelQueueKind,
) -> bool {
    let is_concurrency_retry = is_concurrency_limit(&task.last_error)
        || latest_retry_wait_execution_record(task)
            .map(|record| {
                record.error_kind == "ConcurrencyLimit"
                    || is_concurrency_limit(&record.error_detail)
            })
            .unwrap_or(false);
    let can_switch_from_cooling_lane = task.status == "retry_wait"
        && is_concurrency_retry
        && submit_queue_kind_for_task(task) != target_queue_kind;
    can_switch_from_cooling_lane || is_due_for_submit(task, now)
}

fn apply_planned_submit_completion(task: &mut ScheduledTask) {
    if task.status != "succeeded" {
        return;
    }
    if needs_more_successful_submits(task) {
        let now = now_rfc3339();
        task.status = "queued".to_string();
        task.next_run_at = None;
        task.queued_at = Some(now.clone());
        task.updated_at = now;
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
                let now = now_rfc3339();
                task.status = "queued".to_string();
                task.next_run_at = None;
                task.queued_at = Some(now);
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

pub fn queue_tasks_with_model_strategy(
    data: &mut AppData,
    task_ids: &[String],
    new_scheduled_at: &str,
    planned_submit_count: u32,
    alternate_fast_model: bool,
) -> Result<Vec<ScheduledTask>, SchedulerError> {
    let plan: Vec<BatchQueuePlanItem> = task_ids
        .iter()
        .map(|task_id| BatchQueuePlanItem {
            task_id: task_id.clone(),
            scheduled_at: if new_scheduled_at.trim().is_empty() {
                None
            } else {
                Some(new_scheduled_at.to_string())
            },
        })
        .collect();
    queue_tasks_with_batch_schedule(data, &plan, planned_submit_count, alternate_fast_model)
}

pub fn queue_tasks_with_batch_schedule(
    data: &mut AppData,
    plan: &[BatchQueuePlanItem],
    planned_submit_count: u32,
    alternate_fast_model: bool,
) -> Result<Vec<ScheduledTask>, SchedulerError> {
    if plan.is_empty() {
        return Ok(vec![]);
    }
    for item in plan {
        let Some(scheduled_at) = item.scheduled_at.as_ref().map(|value| value.trim()) else {
            continue;
        };
        if scheduled_at.is_empty() {
            continue;
        }
        if let Ok(time) = DateTime::parse_from_rfc3339(scheduled_at) {
            if time.with_timezone(&Utc) <= Utc::now() {
                return Err(SchedulerError::ScheduledAtInPast);
            }
        }
    }

    let allowed = [
        "draft",
        "queued",
        "scheduled",
        "paused",
        "retry_wait",
        "failed",
        "succeeded",
    ];
    let mut seen: HashSet<String> = HashSet::new();
    let mut selected_tasks = Vec::with_capacity(plan.len());
    for item in plan {
        let task_id = &item.task_id;
        if !seen.insert(task_id.clone()) {
            return Err(SchedulerError::Io(format!("任务重复：{task_id}")));
        }
        let index = data
            .tasks
            .iter()
            .position(|task| task.id == *task_id)
            .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
        let task = &data.tasks[index];
        if !allowed.contains(&task.status.as_str()) {
            return Err(SchedulerError::Io(format!(
                "当前状态 {} 不可重新排期",
                task.status
            )));
        }
        if alternate_fast_model && task.params.model_version == "seedance2.0fast" {
            // 已经是 Fast 模型的任务跳过，不参与交替排队
            continue;
        }
        selected_tasks.push((index, item.scheduled_at.clone()));
    }

    let normalized_count = normalize_planned_submit_count(planned_submit_count);
    let now = now_rfc3339();
    let mut updated = Vec::with_capacity(selected_tasks.len());
    for (index, scheduled_at) in selected_tasks {
        let task = &mut data.tasks[index];
        let scheduled_at = scheduled_at
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        task.planned_submit_count = normalized_count;
        ensure_task_has_pending_submit(task);
        if let Some(scheduled_at) = scheduled_at {
            task.scheduled_at = Some(scheduled_at.to_string());
            task.next_run_at = Some(scheduled_at.to_string());
            task.status = "scheduled".to_string();
        } else {
            task.scheduled_at = None;
            task.next_run_at = None;
            task.status = "queued".to_string();
        }
        task.queued_at = Some(now.clone());
        task.updated_at = now.clone();
        updated.push(task.clone());
    }

    Ok(updated)
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
    for task in data
        .tasks
        .iter_mut()
        .filter(|task| task.status == "scheduled" && is_due(task.next_run_at.as_deref(), now))
    {
        task.status = "queued".to_string();
        task.updated_at = now_rfc3339();
    }

    let active_kinds = active_remote_queue_kinds(data);
    if let Some(selection) = next_submit_task_id_for_available_queues(data, now, &active_kinds) {
        let model_version_override = match selection.target_queue_kind {
            ModelQueueKind::Standard => Some("seedance2.0"),
            ModelQueueKind::Fast => Some(FAST_FALLBACK_MODEL_VERSION),
        };

        return submit_task_once_with_runner(
            data,
            &selection.task_id,
            model_version_override,
            &mut runner,
        )
        .map(Some);
    }

    if data.settings.auto_query_enabled {
        if let Some((task_id, submit_id)) = next_due_initial_current_query_target(data, now) {
            return query_task_submit_id_once_with_runner(data, &task_id, &submit_id, &mut runner)
                .map(Some);
        }

        if let Some((task_id, submit_id)) = next_due_execution_query_target(data, now) {
            return query_task_submit_id_once_with_runner(data, &task_id, &submit_id, &mut runner)
                .map(Some);
        }

        if let Some((task_id, submit_id)) = next_due_current_query_target(data, now) {
            return query_task_submit_id_once_with_runner(data, &task_id, &submit_id, &mut runner)
                .map(Some);
        }
    }

    Ok(None)
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

pub fn pause_tasks(
    data: &mut AppData,
    task_ids: &[String],
) -> Result<Vec<ScheduledTask>, SchedulerError> {
    let mut unique_ids = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        if !unique_ids.contains(task_id) {
            unique_ids.push(task_id.clone());
        }
    }

    for task_id in &unique_ids {
        let task = data
            .tasks
            .iter()
            .find(|task| task.id == *task_id)
            .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
        if !matches!(task.status.as_str(), "scheduled" | "queued" | "retry_wait") {
            return Err(SchedulerError::Io(format!(
                "任务「{}」当前状态 {} 不可暂停",
                task.title, task.status
            )));
        }
    }

    unique_ids
        .iter()
        .map(|task_id| pause_task(data, task_id))
        .collect()
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
    let now = now_rfc3339();
    data.tasks[task_index].queued_at = Some(now.clone());
    data.tasks[task_index].updated_at = now;
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
        let now = now_rfc3339();
        data.tasks[task_index].scheduled_at = None;
        data.tasks[task_index].next_run_at = None;
        data.tasks[task_index].status = "queued".to_string();
        data.tasks[task_index].queued_at = Some(now.clone());
        data.tasks[task_index].updated_at = now;
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
    let now = now_rfc3339();
    data.tasks[task_index].scheduled_at = Some(new_scheduled_at.to_string());
    data.tasks[task_index].next_run_at = Some(new_scheduled_at.to_string());
    data.tasks[task_index].status = "scheduled".to_string();
    data.tasks[task_index].queued_at = Some(now.clone());
    data.tasks[task_index].updated_at = now;
    Ok(data.tasks[task_index].clone())
}

pub fn delete_task_from_data(data: &mut AppData, task_id: &str) -> Result<(), SchedulerError> {
    let index = data
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
    if matches!(
        data.tasks[index].status.as_str(),
        "submitting" | "submitted" | "querying"
    ) {
        return Err(SchedulerError::Io(
            "任务已开始提交、远端排队或生成，当前不可删除".to_string(),
        ));
    }
    data.tasks.remove(index);
    data.task_priorities.remove(task_id);
    Ok(())
}

pub fn delete_tasks_from_data(
    data: &mut AppData,
    task_ids: &[String],
) -> Result<Vec<String>, SchedulerError> {
    let mut unique_ids = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        if !unique_ids.contains(task_id) {
            unique_ids.push(task_id.clone());
        }
    }

    for task_id in &unique_ids {
        let task = data
            .tasks
            .iter()
            .find(|task| task.id == *task_id)
            .ok_or_else(|| SchedulerError::Io(format!("找不到任务：{task_id}")))?;
        if matches!(
            task.status.as_str(),
            "submitting" | "submitted" | "querying"
        ) {
            return Err(SchedulerError::Io(format!(
                "任务「{}」已开始提交、远端排队或生成，当前不可删除",
                task.title
            )));
        }
    }

    for task_id in &unique_ids {
        delete_task_from_data(data, task_id)?;
    }
    Ok(unique_ids)
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
    task.prompt_doc = draft.prompt_doc.clone();
    let now = now_rfc3339();
    task.updated_at = now.clone();
    task.command_preview = new_command_preview;
    // draft 模式：若任务已处于有效执行状态则保留，不强制回退为 draft
    if save_mode == "draft" && task.status != "draft" {
        // 保留 status / scheduled_at / next_run_at，只更新内容字段
    } else {
        task.status = new_status;
        task.scheduled_at = new_scheduled_at.clone();
        task.next_run_at = new_scheduled_at;
        task.queued_at = Some(now);
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

/// 同 `query_task_submit_id_once_with_runner`，但跳过最长等待上限（用于手动查询）。
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
    let Ok(client) = reqwest::blocking::Client::builder()
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(StdDuration::from_secs(120))
        .build()
    else {
        return vec![];
    };
    let mut saved = Vec::new();
    for url in urls {
        let ext = url
            .split('?')
            .next()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("mp4")
            .to_ascii_lowercase();
        let safe_ext = if [
            "mp4", "mov", "webm", "mkv", "png", "jpg", "jpeg", "webp", "gif",
        ]
        .contains(&ext.as_str())
        {
            ext.as_str()
        } else {
            "mp4"
        };
        let url_hash = format!("{:x}", Sha256::digest(url.as_bytes()));
        let id = format!("result_{}", &url_hash[..16]);
        let local_path = results_dir.join(format!("{id}.{safe_ext}"));
        if fs::metadata(&local_path)
            .map(|meta| meta.is_file() && meta.len() > 0)
            .unwrap_or(false)
        {
            saved.push(local_path.to_string_lossy().to_string());
            continue;
        }
        let Ok(mut response) = client.get(url).send() else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let temp_path = results_dir.join(format!("{id}.{}.part", Uuid::new_v4().simple()));
        let copied = fs::File::create(&temp_path)
            .ok()
            .and_then(|mut file| std::io::copy(&mut response, &mut file).ok())
            .is_some();
        if copied && fs::rename(&temp_path, &local_path).is_ok() {
            saved.push(local_path.to_string_lossy().to_string());
        } else {
            let _ = fs::remove_file(&temp_path);
        }
    }
    saved
}

#[derive(Debug, Clone)]
pub struct DueTaskCli {
    pub task_id: String,
    pub args: Vec<String>,
    pub action: DueTaskCliAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DueTaskCliAction {
    Query {
        submit_id: String,
    },
    Submit {
        model_version_override: Option<String>,
    },
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
    // 找下一个待提交任务；普通任务优先，队列空闲时再补试瞬时失败任务。
    let active_kinds = active_remote_queue_kinds(data);
    if let Some(selection) = next_submit_task_id_for_available_queues(data, now, &active_kinds) {
        let Some(task) = data.tasks.iter().find(|task| task.id == selection.task_id) else {
            return Ok(None);
        };
        let model_version_override = match selection.target_queue_kind {
            ModelQueueKind::Standard => Some("seedance2.0".to_string()),
            ModelQueueKind::Fast => Some(FAST_FALLBACK_MODEL_VERSION.to_string()),
        };
        let mut params = task.params.clone();
        if let Some(model_version) = &model_version_override {
            params.model_version = model_version.clone();
        }
        let draft = TaskDraft {
            title: task.title.clone(),
            prompt: task.prompt.clone(),
            image_asset_ids: task.image_asset_ids.clone(),
            audio_asset_ids: task.audio_asset_ids.clone(),
            role_ids: task.role_ids.clone(),
            manual_mention_ids: task.manual_mention_ids.clone(),
            auto_match_roles: task.auto_match_roles,
            params,
            scheduled_at: task.scheduled_at.clone(),
            temp_image_asset_ids: task.temp_image_asset_ids.clone(),
            temp_image_paths: task.temp_image_paths.clone(),
            prompt_doc: task.prompt_doc.clone(),
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
        return Ok(Some(DueTaskCli {
            task_id: task.id.clone(),
            args,
            action: DueTaskCliAction::Submit {
                model_version_override,
            },
        }));
    }

    if data.settings.auto_query_enabled {
        if let Some((task_id, submit_id)) = next_due_initial_current_query_target(data, now) {
            let args = vec![
                "query_result".to_string(),
                format!("--submit_id={submit_id}"),
            ];
            return Ok(Some(DueTaskCli {
                task_id,
                args,
                action: DueTaskCliAction::Query { submit_id },
            }));
        }

        if let Some((task_id, submit_id)) = next_due_execution_query_target(data, now) {
            let args = vec![
                "query_result".to_string(),
                format!("--submit_id={submit_id}"),
            ];
            return Ok(Some(DueTaskCli {
                task_id,
                args,
                action: DueTaskCliAction::Query { submit_id },
            }));
        }

        if let Some((task_id, submit_id)) = next_due_current_query_target(data, now) {
            let args = vec![
                "query_result".to_string(),
                format!("--submit_id={submit_id}"),
            ];
            return Ok(Some(DueTaskCli {
                task_id,
                args,
                action: DueTaskCliAction::Query { submit_id },
            }));
        }
    }

    Ok(None)
}

/// 后台调度循环的空闲短路判定：只有到期动作（含构建出错需处理）才跑重函数。
/// 远端任务仍活跃但尚未到自适应查询时间时返回 false，避免每 30 秒重写整份状态。
pub fn should_process_now(data: &AppData) -> bool {
    !matches!(peek_due_task_cli(data), Ok(None))
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
    let is_fast_fallback_submit = data.tasks[task_index]
        .execution_records
        .iter()
        .find(|record| record.submit_id == submit_id)
        .map(is_fast_execution_record)
        .unwrap_or(false);
    let queried_record_was_active = data.tasks[task_index]
        .execution_records
        .iter()
        .find(|record| record.submit_id == submit_id)
        .map(is_active_execution_record)
        .unwrap_or(false);

    let started_at = now_rfc3339();

    let args = vec![
        "query_result".to_string(),
        format!("--submit_id={submit_id}"),
    ];
    let (stdout, stderr) = match runner(&args) {
        Ok(output) => output,
        Err(message) => {
            let finished = now_rfc3339();
            let duration_secs = calc_duration_seconds(&started_at, &finished);
            let classified = classify_dreamina_error(&message, &data.settings);
            // 已拿到 submit_id 的任务已经被远端受理。本地查询进程异常时没有
            // 远端失败证据，必须继续追踪，不能把远端任务误标为失败。
            let local_query_unavailable = is_current_submit
                && !is_waitable_query_error(&classified.kind, &classified.next_status);
            let waitable = local_query_unavailable
                || is_waitable_query_error(&classified.kind, &classified.next_status);
            let error_kind = if local_query_unavailable {
                "QueryUnavailable".to_string()
            } else {
                format!("{:?}", classified.kind)
            };
            let error_detail = if waitable {
                compact_query_retry_error_detail(&error_kind, &message)
            } else {
                message.clone()
            };
            let attempt_status = if waitable { "retry_wait" } else { "failed" };
            if is_current_submit {
                if waitable {
                    if !matches!(
                        data.tasks[task_index].status.as_str(),
                        "querying" | "submitted"
                    ) {
                        data.tasks[task_index].status = "querying".to_string();
                    }
                    data.tasks[task_index].last_error = error_detail.clone();
                    data.tasks[task_index].auto_query_stopped = false;
                    update_query_backoff(&mut data.tasks[task_index]);
                } else {
                    data.tasks[task_index].status = "failed".to_string();
                    data.tasks[task_index].last_error = message.clone();
                    data.tasks[task_index].finished_at = finished.clone();
                    data.tasks[task_index].queue_info = None;
                    data.tasks[task_index].next_run_at = None;
                }
                data.tasks[task_index].updated_at = finished.clone();
            }
            if let Some(rec) = data.tasks[task_index]
                .execution_records
                .iter_mut()
                .find(|r| r.submit_id == submit_id)
            {
                if waitable {
                    rec.error_kind = error_kind.clone();
                    rec.error_detail = error_detail.clone();
                } else {
                    rec.status = "failed".to_string();
                    rec.error_detail = message.clone();
                    rec.error_kind = error_kind.clone();
                    rec.finished_at = finished.clone();
                }
                rec.query_records.push(TaskAttempt {
                    id: format!("qr_{}", Uuid::new_v4().simple()),
                    started_at,
                    finished_at: finished,
                    status: attempt_status.to_string(),
                    command_preview: args,
                    stdout: String::new(),
                    stderr: String::new(),
                    error_kind,
                    duration_seconds: duration_secs,
                    error_detail,
                });
                sort_query_records_by_time(&mut rec.query_records);
            }
            reconcile_task_with_active_execution(&mut data.tasks[task_index]);
            return Ok(data.tasks[task_index].clone());
        }
    };
    let raw = format!("{stdout}\n{stderr}");
    let parsed = parse_query_output(&raw);
    let has_explicit_remote_progress = query_output_has_explicit_remote_progress(&parsed);
    let historical_tracking_expired = !is_current_submit
        && data.tasks[task_index]
            .execution_records
            .iter()
            .find(|record| record.submit_id == submit_id)
            .is_some_and(|record| execution_tracking_window_expired(record, Utc::now()));
    let status_text = parsed.gen_status.clone().unwrap_or_default().to_lowercase();
    let mut final_status;
    let mut final_result_paths = Vec::new();
    let mut final_result_urls = Vec::new();
    let mut final_error_detail = String::new();
    let mut final_error_kind = String::new();
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
        let failure_message = parsed.fail_reason.unwrap_or_else(|| raw.trim().to_string());
        let classified = classify_dreamina_error(&failure_message, &data.settings);
        if is_waitable_query_error(&classified.kind, &classified.next_status) {
            final_status = "querying".to_string();
            final_error_kind = format!("{:?}", classified.kind);
            final_error_detail =
                compact_query_retry_error_detail(&final_error_kind, &failure_message);
            if is_current_submit {
                data.tasks[task_index].auto_query_stopped = false;
                update_query_backoff(&mut data.tasks[task_index]);
            }
        } else {
            final_status = "failed".to_string();
            final_error_detail = failure_message;
        }
        if final_status == "failed" && is_generation_precheck_failure(&final_error_detail) {
            final_error_kind = "GenerationPrecheck".to_string();
            let previous_failures = data.tasks[task_index]
                .execution_records
                .iter()
                .filter(|record| {
                    record.error_kind == final_error_kind
                        && matches!(record.status.as_str(), "retry_wait" | "failed")
                })
                .count() as u32;
            if is_current_submit && previous_failures < MAX_GENERATION_PRECHECK_RETRIES {
                final_status = "retry_wait".to_string();
                data.tasks[task_index].next_run_at = Some(
                    (Utc::now()
                        + Duration::seconds(data.settings.concurrency_retry_delay_seconds as i64))
                    .to_rfc3339(),
                );
            }
        }
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
            final_error_kind = "RemoteProgressTimeout".to_string();
            final_error_detail = format!(
                "查询超过 {} 分钟仍未返回远端队列信息，疑似提交未真正进入生成队列",
                MAX_NO_REMOTE_QUEUE_INFO_MINUTES
            );
            reset_query_backoff(&mut data.tasks[task_index]);
        } else if !is_manual && is_current_submit && is_past_max_wait(&data.tasks[task_index], now)
        {
            // 检查是否超过最长等待上限（手动查询不触发该限制）
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
    if final_status == "querying" && historical_tracking_expired && !has_explicit_remote_progress {
        final_status = "query_timeout".to_string();
        final_error_kind = "RemoteTrackingExpired".to_string();
        final_error_detail = format!(
            "远端超过 {} 分钟仅返回 querying 且无排队或生成进度，已停止自动追踪并释放本地车道；不代表生成失败。",
            MAX_NO_REMOTE_QUEUE_INFO_MINUTES
        );
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
            data.tasks[task_index].next_run_at = None;
        } else {
            data.tasks[task_index].queue_info = final_queue_info.clone();
            data.tasks[task_index].last_error = final_error_detail.clone();
            if final_status == "retry_wait" && final_error_kind == "GenerationPrecheck" {
                data.tasks[task_index].queued_at = Some(finished.clone());
            }
        }
    } else if (is_fast_fallback_submit || queried_record_was_active) && final_status == "succeeded"
    {
        data.tasks[task_index].status = "succeeded".to_string();
        data.tasks[task_index].submit_id = submit_id.clone();
        data.tasks[task_index].result_paths = final_result_paths.clone();
        data.tasks[task_index].result_urls = final_result_urls.clone();
        data.tasks[task_index].finished_at = finished.clone();
        data.tasks[task_index].queue_info = None;
        data.tasks[task_index].last_error.clear();
        data.tasks[task_index].updated_at = finished.clone();
    }
    let duration_secs = calc_duration_seconds(&started_at, &finished);
    let query_record_id = format!("qr_{}", Uuid::new_v4().simple());
    let query_record = TaskAttempt {
        id: query_record_id.clone(),
        started_at: started_at.clone(),
        finished_at: finished.clone(),
        status: final_status.clone(),
        command_preview: args.clone(),
        stdout: truncate_log(&stdout),
        stderr: truncate_log(&stderr),
        error_kind: final_error_kind.clone(),
        duration_seconds: duration_secs,
        error_detail: final_error_detail.clone(),
    };
    // 兼容旧字段：同时写入顶层 attempts
    if is_current_submit {
        data.tasks[task_index].attempts.push(TaskAttempt {
            id: query_record_id,
            started_at,
            finished_at: finished.clone(),
            status: final_status.clone(),
            command_preview: args,
            stdout: truncate_log(&stdout),
            stderr: truncate_log(&stderr),
            error_kind: final_error_kind.clone(),
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
        if !query_record_exists(&rec.query_records, &query_record) {
            rec.query_records.push(query_record);
        }
        sort_query_records_by_time(&mut rec.query_records);
        rec.status = final_status.clone();
        if final_status == "succeeded" {
            rec.result_paths = final_result_paths;
            rec.result_urls = final_result_urls;
            rec.finished_at = finished;
            rec.error_kind.clear();
            rec.error_detail.clear();
        } else if matches!(
            final_status.as_str(),
            "failed" | "query_timeout" | "retry_wait"
        ) {
            rec.error_detail = final_error_detail;
            rec.error_kind = final_error_kind;
            rec.finished_at = finished;
        } else {
            rec.error_detail = final_error_detail;
            rec.error_kind = final_error_kind;
            rec.finished_at.clear();
        }
    }
    if data.tasks[task_index].status != "succeeded" {
        reconcile_task_with_active_execution(&mut data.tasks[task_index]);
    }
    apply_planned_submit_completion(&mut data.tasks[task_index]);
    Ok(data.tasks[task_index].clone())
}

pub fn submit_task_once(
    data: &mut AppData,
    task_id: &str,
) -> Result<ScheduledTask, SchedulerError> {
    submit_task_once_with_runner(data, task_id, None, |args| run_dreamina_command(args))
}

pub fn submit_task_once_with_runner<F>(
    data: &mut AppData,
    task_id: &str,
    model_version_override: Option<&str>,
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
    let mut params = task.params.clone();
    if let Some(mv) = model_version_override {
        params.model_version = mv.to_string();
    }
    let draft = TaskDraft {
        title: task.title.clone(),
        prompt: task.prompt.clone(),
        image_asset_ids: task.image_asset_ids.clone(),
        audio_asset_ids: task.audio_asset_ids.clone(),
        role_ids: task.role_ids.clone(),
        manual_mention_ids: task.manual_mention_ids.clone(),
        auto_match_roles: task.auto_match_roles,
        params,
        scheduled_at: task.scheduled_at.clone(),
        temp_image_asset_ids: task.temp_image_asset_ids.clone(),
        temp_image_paths: task.temp_image_paths.clone(),
        prompt_doc: task.prompt_doc.clone(),
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
    let mut snapshot_task = data.tasks[task_index].clone();
    snapshot_task.params = draft.params.clone();
    let preflight_detail = build_submit_preflight_detail(&snapshot_task, &resolved, &data.assets);
    let log_task = snapshot_task.clone();
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
                params: draft.params.clone(),
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
            reconcile_task_with_active_execution(&mut data.tasks[task_index]);
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
        params: draft.params.clone(),
        temp_image_asset_ids: task.temp_image_asset_ids.clone(),
    };
    let execution_finished_at = if matches!(final_status.as_str(), "querying" | "submitted") {
        String::new()
    } else {
        finished.clone()
    };
    upsert_submit_execution_record(
        &mut data.tasks[task_index],
        TaskExecutionRecord {
            id: format!("exec_{}", Uuid::new_v4().simple()),
            submit_id: submit_id_now,
            status: final_status,
            started_at,
            finished_at: execution_finished_at,
            input_snapshot,
            command_preview: args,
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind,
            error_detail: display_error_detail,
        },
    );
    if data.tasks[task_index].status != "succeeded" {
        reconcile_task_with_active_execution(&mut data.tasks[task_index]);
    }
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

    for role in roles.iter().filter(|role| !role.disabled) {
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

fn is_generation_precheck_failure(message: &str) -> bool {
    message
        .to_ascii_lowercase()
        .contains("pre-tns check did not pass")
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

fn command_output_to_result(output: std::process::Output) -> Result<(String, String), String> {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "未知".to_string());
        let detail = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(if detail.is_empty() {
            format!("dreamina CLI 退出码 {code}")
        } else {
            format!("dreamina CLI 退出码 {code}: {detail}")
        });
    }
    if stdout.trim().is_empty() && !stderr.trim().is_empty() {
        let classified = classify_dreamina_error(&stderr, &SchedulerSettings::default());
        if classified.kind != DreaminaErrorKind::Generic {
            return Err(stderr.trim().to_string());
        }
    }
    Ok((stdout, stderr))
}

fn run_command_with_timeout(
    program: &str,
    args: &[String],
    timeout: StdDuration,
) -> Result<std::process::Output, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取 dreamina CLI 标准输出".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取 dreamina CLI 错误输出".to_string())?;
    let stdout_reader = std::thread::spawn(move || {
        let mut reader = stdout;
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut reader = stderr;
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    });
    let deadline = std::time::Instant::now() + timeout;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!(
                    "dreamina CLI timeout：执行超时（{} 秒），已终止本次操作",
                    timeout.as_secs_f64()
                ));
            }
            Ok(None) => std::thread::sleep(StdDuration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error.to_string());
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "读取 dreamina CLI 标准输出失败".to_string())?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "读取 dreamina CLI 错误输出失败".to_string())?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn run_dreamina_command(args: &[String]) -> Result<(String, String), String> {
    let cli = check_dreamina_cli_status();
    if !cli.available {
        return Err(cli.message);
    }
    let output = run_command_with_timeout(
        &cli.path,
        args,
        StdDuration::from_secs(DREAMINA_CLI_TIMEOUT_SECS),
    )?;
    command_output_to_result(output)
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
    token: String,
}

impl Drop for StoreQueueLockGuard {
    fn drop(&mut self) {
        let expected_token = format!("token={}", self.token);
        let owns_lock = fs::read_to_string(&self.path)
            .map(|content| content.lines().any(|line| line == expected_token))
            .unwrap_or(false);
        if owns_lock {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// queue.lock 陈旧阈值：超过此时长仍存在的锁视为持有者非正常退出遗留，可回收。
/// 取 600s，明显大于单步 CLI 合法最大持锁（IMAGE_SUBMIT_TIMEOUT_SECS=300），避免误抢正在干活的锁。
const QUEUE_LOCK_STALE_SECS: i64 = 600;

/// 根据锁文件内容判断是否陈旧：created_at 距 now 超过阈值，或无法解析 created_at（旧格式/损坏），
/// 均视为陈旧（可被回收）。纯函数，便于测试。
fn lock_is_stale(content: &str, now: DateTime<Utc>, stale_secs: i64) -> bool {
    for line in content.lines() {
        if let Some(ts) = line.strip_prefix("created_at=") {
            if let Ok(created) = DateTime::parse_from_rfc3339(ts.trim()) {
                return now
                    .signed_duration_since(created.with_timezone(&Utc))
                    .num_seconds()
                    >= stale_secs;
            }
        }
    }
    true
}

fn create_store_queue_lock(lock_path: &Path, origin: &str) -> Option<StoreQueueLockGuard> {
    let token = Uuid::new_v4().simple().to_string();
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(mut file) => {
            let written = writeln!(file, "token={token}")
                .and_then(|_| writeln!(file, "origin={origin}"))
                .and_then(|_| writeln!(file, "pid={}", std::process::id()))
                .and_then(|_| writeln!(file, "created_at={}", now_rfc3339()))
                .is_ok();
            if !written {
                drop(file);
                let _ = fs::remove_file(lock_path);
                return None;
            }
            Some(StoreQueueLockGuard {
                path: lock_path.to_path_buf(),
                token,
            })
        }
        Err(_) => None,
    }
}

fn try_acquire_store_queue_lock(store: &AppStore, origin: &str) -> Option<StoreQueueLockGuard> {
    let lock_path = store.root_dir.join("queue.lock");
    let parent = lock_path.parent()?;
    if fs::create_dir_all(parent).is_err() {
        return None;
    }
    if let Some(guard) = create_store_queue_lock(&lock_path, origin) {
        return Some(guard);
    }
    // 已存在：若是陈旧锁（持有者崩溃/强退/被 kill 遗留），回收后重试一次，避免永久卡死调度。
    if let Ok(content) = fs::read_to_string(&lock_path) {
        let has_parseable_created_at = content.lines().any(|line| {
            line.strip_prefix("created_at=")
                .and_then(|value| DateTime::parse_from_rfc3339(value.trim()).ok())
                .is_some()
        });
        let stale = if has_parseable_created_at {
            lock_is_stale(&content, Utc::now(), QUEUE_LOCK_STALE_SECS)
        } else {
            fs::metadata(&lock_path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .map(|age| age >= StdDuration::from_secs(QUEUE_LOCK_STALE_SECS as u64))
                .unwrap_or(false)
        };
        if stale {
            let _ = fs::remove_file(&lock_path);
            return create_store_queue_lock(&lock_path, origin);
        }
    }
    None
}

/// 每条执行记录保留的 query_records（自动轮询历史）上限。
const MAX_QUERY_RECORDS_PER_EXECUTION: usize = 500;
/// 每个任务保留的 attempts（尝试历史）上限。
const MAX_ATTEMPTS_PER_TASK: usize = 50;

fn query_attempt_semantic_key(attempt: &TaskAttempt) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        attempt.started_at,
        attempt.finished_at,
        attempt.status,
        attempt.command_preview.join("\u{1e}"),
        attempt.stdout,
        attempt.stderr,
        attempt.error_detail,
    )
}

fn query_record_exists(records: &[TaskAttempt], candidate: &TaskAttempt) -> bool {
    let candidate_key = query_attempt_semantic_key(candidate);
    records.iter().any(|record| {
        record.id == candidate.id || query_attempt_semantic_key(record) == candidate_key
    })
}

fn dedupe_query_records(records: &mut Vec<TaskAttempt>) {
    let mut seen_ids = HashSet::new();
    let mut seen_keys = HashSet::new();
    records.retain(|attempt| {
        let key = query_attempt_semantic_key(attempt);
        seen_ids.insert(attempt.id.clone()) && seen_keys.insert(key)
    });
}

fn dedupe_all_query_records(data: &mut AppData) {
    for task in &mut data.tasks {
        for rec in &mut task.execution_records {
            dedupe_query_records(&mut rec.query_records);
        }
    }
}

/// Sort a task's execution record query_records by started_at (chronological).
fn sort_query_records_by_time(records: &mut Vec<TaskAttempt>) {
    records.sort_by(|a, b| a.started_at.cmp(&b.started_at));
}

fn sort_all_query_records(data: &mut AppData) {
    for task in &mut data.tasks {
        for rec in &mut task.execution_records {
            sort_query_records_by_time(&mut rec.query_records);
        }
    }
}

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

const MAX_FRONTEND_LOG_ENTRIES: usize = 200;

fn compact_attempt_for_frontend(attempt: &mut TaskAttempt) {
    let queue_info = parse_query_output(&attempt.stdout).queue_info;
    attempt.stdout = queue_info
        .and_then(|queue| serde_json::to_string(&serde_json::json!({ "queue_info": queue })).ok())
        .unwrap_or_default();
    attempt.stderr.clear();
    attempt.command_preview.clear();
}

/// IPC 只传 UI 实际使用的数据。磁盘中的完整历史保持不变。
fn frontend_app_state(mut data: AppData) -> AppData {
    if data.logs.len() > MAX_FRONTEND_LOG_ENTRIES {
        let remove = data.logs.len() - MAX_FRONTEND_LOG_ENTRIES;
        data.logs.drain(0..remove);
    }
    for log in &mut data.logs {
        log.detail.clear();
        log.error_detail = None;
        log.raw_output = None;
        log.stdout = None;
        log.stderr = None;
    }
    for task in &mut data.tasks {
        for attempt in &mut task.attempts {
            compact_attempt_for_frontend(attempt);
        }
        for record in &mut task.execution_records {
            for attempt in &mut record.query_records {
                compact_attempt_for_frontend(attempt);
            }
        }
    }
    data
}

/// Trim log entries older than `log_retention_days` (default 3 days).
/// Logs are chronologically ordered (always appended), so binary search is used.
fn apply_log_retention(data: &mut AppData) {
    let max_days = data.settings.log_retention_days.max(1);
    let cutoff = Utc::now() - chrono::Duration::days(max_days as i64);
    let cutoff_str = cutoff.to_rfc3339();

    let idx = data.logs.partition_point(|e| e.timestamp < cutoff_str);
    if idx > 0 {
        data.logs.drain(0..idx);
    }
    // Hard cap: prevent unbounded growth in edge cases
    const MAX_LOG_ENTRIES: usize = 10000;
    if data.logs.len() > MAX_LOG_ENTRIES {
        let drain = data.logs.len() - MAX_LOG_ENTRIES;
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

    let Some(process_guard) = try_begin_process_queue() else {
        let _ = store.mutate(|data| {
            record_scheduler_tick(data, origin, "skipped_busy");
            Ok(())
        });
        return Ok(None);
    };

    let Some(store_lock) = try_acquire_store_queue_lock(store, origin) else {
        let _ = store.mutate(|data| {
            record_scheduler_tick(data, origin, "skipped_busy");
            Ok(())
        });
        return Ok(None);
    };

    // 锁外：探测下一步应执行的 CLI 调用
    let due = {
        let data = store.try_snapshot().map_err(|error| error.to_string())?;
        peek_due_task_cli(&data)
    };
    let DueTaskCli {
        task_id,
        args,
        action,
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
            // scheduled → queued 迁移（仅普通 submit 路径需要）
            if matches!(action, DueTaskCliAction::Submit { .. }) {
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
            let task = match &action {
                DueTaskCliAction::Query { submit_id } => {
                    query_task_submit_id_once_with_runner(data, &task_id, submit_id, |_| {
                        cli_result.clone()
                    })?
                }
                DueTaskCliAction::Submit {
                    model_version_override,
                } => submit_task_once_with_runner(
                    data,
                    &task_id,
                    model_version_override.as_deref(),
                    |_| cli_result.clone(),
                )?,
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
    drop(store_lock);
    drop(process_guard);
    Ok(Some(task))
}

fn start_background_scheduler(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        let store = app_handle.state::<AppStore>();
        let waker = app_handle.state::<SchedulerWaker>();

        // 单次快照供「空闲短路」与「等待时长」共用，避免一个 tick 内重复读盘。
        let snapshot = match store.try_snapshot() {
            Ok(snapshot) => snapshot,
            Err(_) => {
                waker.wait(StdDuration::from_secs(SCHEDULER_TICK_INTERVAL_SECS));
                continue;
            }
        };

        // 完全空闲时跳过重函数：不写 started/no_due_task 噪音日志、不整份序列化落盘。
        // 若这次 tick 刚让某任务到达终态（成功/失败），说明队列腾出来了——不进入等待，
        // 立刻再跑一轮把后续任务马上提交，避免白白浪费一个查询间隔。
        let mut task_just_finished = false;
        if should_process_now(&snapshot) {
            match process_queue_for_store_blocking(&store, "background") {
                Ok(Some(task)) if task.status == "succeeded" || task.status == "failed" => {
                    task_just_finished = true;
                }
                Ok(_) => {}
                Err(error) => {
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
        }

        // 刚有任务完成 → 立即进入下一轮提交后续任务，不等待。
        if task_just_finished {
            continue;
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
            commands::queue_tasks_with_model_strategy_command,
            commands::queue_tasks_with_batch_schedule_command,
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
            commands::set_lane_enabled_command,
            commands::set_task_queue_priority_command,
            commands::pause_task_command,
            commands::pause_tasks_command,
            commands::resume_task_command,
            commands::reschedule_task_command,
            commands::open_result_dir_command,
            commands::open_external_url_command,
            commands::download_result_url_command,
            commands::install_dreamina_cli_command,
            commands::login_dreamina_cli_command,
            commands::delete_task_command,
            commands::delete_tasks_command,
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

    #[tauri::command(async)]
    pub fn get_app_state(store: State<'_, AppStore>) -> Result<AppData, String> {
        store
            .try_snapshot()
            .map(frontend_app_state)
            .map_err(|error| error.to_string())
    }

    /// 廉价变更签名（仅读取 SQLite revision）。前端轮询比对，未变则跳过 `get_app_state`。
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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
                touch_asset_last_used(data, &task.image_asset_ids, &task.audio_asset_ids);
                Ok(task)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command(async)]
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
                touch_asset_last_used(data, &task.image_asset_ids, &task.audio_asset_ids);
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
        let store_lock = try_acquire_store_queue_lock(&store, "manual_submit")
            .ok_or_else(|| "调度器正在执行提交或查询，请稍后再试".to_string())?;
        // 锁外：从快照构建 CLI 参数
        let args = {
            let data = store.try_snapshot().map_err(|error| error.to_string())?;
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
                prompt_doc: task.prompt_doc.clone(),
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
                    submit_task_once_with_runner(data, &task_id, None, |_args| cli_result.clone())?;
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
        drop(store_lock);
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
            let data = store.try_snapshot().map_err(|error| error.to_string())?;
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
        let store_lock = try_acquire_store_queue_lock(&store, "manual_query")
            .ok_or_else(|| "调度器正在执行提交或查询，请稍后再刷新".to_string())?;
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
                    if t.submit_id == submit_id && t.auto_query_stopped {
                        // 仅在自动查询已停止时重置退避，避免干扰正在运行的自适应间隔
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
        drop(store_lock);
        Ok(task)
    }

    #[tauri::command(async)]
    pub fn process_queue_command(
        store: State<'_, AppStore>,
    ) -> Result<Option<ScheduledTask>, String> {
        process_queue_for_store_blocking(&store, "manual")
    }

    #[tauri::command(async)]
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
        let settings = store
            .try_snapshot()
            .map_err(|error| error.to_string())?
            .settings;
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
        let settings = store
            .try_snapshot()
            .map_err(|error| error.to_string())?
            .settings;
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
            let data = store.try_snapshot().map_err(|error| error.to_string())?;
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
        let snapshot = store.try_snapshot().map_err(|error| error.to_string())?;
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
            .try_snapshot()
            .map_err(|error| error.to_string())?
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
            .try_snapshot()
            .map_err(|error| error.to_string())?
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

    #[tauri::command(async)]
    pub fn download_imagegen_image_command(
        store: State<'_, AppStore>,
        history_id: String,
    ) -> Result<String, String> {
        let item = store
            .try_snapshot()
            .map_err(|error| error.to_string())?
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

    #[tauri::command(async)]
    pub fn copy_imagegen_image_command(
        store: State<'_, AppStore>,
        history_id: String,
    ) -> Result<(), String> {
        let item = store
            .try_snapshot()
            .map_err(|error| error.to_string())?
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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
                    log_retention_days: input.log_retention_days.unwrap_or(3),
                    mac_install_command: input.mac_install_command,
                    windows_install_command: input.windows_install_command,
                    ai_model_configs: input.ai_model_configs,
                    active_ai_model_id: input.active_ai_model_id,
                    prevent_sleep: input.prevent_sleep,
                    standard_lane_enabled: input.standard_lane_enabled,
                    fast_lane_enabled: input.fast_lane_enabled,
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

    #[tauri::command(async)]
    pub fn set_lane_enabled_command(
        store: State<'_, AppStore>,
        waker: State<'_, SchedulerWaker>,
        queue_kind: String,
        enabled: bool,
    ) -> Result<SchedulerSettings, String> {
        let kind = parse_lane_kind(&queue_kind).map_err(|error| error.to_string())?;
        let settings = store
            .mutate(|data| {
                set_lane_enabled(data, kind, enabled)?;
                Ok(data.settings.clone())
            })
            .map_err(|error| error.to_string())?;
        waker.notify();
        Ok(settings)
    }

    #[tauri::command(async)]
    pub fn set_task_queue_priority_command(
        store: State<'_, AppStore>,
        waker: State<'_, SchedulerWaker>,
        task_id: String,
        priority: u8,
    ) -> Result<u8, String> {
        let priority = store
            .mutate(|data| set_task_queue_priority(data, &task_id, priority))
            .map_err(|error| error.to_string())?;
        waker.notify();
        Ok(priority)
    }

    #[tauri::command(async)]
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

    #[tauri::command(async)]
    pub fn pause_tasks_command(
        store: State<'_, AppStore>,
        task_ids: Vec<String>,
    ) -> Result<Vec<ScheduledTask>, String> {
        store
            .mutate(|data| {
                let tasks = pause_tasks(data, &task_ids)?;
                for task in &tasks {
                    append_task_log(
                        data,
                        task,
                        LogEntryDraft {
                            level: LogLevel::Info,
                            source: LogSource::Scheduler,
                            category: "task".to_string(),
                            event_type: "pause".to_string(),
                            message: format!("批量暂停任务：{}", task.title),
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
                }
                Ok(tasks)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command(async)]
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

    #[tauri::command(async)]
    pub fn reschedule_task_command(
        store: State<'_, AppStore>,
        waker: State<'_, SchedulerWaker>,
        task_id: String,
        new_scheduled_at: String,
    ) -> Result<ScheduledTask, String> {
        let task = store
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
            .map_err(|error| error.to_string())?;
        waker.notify();
        Ok(task)
    }

    #[tauri::command(async)]
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

    #[tauri::command(async)]
    pub fn queue_tasks_with_model_strategy_command(
        store: State<'_, AppStore>,
        task_ids: Vec<String>,
        new_scheduled_at: String,
        planned_submit_count: u32,
        alternate_fast_model: bool,
    ) -> Result<Vec<ScheduledTask>, String> {
        store
            .mutate(|data| {
                let tasks = queue_tasks_with_model_strategy(
                    data,
                    &task_ids,
                    &new_scheduled_at,
                    planned_submit_count,
                    alternate_fast_model,
                )?;
                for task in &tasks {
                    append_task_log(
                        data,
                        task,
                        LogEntryDraft {
                            level: LogLevel::Info,
                            source: LogSource::Scheduler,
                            category: "task".to_string(),
                            event_type: "queue_model_strategy".to_string(),
                            message: format!(
                                "排队策略：{} -> {}{}",
                                task.title,
                                if task.status == "scheduled" {
                                    "预定中"
                                } else {
                                    "排队中"
                                },
                                if alternate_fast_model {
                                    format!(" · {}", task.params.model_version)
                                } else {
                                    String::new()
                                }
                            ),
                            detail: if new_scheduled_at.trim().is_empty() {
                                String::new()
                            } else {
                                format!("scheduled_at={new_scheduled_at}")
                            },
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
                Ok(tasks)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command(async)]
    pub fn queue_tasks_with_batch_schedule_command(
        store: State<'_, AppStore>,
        waker: State<'_, SchedulerWaker>,
        plan: Vec<BatchQueuePlanItem>,
        planned_submit_count: u32,
        alternate_fast_model: bool,
    ) -> Result<Vec<ScheduledTask>, String> {
        let tasks = store
            .mutate(|data| {
                let tasks = queue_tasks_with_batch_schedule(
                    data,
                    &plan,
                    planned_submit_count,
                    alternate_fast_model,
                )?;
                for task in &tasks {
                    append_task_log(
                        data,
                        task,
                        LogEntryDraft {
                            level: LogLevel::Info,
                            source: LogSource::Scheduler,
                            category: "task".to_string(),
                            event_type: "batch_queue_plan".to_string(),
                            message: format!(
                                "批量排队：{} -> {}{}",
                                task.title,
                                if task.status == "scheduled" {
                                    "预定中"
                                } else {
                                    "排队中"
                                },
                                if alternate_fast_model {
                                    format!(" · {}", task.params.model_version)
                                } else {
                                    String::new()
                                }
                            ),
                            detail: task
                                .scheduled_at
                                .as_ref()
                                .map(|scheduled_at| format!("scheduled_at={scheduled_at}"))
                                .unwrap_or_default(),
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
                Ok(tasks)
            })
            .map_err(|error| error.to_string())?;
        waker.notify();
        Ok(tasks)
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
    pub fn open_external_url_command(url: String) -> Result<(), String> {
        let parsed = reqwest::Url::parse(&url).map_err(|_| "结果链接无效".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("只允许打开 HTTP 或 HTTPS 结果链接".to_string());
        }
        #[cfg(target_os = "macos")]
        let status = Command::new("open").arg(parsed.as_str()).status();
        #[cfg(target_os = "windows")]
        let status = Command::new("cmd")
            .args(["/C", "start", "", parsed.as_str()])
            .status();
        #[cfg(target_os = "linux")]
        let status = Command::new("xdg-open").arg(parsed.as_str()).status();
        status
            .map_err(|error| format!("打开结果链接失败：{error}"))?
            .success()
            .then_some(())
            .ok_or_else(|| "打开结果链接失败".to_string())
    }

    #[tauri::command]
    pub async fn download_result_url_command(
        store: State<'_, AppStore>,
        url: String,
    ) -> Result<String, String> {
        let results_dir = store.assets_dir().join("results");
        tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
            download_result_urls(&[url], &results_dir)
                .into_iter()
                .next()
                .ok_or_else(|| "下载失败或远端未返回有效文件".to_string())
        })
        .await
        .map_err(|e| format!("任务错误：{e}"))?
    }

    #[tauri::command]
    pub async fn install_dreamina_cli_command(
        store: State<'_, AppStore>,
    ) -> Result<String, String> {
        let settings = store
            .try_snapshot()
            .map_err(|error| error.to_string())?
            .settings;
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

    #[tauri::command(async)]
    pub fn delete_task_command(store: State<'_, AppStore>, task_id: String) -> Result<(), String> {
        store
            .mutate(|data| {
                delete_task_from_data(data, &task_id)?;
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

    #[tauri::command(async)]
    pub fn delete_tasks_command(
        store: State<'_, AppStore>,
        task_ids: Vec<String>,
    ) -> Result<Vec<String>, String> {
        store
            .mutate(|data| {
                let deleted_ids = delete_tasks_from_data(data, &task_ids)?;
                for task_id in &deleted_ids {
                    append_log(
                        data,
                        LogEntryDraft {
                            level: LogLevel::Info,
                            source: LogSource::Scheduler,
                            category: "task".to_string(),
                            event_type: "delete".to_string(),
                            message: format!("批量删除任务：{task_id}"),
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
                }
                Ok(deleted_ids)
            })
            .map_err(|e| e.to_string())
    }

    #[tauri::command(async)]
    pub fn delete_execution_record_command(
        store: State<'_, AppStore>,
        task_id: String,
        execution_id: String,
    ) -> Result<ScheduledTask, String> {
        store
            .mutate(|data| delete_execution_record_from_data(data, &task_id, &execution_id))
            .map_err(|e| e.to_string())
    }

    #[tauri::command(async)]
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

    #[tauri::command(async)]
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
                touch_asset_last_used(data, &task.image_asset_ids, &task.audio_asset_ids);
                Ok(task)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command(async)]
    pub fn sync_keep_awake_command(
        store: State<'_, AppStore>,
        guard: State<'_, keep_awake::KeepAwakeGuard>,
    ) -> bool {
        let Ok(data) = store.try_snapshot() else {
            guard.release();
            return false;
        };
        if data.settings.prevent_sleep && needs_keep_awake(&data.tasks) {
            guard.acquire();
        } else {
            guard.release();
        }
        guard.is_active()
    }

    #[tauri::command(async)]
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

    #[test]
    fn mcp_queued_task_update_parses_compatible_task_id_and_image_aliases() {
        let parsed = parse_mcp_queued_task_update_input(serde_json::json!({
            "taskId": "task-1",
            "prompt": "替换后的提示词",
            "images": { "path": "file:///tmp/scene%2001.png" },
        }))
        .expect("兼容参数应被解析");
        assert_eq!(parsed.task_id, "task-1");
        assert_eq!(parsed.task.image_paths, vec!["/tmp/scene 01.png"]);
    }

    #[test]
    fn only_pristine_queued_task_can_be_updated_in_place() {
        let draft = TaskDraft {
            title: "未执行任务".to_string(),
            prompt: "提示词".to_string(),
            image_asset_ids: vec![],
            audio_asset_ids: vec![],
            role_ids: vec![],
            manual_mention_ids: vec![],
            auto_match_roles: false,
            params: VideoParams::default(),
            scheduled_at: None,
            temp_image_asset_ids: vec![],
            temp_image_paths: vec![],
            prompt_doc: None,
        };
        let mut task = ScheduledTask::from(draft);
        assert!(is_never_executed_queued_task(&task));
        task.attempts.push(TaskAttempt {
            id: "attempt-1".to_string(),
            started_at: String::new(),
            finished_at: String::new(),
            status: "failed".to_string(),
            command_preview: vec![],
            stdout: String::new(),
            stderr: String::new(),
            error_kind: String::new(),
            duration_seconds: 0.0,
            error_detail: String::new(),
        });
        assert!(!is_never_executed_queued_task(&task));
    }

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
            queued_at: None,
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
            prompt_doc: None,
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

    fn make_pre_tns_querying_task(submit_id: &str) -> ScheduledTask {
        let mut task = make_task_with_execution_records();
        task.status = "querying".to_string();
        task.submit_id = submit_id.to_string();
        task.planned_submit_count = 1;
        task.result_paths.clear();
        task.result_urls.clear();
        task.execution_records = vec![TaskExecutionRecord {
            id: format!("rec-{submit_id}"),
            submit_id: submit_id.to_string(),
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
        }];
        task
    }

    fn append_querying_execution_record(task: &mut ScheduledTask, submit_id: &str) {
        task.status = "querying".to_string();
        task.submit_id = submit_id.to_string();
        task.finished_at.clear();
        task.last_error.clear();
        task.execution_records.push(TaskExecutionRecord {
            id: format!("rec-{submit_id}"),
            submit_id: submit_id.to_string(),
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
    }

    fn query_pre_tns_failure(data: &mut AppData, submit_id: &str) -> ScheduledTask {
        query_task_submit_id_once_with_runner(data, "task-1", submit_id, |_| {
            Ok((
                r#"{"gen_status":"FAILED","fail_reason":"generation failed: pre-TNS check did not pass"}"#.to_string(),
                String::new(),
            ))
        })
        .expect("pre-TNS 查询失败应写回任务状态")
    }

    #[test]
    fn pre_tns_generation_failure_retries_twice_then_stays_failed() {
        let mut task = make_pre_tns_querying_task("sub-1");
        task.queued_at = Some("2026-04-30T00:00:00Z".to_string());
        let mut data = AppData {
            settings: SchedulerSettings {
                concurrency_retry_delay_seconds: 0,
                ..SchedulerSettings::default()
            },
            tasks: vec![task],
            ..AppData::default()
        };

        let first = query_pre_tns_failure(&mut data, "sub-1");
        assert_eq!(first.status, "retry_wait");
        assert_ne!(
            first.queued_at.as_deref(),
            Some("2026-04-30T00:00:00Z"),
            "审核失败重新排队时应重置入队时间"
        );
        assert_eq!(
            first.execution_records.last().unwrap().error_kind,
            "GenerationPrecheck"
        );
        assert_eq!(
            next_due_submit_task_id(&data, Utc::now()).as_deref(),
            Some("task-1")
        );

        append_querying_execution_record(&mut data.tasks[0], "sub-2");
        let second = query_pre_tns_failure(&mut data, "sub-2");
        assert_eq!(second.status, "retry_wait");
        assert_eq!(
            next_due_submit_task_id(&data, Utc::now()).as_deref(),
            Some("task-1")
        );

        append_querying_execution_record(&mut data.tasks[0], "sub-3");
        let third = query_pre_tns_failure(&mut data, "sub-3");
        assert_eq!(third.status, "failed");
        assert_eq!(
            third
                .execution_records
                .iter()
                .filter(|record| record.error_kind == "GenerationPrecheck"
                    && matches!(record.status.as_str(), "retry_wait" | "failed"))
                .count(),
            3
        );
        assert_eq!(next_due_submit_task_id(&data, Utc::now()), None);
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
    fn concurrency_retry_delay_default_is_five_minutes() {
        assert_eq!(
            SchedulerSettings::default().concurrency_retry_delay_seconds,
            300
        );
    }

    #[test]
    fn normalize_loaded_app_data_migrates_legacy_hot_concurrency_retry_delay() {
        let mut data = AppData::default();
        data.settings.concurrency_retry_delay_seconds = 30;

        normalize_loaded_app_data(&mut data);

        assert_eq!(data.settings.concurrency_retry_delay_seconds, 300);
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
            queued_at: None,
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
            prompt_doc: None,
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

    #[test]
    fn app_store_initializes_sqlite_without_legacy_lock_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let _store = AppStore::load(temp.path().to_path_buf());
        assert!(temp.path().join("state.sqlite3").exists());
        assert!(!temp.path().join("state.write.lock").exists());
    }

    // ── lock_is_stale（陈旧锁回收）─────────────────────────────────────────

    #[test]
    fn lock_is_stale_when_created_at_exceeds_threshold() {
        let now = DateTime::parse_from_rfc3339("2026-06-24T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // 11 分钟前创建，阈值 600s → 陈旧
        let content = "origin=background\npid=584\ncreated_at=2026-06-24T23:19:00Z\n";
        assert!(lock_is_stale(content, now, 600));
    }

    #[test]
    fn lock_is_fresh_when_created_recently() {
        let now = DateTime::parse_from_rfc3339("2026-06-24T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // 30 秒前创建 → 未陈旧
        let content = "origin=background\npid=584\ncreated_at=2026-06-24T23:29:30Z\n";
        assert!(!lock_is_stale(content, now, 600));
    }

    #[test]
    fn lock_is_stale_when_created_at_unparseable() {
        let now = DateTime::parse_from_rfc3339("2026-06-24T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // 无 created_at（旧格式/损坏）→ 保守视为陈旧，可被回收
        assert!(lock_is_stale("origin=background\npid=1\n", now, 600));
    }

    #[test]
    fn try_acquire_reclaims_stale_lock_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        let lock_path = temp.path().join("queue.lock");
        // 写入陈旧锁（created_at 远早于现在，模拟崩溃/被 kill 遗留）
        std::fs::write(
            &lock_path,
            "origin=old\npid=99999\ncreated_at=2000-01-01T00:00:00Z\n",
        )
        .expect("write stale lock");
        let guard = try_acquire_store_queue_lock(&store, "background");
        assert!(guard.is_some(), "应回收陈旧锁并获取成功");
        drop(guard);
        assert!(!lock_path.exists(), "释放后锁文件应被删除");
    }

    #[test]
    fn try_acquire_blocked_by_fresh_lock() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        let g1 = try_acquire_store_queue_lock(&store, "a");
        assert!(g1.is_some(), "首次应获取成功");
        let g2 = try_acquire_store_queue_lock(&store, "b");
        assert!(g2.is_none(), "新鲜锁应阻塞第二次获取");
    }

    #[test]
    fn try_acquire_does_not_steal_a_newly_created_empty_lock() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        let lock_path = temp.path().join("queue.lock");
        std::fs::File::create(&lock_path).expect("create fresh partial lock");

        let guard = try_acquire_store_queue_lock(&store, "contender");

        assert!(guard.is_none(), "刚创建的半写锁不能被当成陈旧锁抢走");
        assert!(lock_path.exists());
    }

    // process_queue_for_store_blocking 用进程级全局原子 PROCESS_QUEUE_RUNNING，
    // 这些端到端测试串行执行以避免相互干扰。
    static PQ_E2E_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn process_queue_recovers_from_stale_lock_end_to_end() {
        let _serial = PQ_E2E_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        // 模拟崩溃/被 kill 的前一进程遗留的陈旧锁
        let lock_path = temp.path().join("queue.lock");
        std::fs::write(
            &lock_path,
            "origin=old\npid=99999\ncreated_at=2000-01-01T00:00:00Z\n",
        )
        .expect("write stale lock");
        // 无到期任务：走 no_due_task 路径，不触发 CLI；但必须先经过真实的锁获取
        let result = process_queue_for_store_blocking(&store, "test");
        assert!(result.is_ok(), "应正常完成（回收陈旧锁后处理）");
        assert!(
            !lock_path.exists(),
            "陈旧锁应被回收并在处理后释放，不残留——这正是导致调度卡死的故障路径"
        );
    }

    #[test]
    fn process_queue_skips_under_fresh_foreign_lock_end_to_end() {
        let _serial = PQ_E2E_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        let lock_path = temp.path().join("queue.lock");
        std::fs::write(
            &lock_path,
            format!("origin=other\npid=1\ncreated_at={}\n", now_rfc3339()),
        )
        .expect("write fresh lock");
        let result = process_queue_for_store_blocking(&store, "test");
        assert!(result.is_ok());
        assert!(
            lock_path.exists(),
            "新鲜的外部锁不应被偷走（避免误抢正在干活的进程）"
        );
    }

    // ── cap_execution_history（历史裁剪）───────────────────────────────────

    fn cap_attempt(id: usize) -> TaskAttempt {
        TaskAttempt {
            id: format!("att-{id}"),
            started_at: format!("2026-05-01T00:{:02}:00Z", id % 60),
            finished_at: format!("2026-05-01T00:{:02}:01Z", id % 60),
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
    fn frontend_state_projection_keeps_history_but_drops_heavy_transport_fields() {
        let large_prompt = "长提示词".repeat(2_000);
        let stdout = serde_json::json!({
            "submit_id": "submit-1",
            "gen_status": "querying",
            "prompt": large_prompt,
            "queue_info": {
                "queue_idx": 23,
                "queue_length": 456,
                "queue_status": "Queueing"
            }
        })
        .to_string();
        let mut attempt = cap_attempt(1);
        attempt.status = "querying".to_string();
        attempt.stdout = stdout;
        attempt.stderr = "重复的命令输出".repeat(1_000);

        let mut task = make_queued_task_for_submit("frontend-projection");
        task.attempts = vec![attempt.clone()];
        task.execution_records = vec![TaskExecutionRecord {
            id: "record-1".to_string(),
            submit_id: "submit-1".to_string(),
            status: "querying".to_string(),
            started_at: "2026-07-11T00:00:00Z".to_string(),
            finished_at: String::new(),
            input_snapshot: TaskExecutionInputSnapshot::default(),
            command_preview: vec![],
            query_records: vec![attempt],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: String::new(),
            error_detail: String::new(),
        }];
        let mut data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };
        for index in 0..250 {
            append_log(
                &mut data,
                LogEntryDraft {
                    level: LogLevel::Debug,
                    source: LogSource::System,
                    category: "test".to_string(),
                    event_type: "projection".to_string(),
                    message: format!("log-{index}"),
                    detail: "大段日志".repeat(500),
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

        let projected = frontend_app_state(data.clone());

        assert_eq!(data.logs.len(), 250, "磁盘态副本不能被裁剪");
        assert_eq!(projected.logs.len(), 200, "前端只需要最近日志");
        assert_eq!(projected.tasks[0].attempts.len(), 1);
        assert_eq!(
            projected.tasks[0].execution_records[0].query_records.len(),
            1
        );
        let projected_attempt = &projected.tasks[0].execution_records[0].query_records[0];
        assert!(projected_attempt.stdout.len() < 512);
        assert!(projected_attempt.stderr.is_empty());
        let queue = parse_query_output(&projected_attempt.stdout)
            .queue_info
            .expect("queue info should remain available to the UI");
        assert_eq!(queue.queue_idx, Some(23));
        assert_eq!(queue.queue_length, Some(456));
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
        // query_records 上限 500，45 条不触发裁剪
        assert_eq!(data.tasks[0].execution_records[0].query_records.len(), 45);
        assert_eq!(
            data.tasks[0].execution_records[0].query_records[0].id,
            "att-0"
        );
        assert_eq!(removed, 10); // attempts: 60→50=10, query_records: 45<500=0
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
    fn load_migrates_and_trims_legacy_state_json() {
        let temp = tempfile::tempdir().expect("temp dir");
        // 手工写入旧版 state.json，首次 load 应迁移到 SQLite。
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

        let reloaded = AppStore::load(temp.path().to_path_buf()).snapshot();
        assert!(temp.path().join("state.sqlite3").exists());
        assert!(!path.exists(), "迁移成功后旧状态不能继续作为数据源");
        assert!(
            temp.path().join("state.json.migrated.bak").exists(),
            "必须保留迁移前原始备份"
        );
        assert_eq!(
            reloaded.tasks[0].execution_records[0].query_records.len(),
            45
        );
    }

    // ── SQLite 行级持久化 ──────────────────────────────────────────────────

    #[test]
    fn persist_writes_sqlite_and_can_reload() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                data.tasks.push(make_queued_task_for_submit("compact-1"));
                Ok(())
            })
            .expect("mutate");
        assert!(temp.path().join("state.sqlite3").exists());
        assert!(!temp.path().join("state.json").exists());
        let reloaded = AppStore::load(temp.path().to_path_buf()).snapshot();
        assert_eq!(reloaded.tasks.len(), 1);
        assert_eq!(reloaded.tasks[0].id, "compact-1");
    }

    #[test]
    fn sqlite_mutation_updates_only_changed_task_row() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                data.tasks.push(make_queued_task_for_submit("changed"));
                data.tasks.push(make_queued_task_for_submit("untouched"));
                Ok(())
            })
            .expect("seed tasks");
        let connection =
            rusqlite::Connection::open(temp.path().join("state.sqlite3")).expect("open sqlite");
        connection
            .execute_batch(
                "
                CREATE TABLE task_update_audit (task_id TEXT NOT NULL);
                CREATE TRIGGER audit_task_update
                AFTER UPDATE ON tasks
                BEGIN
                    INSERT INTO task_update_audit (task_id) VALUES (NEW.id);
                END;
                ",
            )
            .expect("install audit trigger");

        store
            .mutate(|data| {
                data.tasks[0].status = "paused".to_string();
                Ok(())
            })
            .expect("update one task");

        let updated_ids = connection
            .prepare("SELECT task_id FROM task_update_audit")
            .expect("prepare audit query")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query audit")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect audit");
        assert_eq!(updated_ids, vec!["changed"]);
    }

    #[test]
    fn sqlite_noop_mutation_keeps_revision_stable() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        let before = store.state_signature();
        store.mutate(|_| Ok(())).expect("no-op mutation");
        assert_eq!(store.state_signature(), before);
    }

    #[test]
    fn sqlite_delete_and_head_insert_do_not_update_unchanged_rows() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                data.tasks.push(make_queued_task_for_submit("delete-me"));
                data.tasks.push(make_queued_task_for_submit("keep-me"));
                data.imagegen_history.push(ImageGenHistoryItem {
                    id: "old-image".to_string(),
                    prompt: "old".to_string(),
                    size: "1:1".to_string(),
                    stored_path: String::new(),
                    size_bytes: 0,
                    mime: String::new(),
                    reference_asset_ids: vec![],
                    created_at: now_rfc3339(),
                    status: "completed".to_string(),
                    task_id: None,
                    error: None,
                });
                Ok(())
            })
            .expect("seed rows");
        let connection =
            rusqlite::Connection::open(temp.path().join("state.sqlite3")).expect("open sqlite");
        connection
            .execute_batch(
                "
                CREATE TABLE row_update_audit (table_name TEXT NOT NULL, row_id TEXT NOT NULL);
                CREATE TRIGGER audit_task_row_update
                AFTER UPDATE ON tasks
                BEGIN
                    INSERT INTO row_update_audit VALUES ('tasks', NEW.id);
                END;
                CREATE TRIGGER audit_image_row_update
                AFTER UPDATE ON imagegen_history
                BEGIN
                    INSERT INTO row_update_audit VALUES ('imagegen_history', NEW.id);
                END;
                ",
            )
            .expect("install audit triggers");

        store
            .mutate(|data| {
                data.tasks.remove(0);
                data.imagegen_history.insert(
                    0,
                    ImageGenHistoryItem {
                        id: "new-image".to_string(),
                        prompt: "new".to_string(),
                        size: "1:1".to_string(),
                        stored_path: String::new(),
                        size_bytes: 0,
                        mime: String::new(),
                        reference_asset_ids: vec![],
                        created_at: now_rfc3339(),
                        status: "completed".to_string(),
                        task_id: None,
                        error: None,
                    },
                );
                Ok(())
            })
            .expect("delete and prepend");

        let update_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM row_update_audit", [], |row| {
                row.get(0)
            })
            .expect("count updates");
        assert_eq!(update_count, 0);
        let reloaded = AppStore::load(temp.path().to_path_buf()).snapshot();
        assert_eq!(reloaded.tasks[0].id, "keep-me");
        assert_eq!(reloaded.imagegen_history[0].id, "new-image");
        assert_eq!(reloaded.imagegen_history[1].id, "old-image");
    }

    #[test]
    fn sqlite_persists_restart_normalization() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                let mut task = make_queued_task_for_submit("recover");
                task.status = "submitting".to_string();
                data.tasks.push(task);
                Ok(())
            })
            .expect("seed interrupted task");
        drop(store);

        let _reloaded = AppStore::load(temp.path().to_path_buf());
        let connection =
            rusqlite::Connection::open(temp.path().join("state.sqlite3")).expect("open sqlite");
        let stored_json = connection
            .query_row("SELECT data_json FROM tasks LIMIT 1", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("read task row");
        let stored: ScheduledTask = serde_json::from_str(&stored_json).expect("parse task row");
        assert_eq!(stored.status, "queued");
    }

    #[test]
    fn sqlite_preserves_existing_entity_reorder() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                data.tasks.push(make_queued_task_for_submit("first"));
                data.tasks.push(make_queued_task_for_submit("second"));
                Ok(())
            })
            .expect("seed tasks");
        store
            .mutate(|data| {
                data.tasks.swap(0, 1);
                Ok(())
            })
            .expect("reorder tasks");

        let reloaded = AppStore::load(temp.path().to_path_buf()).snapshot();
        let ids = reloaded
            .tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["second", "first"]);
    }

    #[test]
    fn sqlite_rejects_newer_schema_version() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        drop(store);
        let connection =
            rusqlite::Connection::open(temp.path().join("state.sqlite3")).expect("open sqlite");
        connection
            .pragma_update(None, "user_version", 2)
            .expect("set future schema");
        drop(connection);

        let incompatible = AppStore::load(temp.path().to_path_buf());
        let error = incompatible
            .try_snapshot()
            .expect_err("must reject newer schema");
        assert!(error.to_string().contains("高于当前支持版本"));
        assert!(incompatible.mutate(|_| Ok(())).is_err());
    }

    #[test]
    fn sqlite_preserves_newest_first_image_history_order() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = AppStore::load(temp.path().to_path_buf());
        store
            .mutate(|data| {
                for id in ["older", "newer"] {
                    data.imagegen_history.insert(
                        0,
                        ImageGenHistoryItem {
                            id: id.to_string(),
                            prompt: id.to_string(),
                            size: "1:1".to_string(),
                            stored_path: String::new(),
                            size_bytes: 0,
                            mime: String::new(),
                            reference_asset_ids: vec![],
                            created_at: now_rfc3339(),
                            status: "completed".to_string(),
                            task_id: None,
                            error: None,
                        },
                    );
                }
                Ok(())
            })
            .expect("mutate");

        let reloaded = AppStore::load(temp.path().to_path_buf()).snapshot();
        let ids = reloaded
            .imagegen_history
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["newer", "older"]);
    }

    #[test]
    fn load_isolates_task_with_invalid_command_preview_type() {
        let temp = tempfile::tempdir().expect("temp dir");
        let good = make_queued_task_for_submit("good-task");
        let bad = make_queued_task_for_submit("bad-task");
        let mut value = serde_json::to_value(AppData {
            tasks: vec![good, bad],
            ..AppData::default()
        })
        .expect("serialize app data");
        let tasks = value
            .get_mut("tasks")
            .and_then(|item| item.as_array_mut())
            .expect("tasks array");
        tasks[1]["command_preview"] = JsonValue::String("--prompt=broken".to_string());
        std::fs::write(
            temp.path().join("state.json"),
            serde_json::to_string(&value).expect("serialize broken state"),
        )
        .expect("write state");

        let reloaded = AppStore::load(temp.path().to_path_buf()).snapshot();

        assert_eq!(reloaded.tasks.len(), 2, "坏任务不应导致任务库清空");
        assert_eq!(reloaded.tasks[0].id, "good-task");
        let isolated = reloaded
            .tasks
            .iter()
            .find(|task| task.id == "bad-task")
            .expect("bad task should remain visible");
        assert_eq!(isolated.status, "schema_error");
        assert!(isolated.next_run_at.is_none());
        assert!(
            isolated.last_error.contains("command_preview"),
            "unexpected error: {}",
            isolated.last_error
        );
        assert!(reloaded.logs.iter().any(|log| {
            log.event_type == "task_schema_error_isolated"
                && log.task_id.as_deref() == Some("bad-task")
        }));
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

    #[test]
    fn should_process_now_false_when_remote_query_is_not_due() {
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let mut task = make_backoff_task(1, Some(&now_text), false);
        task.queue_info = Some(QueueInfo {
            queue_idx: Some(5_000),
            priority: Some(1),
            queue_status: Some("Queueing".to_string()),
            queue_length: Some(300_000),
        });
        let data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };

        assert!(
            !should_process_now(&data),
            "查询未到期时不应执行重函数并重写整份状态"
        );
    }

    #[test]
    fn auto_query_disabled_does_not_plan_remote_query() {
        let settings = SchedulerSettings {
            auto_query_enabled: false,
            ..SchedulerSettings::default()
        };
        let task = make_backoff_task(0, None, false);
        let data = AppData {
            settings,
            tasks: vec![task],
            ..AppData::default()
        };

        assert!(
            peek_due_task_cli(&data).expect("plan query").is_none(),
            "关闭自动查询后只能由手动刷新触发远端查询"
        );
    }

    // ── query_interval_secs（按队列状态/位置自适应，无退避增长）─────────────
    fn make_queue_task(queue_status: &str, queue_idx: Option<u64>) -> ScheduledTask {
        let mut task = make_backoff_task(0, None, false);
        task.queue_info = Some(QueueInfo {
            queue_idx,
            priority: Some(1),
            queue_status: Some(queue_status.to_string()),
            queue_length: Some(304_151),
        });
        task
    }

    #[test]
    fn query_interval_generating_is_60s() {
        assert_eq!(
            query_interval_secs(&make_queue_task("Generating", Some(0))),
            60
        );
    }

    #[test]
    fn query_interval_queueing_within_100_is_180s() {
        assert_eq!(
            query_interval_secs(&make_queue_task("Queueing", Some(50))),
            180
        );
        assert_eq!(
            query_interval_secs(&make_queue_task("Queueing", Some(100))),
            180
        );
    }

    #[test]
    fn query_interval_queueing_101_to_1000_is_600s() {
        assert_eq!(
            query_interval_secs(&make_queue_task("Queueing", Some(101))),
            600
        );
        assert_eq!(
            query_interval_secs(&make_queue_task("Queueing", Some(1000))),
            600
        );
    }

    #[test]
    fn query_interval_queueing_over_1000_is_1200s() {
        assert_eq!(
            query_interval_secs(&make_queue_task("Queueing", Some(1001))),
            1200
        );
        assert_eq!(
            query_interval_secs(&make_queue_task("Queueing", Some(5_000))),
            1200
        );
    }

    #[test]
    fn query_interval_no_queue_info_is_60s() {
        let task = make_backoff_task(0, None, false); // queue_info: None
        assert_eq!(query_interval_secs(&task), 60);
    }

    // ── is_query_due（固定/自适应间隔，命中即查，不随次数退避）──────────────
    #[test]
    fn is_query_due_no_last_query_returns_true() {
        let task = make_queue_task("Generating", Some(0));
        assert!(is_query_due(&task, Utc::now()));
    }

    #[test]
    fn is_query_due_generating_interval_not_elapsed_returns_false() {
        let mut task = make_queue_task("Generating", Some(0));
        task.last_auto_query_at = Some((Utc::now() - Duration::seconds(30)).to_rfc3339());
        assert!(!is_query_due(&task, Utc::now()));
    }

    #[test]
    fn is_query_due_generating_interval_elapsed_returns_true() {
        let mut task = make_queue_task("Generating", Some(0));
        task.last_auto_query_at = Some((Utc::now() - Duration::seconds(61)).to_rfc3339());
        assert!(is_query_due(&task, Utc::now()));
    }

    #[test]
    fn is_query_due_does_not_grow_with_consecutive_count() {
        // 退避已废除：间隔只取决于队列状态/位置，不随查询次数增长。
        let mut task = make_queue_task("Generating", Some(0));
        task.consecutive_no_result_queries = 50; // 旧退避会膨胀到 600s
        task.last_auto_query_at = Some((Utc::now() - Duration::seconds(61)).to_rfc3339());
        assert!(is_query_due(&task, Utc::now()));
    }

    #[test]
    fn is_query_due_with_malformed_timestamp_returns_true() {
        let mut task = make_queue_task("Queueing", Some(50));
        task.last_auto_query_at = Some("not-a-timestamp".to_string());
        assert!(is_query_due(&task, Utc::now()));
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
    fn is_past_max_wait_under_limit_returns_false() {
        let recent = (Utc::now() - Duration::hours(2)).to_rfc3339();
        assert!(!is_past_max_wait(
            &make_past_max_task(Some(&recent)),
            Utc::now()
        ));
    }

    #[test]
    fn is_past_max_wait_over_6_hours_returns_true() {
        let past = (Utc::now() - Duration::hours(7)).to_rfc3339();
        assert!(is_past_max_wait(
            &make_past_max_task(Some(&past)),
            Utc::now()
        ));
    }

    #[test]
    fn is_past_max_wait_exactly_6_hours_returns_true() {
        let past = (Utc::now() - Duration::hours(6)).to_rfc3339();
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

    // ── manual query bypasses max-wait cap ─────────────────────────────────

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
    fn auto_query_past_max_wait_keeps_task_stopped() {
        let mut data = AppData {
            tasks: vec![make_long_pending_task()],
            ..AppData::default()
        };
        // 有远端任务状态但已达到最长等待时间：停止自动查询，改为手动查询
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
    fn manual_query_past_max_wait_bypasses_cap() {
        let mut data = AppData {
            tasks: vec![make_long_pending_task()],
            ..AppData::default()
        };
        // 模拟手动查询前的 reset
        if let Some(t) = data.tasks.iter_mut().find(|t| t.id == "task-stale") {
            reset_query_backoff(t);
        }
        // No-result CLI output: should NOT re-trigger max-wait cap
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
            "manual query should not surface the max-wait stop message: {}",
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
            last_used_at: None,
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
            queued_at: None,
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
            prompt_doc: None,
        }
    }

    #[test]
    fn next_due_submit_uses_queue_start_time_when_scheduled_time_ties() {
        let scheduled_at = "2026-05-03T02:00:00Z";
        let mut old_draft_queued_later = make_queued_task_for_submit("old-draft");
        old_draft_queued_later.status = "scheduled".to_string();
        old_draft_queued_later.scheduled_at = Some(scheduled_at.to_string());
        old_draft_queued_later.next_run_at = Some(scheduled_at.to_string());
        old_draft_queued_later.created_at = "2026-05-01T00:00:00Z".to_string();
        old_draft_queued_later.updated_at = "2026-05-02T10:00:00Z".to_string();

        let mut new_draft_queued_earlier = make_queued_task_for_submit("new-draft");
        new_draft_queued_earlier.status = "scheduled".to_string();
        new_draft_queued_earlier.scheduled_at = Some(scheduled_at.to_string());
        new_draft_queued_earlier.next_run_at = Some(scheduled_at.to_string());
        new_draft_queued_earlier.created_at = "2026-05-01T12:00:00Z".to_string();
        new_draft_queued_earlier.updated_at = "2026-05-02T09:00:00Z".to_string();

        let data = AppData {
            tasks: vec![old_draft_queued_later, new_draft_queued_earlier],
            ..AppData::default()
        };

        assert_eq!(
            next_due_submit_task_id(
                &data,
                DateTime::parse_from_rfc3339(scheduled_at)
                    .unwrap()
                    .with_timezone(&Utc)
            ),
            Some("new-draft".to_string())
        );
    }

    #[test]
    fn finished_execution_record_does_not_hold_standard_lane() {
        let now = DateTime::parse_from_rfc3339("2026-05-03T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut stale_standard = make_queued_task_for_submit("old-standard");
        stale_standard.status = "succeeded".to_string();
        stale_standard.execution_records.push(TaskExecutionRecord {
            id: "rec-finished".to_string(),
            submit_id: "finished-submit".to_string(),
            status: "querying".to_string(),
            started_at: "2026-05-02T01:00:00Z".to_string(),
            finished_at: "2026-05-02T01:10:00Z".to_string(),
            input_snapshot: TaskExecutionInputSnapshot::default(),
            command_preview: vec![],
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: String::new(),
            error_detail: String::new(),
        });
        let mut queued_standard = make_queued_task_for_submit("new-standard");
        queued_standard.queued_at = Some("2026-05-03T01:00:00Z".to_string());
        let data = AppData {
            tasks: vec![stale_standard, queued_standard],
            ..AppData::default()
        };
        let active_kinds = active_remote_queue_kinds(&data);

        assert!(
            !active_kinds.contains(&ModelQueueKind::Standard),
            "finished execution records must not keep the standard lane busy"
        );
        let selection = next_submit_task_id_for_available_queues(&data, now, &active_kinds)
            .expect("standard lane should accept the queued task");
        assert_eq!(selection.task_id, "new-standard");
        assert_eq!(selection.target_queue_kind, ModelQueueKind::Standard);
    }

    #[test]
    fn succeeded_task_active_fast_record_keeps_fast_lane_occupied() {
        let now = DateTime::parse_from_rfc3339("2026-05-03T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut completed = make_queued_task_for_submit("completed");
        completed.status = "succeeded".to_string();
        completed.submit_id = "standard-done".to_string();
        completed.result_urls = vec!["https://example.com/result.mp4".to_string()];
        completed.execution_records.push(TaskExecutionRecord {
            id: "stale-fast".to_string(),
            submit_id: "fast-stale".to_string(),
            status: "querying".to_string(),
            started_at: "2026-05-02T01:00:00Z".to_string(),
            finished_at: String::new(),
            input_snapshot: TaskExecutionInputSnapshot {
                params: VideoParams {
                    model_version: FAST_FALLBACK_MODEL_VERSION.to_string(),
                    ..VideoParams::default()
                },
                ..TaskExecutionInputSnapshot::default()
            },
            command_preview: vec![],
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: "FastFallback".to_string(),
            error_detail: String::new(),
        });
        let mut queued_fast = make_queued_task_for_submit("new-fast");
        queued_fast.params.model_version = FAST_FALLBACK_MODEL_VERSION.to_string();
        let data = AppData {
            tasks: vec![completed, queued_fast],
            ..AppData::default()
        };
        let active_kinds = active_remote_queue_kinds(&data);

        assert!(
            active_kinds.contains(&ModelQueueKind::Fast),
            "a remote record must stay occupied until its own query finishes"
        );
        let selection = next_submit_task_id_for_available_queues(&data, now, &active_kinds)
            .expect("an idle lane should accept the queued task");
        assert_eq!(selection.task_id, "new-fast");
        assert_eq!(selection.target_queue_kind, ModelQueueKind::Standard);
    }

    #[test]
    fn fast_concurrency_retry_switches_immediately_to_idle_standard_lane() {
        let now = DateTime::parse_from_rfc3339("2026-05-03T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut task = make_queued_task_for_submit("fast-cooling");
        task.status = "retry_wait".to_string();
        task.next_run_at = Some("2026-05-03T02:05:00Z".to_string());
        task.last_error = "ExceedConcurrencyLimit".to_string();
        task.params.model_version = FAST_FALLBACK_MODEL_VERSION.to_string();
        task.execution_records.push(TaskExecutionRecord {
            id: "fast-retry".to_string(),
            submit_id: String::new(),
            status: "retry_wait".to_string(),
            started_at: "2026-05-03T01:59:00Z".to_string(),
            finished_at: "2026-05-03T01:59:01Z".to_string(),
            input_snapshot: TaskExecutionInputSnapshot {
                params: VideoParams {
                    model_version: FAST_FALLBACK_MODEL_VERSION.to_string(),
                    ..VideoParams::default()
                },
                ..TaskExecutionInputSnapshot::default()
            },
            command_preview: vec![],
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: "ConcurrencyLimit".to_string(),
            error_detail: "ExceedConcurrencyLimit".to_string(),
        });
        let data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };

        let selection = next_submit_task_id_for_available_queues(&data, now, &HashSet::new())
            .expect("idle standard lane should take over immediately");
        assert_eq!(selection.task_id, "fast-cooling");
        assert_eq!(selection.target_queue_kind, ModelQueueKind::Standard);
    }

    #[test]
    fn disabled_standard_lane_routes_new_task_to_fast() {
        let now = DateTime::parse_from_rfc3339("2026-05-03T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut data = AppData {
            tasks: vec![make_queued_task_for_submit("fast-only")],
            ..AppData::default()
        };
        data.settings.standard_lane_enabled = false;

        let selection = next_submit_task_id_for_available_queues(&data, now, &HashSet::new())
            .expect("enabled fast lane should accept the task");
        assert_eq!(selection.target_queue_kind, ModelQueueKind::Fast);
    }

    #[test]
    fn queue_priority_stars_choose_next_then_second() {
        let now = DateTime::parse_from_rfc3339("2026-05-03T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut data = AppData {
            tasks: vec![
                make_queued_task_for_submit("normal"),
                make_queued_task_for_submit("one-star"),
                make_queued_task_for_submit("two-star"),
            ],
            ..AppData::default()
        };
        set_task_queue_priority(&mut data, "one-star", 1).unwrap();
        set_task_queue_priority(&mut data, "two-star", 2).unwrap();

        let first = next_submit_task_id_for_available_queues(&data, now, &HashSet::new())
            .expect("two-star task should be selected");
        assert_eq!(first.task_id, "two-star");

        data.tasks.retain(|task| task.id != "two-star");
        let second = next_submit_task_id_for_available_queues(&data, now, &HashSet::new())
            .expect("one-star task should be selected second");
        assert_eq!(second.task_id, "one-star");
    }

    #[test]
    fn generation_precheck_retry_waits_behind_regular_queue_even_when_starred() {
        let now = DateTime::parse_from_rfc3339("2026-05-03T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut review_retry = make_queued_task_for_submit("task-a-review-retry");
        review_retry.status = "retry_wait".to_string();
        review_retry.next_run_at = Some("2026-05-03T01:59:00Z".to_string());
        review_retry.last_error = "generation failed: pre-TNS check did not pass".to_string();
        review_retry.execution_records.push(TaskExecutionRecord {
            id: "review-retry-record".to_string(),
            submit_id: "review-submit".to_string(),
            status: "retry_wait".to_string(),
            started_at: "2026-05-03T01:58:00Z".to_string(),
            finished_at: "2026-05-03T01:59:00Z".to_string(),
            input_snapshot: TaskExecutionInputSnapshot::default(),
            command_preview: vec![],
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: "GenerationPrecheck".to_string(),
            error_detail: "generation failed: pre-TNS check did not pass".to_string(),
        });
        let regular = make_queued_task_for_submit("task-z-regular");
        let mut data = AppData {
            tasks: vec![review_retry, regular],
            ..AppData::default()
        };
        set_task_queue_priority(&mut data, "task-a-review-retry", 2).unwrap();

        let first = next_submit_task_id_for_available_queues(&data, now, &HashSet::new())
            .expect("普通排队任务应先于审核重试");
        assert_eq!(first.task_id, "task-z-regular");

        data.tasks.retain(|task| task.id != "task-z-regular");
        let second = next_submit_task_id_for_available_queues(&data, now, &HashSet::new())
            .expect("没有普通任务后应执行审核重试");
        assert_eq!(second.task_id, "task-a-review-retry");
    }

    #[test]
    fn unstarred_queue_uses_stable_random_id_order_instead_of_fifo() {
        let now = DateTime::parse_from_rfc3339("2026-05-03T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut older = make_queued_task_for_submit("task-z");
        older.queued_at = Some("2026-05-03T00:00:00Z".to_string());
        let mut newer = make_queued_task_for_submit("task-a");
        newer.queued_at = Some("2026-05-03T01:00:00Z".to_string());
        let data = AppData {
            tasks: vec![older, newer],
            ..AppData::default()
        };

        let selected = next_submit_task_id_for_available_queues(&data, now, &HashSet::new())
            .expect("one task should be selected");
        assert_eq!(selected.task_id, "task-a");
    }

    #[test]
    fn recover_tasks_on_load_preserves_other_active_records() {
        let mut completed = make_queued_task_for_submit("completed");
        completed.status = "succeeded".to_string();
        completed.submit_id = "standard-done".to_string();
        completed.finished_at = "2026-05-03T01:00:00Z".to_string();
        completed.result_urls = vec!["https://example.com/result.mp4".to_string()];
        completed.execution_records = vec![
            TaskExecutionRecord {
                id: "standard-done-rec".to_string(),
                submit_id: "standard-done".to_string(),
                status: "succeeded".to_string(),
                started_at: "2026-05-02T01:00:00Z".to_string(),
                finished_at: "2026-05-03T01:00:00Z".to_string(),
                input_snapshot: TaskExecutionInputSnapshot::default(),
                command_preview: vec![],
                query_records: vec![],
                result_paths: vec![],
                result_urls: vec!["https://example.com/result.mp4".to_string()],
                error_kind: String::new(),
                error_detail: String::new(),
            },
            TaskExecutionRecord {
                id: "stale-fast".to_string(),
                submit_id: "fast-stale".to_string(),
                status: "querying".to_string(),
                started_at: now_rfc3339(),
                finished_at: String::new(),
                input_snapshot: TaskExecutionInputSnapshot {
                    params: VideoParams {
                        model_version: FAST_FALLBACK_MODEL_VERSION.to_string(),
                        ..VideoParams::default()
                    },
                    ..TaskExecutionInputSnapshot::default()
                },
                command_preview: vec![],
                query_records: vec![],
                result_paths: vec![],
                result_urls: vec![],
                error_kind: "FastFallback".to_string(),
                error_detail: String::new(),
            },
        ];
        let mut data = AppData {
            tasks: vec![completed],
            ..AppData::default()
        };

        recover_tasks_on_load(&mut data);

        let stale = data.tasks[0]
            .execution_records
            .iter()
            .find(|record| record.id == "stale-fast")
            .expect("stale fast record");
        assert_eq!(stale.status, "querying");
        assert!(stale.finished_at.is_empty());
        assert!(stale.error_detail.is_empty());
        assert!(active_remote_queue_kinds(&data).contains(&ModelQueueKind::Fast));
    }

    #[test]
    fn recover_tasks_on_load_releases_stale_fast_record_without_remote_progress() {
        let mut completed = make_queued_task_for_submit("completed-with-stale-fast");
        completed.status = "succeeded".to_string();
        completed.submit_id = "completed-submit".to_string();
        completed.result_urls = vec!["https://example.com/completed.mp4".to_string()];
        completed.finished_at = "2026-07-01T01:00:00Z".to_string();
        completed.execution_records.push(TaskExecutionRecord {
            id: "stale-fast-querying".to_string(),
            submit_id: "stale-fast-submit".to_string(),
            status: "querying".to_string(),
            started_at: "2026-07-01T00:00:00Z".to_string(),
            finished_at: String::new(),
            input_snapshot: TaskExecutionInputSnapshot {
                params: VideoParams {
                    model_version: FAST_FALLBACK_MODEL_VERSION.to_string(),
                    ..VideoParams::default()
                },
                ..TaskExecutionInputSnapshot::default()
            },
            command_preview: vec![],
            query_records: vec![TaskAttempt {
                id: "stale-fast-query".to_string(),
                started_at: "2026-07-01T07:00:00Z".to_string(),
                finished_at: "2026-07-01T07:00:01Z".to_string(),
                status: "querying".to_string(),
                command_preview: vec![
                    "query_result".to_string(),
                    "--submit_id=stale-fast-submit".to_string(),
                ],
                stdout: r#"{"gen_status":"querying"}"#.to_string(),
                stderr: String::new(),
                error_kind: String::new(),
                duration_seconds: 1.0,
                error_detail: String::new(),
            }],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: String::new(),
            error_detail: String::new(),
        });
        let mut data = AppData {
            tasks: vec![completed],
            ..AppData::default()
        };

        recover_tasks_on_load(&mut data);

        let task = &data.tasks[0];
        let record = &task.execution_records[0];
        assert_eq!(task.status, "succeeded");
        assert_eq!(record.status, "query_timeout");
        assert!(!record.finished_at.is_empty());
        assert!(!active_remote_queue_kinds(&data).contains(&ModelQueueKind::Fast));
    }

    #[test]
    fn recover_tasks_on_load_preserves_historical_record_after_network_query_error() {
        let mut completed = make_queued_task_for_submit("completed-with-offline-fast");
        completed.status = "succeeded".to_string();
        completed.submit_id = "completed-submit".to_string();
        completed.result_urls = vec!["https://example.com/completed.mp4".to_string()];
        completed.finished_at = "2026-07-01T01:00:00Z".to_string();
        completed.execution_records.push(TaskExecutionRecord {
            id: "offline-fast-querying".to_string(),
            submit_id: "offline-fast-submit".to_string(),
            status: "querying".to_string(),
            started_at: "2026-07-01T00:00:00Z".to_string(),
            finished_at: String::new(),
            input_snapshot: TaskExecutionInputSnapshot {
                params: VideoParams {
                    model_version: FAST_FALLBACK_MODEL_VERSION.to_string(),
                    ..VideoParams::default()
                },
                ..TaskExecutionInputSnapshot::default()
            },
            command_preview: vec![],
            query_records: vec![TaskAttempt {
                id: "offline-fast-query".to_string(),
                started_at: "2026-07-01T07:00:00Z".to_string(),
                finished_at: "2026-07-01T07:00:01Z".to_string(),
                status: "retry_wait".to_string(),
                command_preview: vec![
                    "query_result".to_string(),
                    "--submit_id=offline-fast-submit".to_string(),
                ],
                stdout: String::new(),
                stderr: "dial tcp: lookup jimeng.jianying.com: no such host".to_string(),
                error_kind: "NetworkUnavailable".to_string(),
                duration_seconds: 1.0,
                error_detail: "网络暂不可用".to_string(),
            }],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: "NetworkUnavailable".to_string(),
            error_detail: "网络暂不可用".to_string(),
        });
        let mut data = AppData {
            tasks: vec![completed],
            ..AppData::default()
        };

        recover_tasks_on_load(&mut data);

        let record = &data.tasks[0].execution_records[0];
        assert_eq!(record.status, "querying");
        assert!(record.finished_at.is_empty());
        assert!(active_remote_queue_kinds(&data).contains(&ModelQueueKind::Fast));
    }

    #[test]
    fn submit_5xx_first_time_goes_to_retry_wait() {
        let mut data = AppData {
            tasks: vec![make_queued_task_for_submit("t5xx")],
            assets: vec![make_test_image_asset()],
            ..AppData::default()
        };
        let task = submit_task_once_with_runner(&mut data, "t5xx", None, |_args| {
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
        let task = submit_task_once_with_runner(&mut data, "t5xx2", None, |_args| {
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
        let task = submit_task_once_with_runner(&mut data, "t5xx3", None, |_args| {
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
        let task = submit_task_once_with_runner(&mut data, "t5xx-ok", None, |_args| {
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
            None,
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

    #[cfg(unix)]
    #[test]
    fn command_output_nonzero_exit_is_an_error() {
        use std::os::unix::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"authsdk: refresh failed: protocol transport: do request".to_vec(),
        };
        let error = command_output_to_result(output).expect_err("non-zero exit must fail");

        assert!(error.contains("退出码 1"));
        assert!(error.contains("authsdk: refresh failed"));
    }

    #[cfg(unix)]
    #[test]
    fn command_output_zero_exit_with_empty_stdout_and_network_stderr_is_an_error() {
        use std::os::unix::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: b"dial tcp: lookup jimeng.jianying.com: no such host".to_vec(),
        };
        let error = command_output_to_result(output)
            .expect_err("empty stdout plus a network failure must remain retryable");

        assert!(error.contains("no such host"));
    }

    #[cfg(unix)]
    #[test]
    fn command_output_keeps_valid_stdout_even_when_stderr_has_a_warning() {
        use std::os::unix::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: br#"{"submit_id":"sub-ok","gen_status":"querying"}"#.to_vec(),
            stderr: b"temporary warning: context deadline exceeded".to_vec(),
        };
        let (stdout, _) = command_output_to_result(output)
            .expect("valid stdout must win over a non-fatal stderr warning");

        assert!(stdout.contains("sub-ok"));
    }

    #[cfg(unix)]
    #[test]
    fn command_execution_timeout_terminates_a_hung_process() {
        let args = vec!["-c".to_string(), "while :; do :; done".to_string()];
        let started = std::time::Instant::now();

        let error = run_command_with_timeout("sh", &args, StdDuration::from_millis(50))
            .expect_err("hung process must be terminated");

        assert!(error.contains("超时"));
        assert!(started.elapsed() < StdDuration::from_secs(1));
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
        task.execution_records.clear();
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

    #[test]
    fn query_cli_exit_with_known_submit_keeps_remote_task_querying() {
        let mut task = make_task_with_execution_records();
        task.execution_records.clear();
        task.id = "task-query-cli-exit".to_string();
        task.submit_id = "sub-query-cli-exit".to_string();
        task.status = "querying".to_string();
        task.submitted_at = Some((Utc::now() - Duration::minutes(5)).to_rfc3339());
        task.execution_records.push(TaskExecutionRecord {
            id: "rec-query-cli-exit".to_string(),
            submit_id: task.submit_id.clone(),
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

        let result = query_task_submit_id_once_with_runner(
            &mut data,
            "task-query-cli-exit",
            "sub-query-cli-exit",
            |_args| Err("dreamina CLI 退出码 1".to_string()),
        )
        .expect("local query error should be recorded without failing remote task");

        assert_eq!(result.status, "querying");
        assert_eq!(result.submit_id, "sub-query-cli-exit");
        assert!(!result.auto_query_stopped);
        assert_eq!(
            result.last_error,
            "本地查询暂时未取得结果，已保留远端任务，稍后自动查询。"
        );
        let record = result.execution_records.last().expect("execution record");
        assert_eq!(record.status, "querying");
        assert_eq!(record.error_kind, "QueryUnavailable");
        assert_eq!(record.query_records.last().unwrap().status, "retry_wait");
    }

    #[test]
    fn querying_without_queue_info_after_ten_minutes_marks_failed() {
        let mut task = make_task_with_execution_records();
        task.execution_records.clear();
        task.id = "task-live-no-queue-info".to_string();
        task.submit_id = "sub-live-no-queue-info".to_string();
        task.status = "querying".to_string();
        task.submitted_at = Some((Utc::now() - Duration::minutes(11)).to_rfc3339());
        task.execution_records.push(TaskExecutionRecord {
            id: "rec-live-no-queue-info".to_string(),
            submit_id: task.submit_id.clone(),
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

        let result = query_task_submit_id_once_with_runner(
            &mut data,
            "task-live-no-queue-info",
            "sub-live-no-queue-info",
            |_args| {
                Ok((
                    r#"{"submit_id":"sub-live-no-queue-info","gen_status":"querying"}"#.to_string(),
                    String::new(),
                ))
            },
        )
        .expect("query should succeed");

        assert_eq!(result.status, "failed");
        assert!(result.last_error.contains("10 分钟"));
        assert_eq!(
            result.execution_records.last().unwrap().error_kind,
            "RemoteProgressTimeout"
        );
    }

    #[test]
    fn async_submit_execution_record_remains_active() {
        let mut data = AppData {
            tasks: vec![make_queued_task_for_submit("task-active-submit")],
            assets: vec![make_test_image_asset()],
            ..AppData::default()
        };

        let result = submit_task_once_with_runner(
            &mut data,
            "task-active-submit",
            Some("seedance2.0"),
            |_args| {
                Ok((
                    r#"{"submit_id":"sub-active","gen_status":"querying"}"#.to_string(),
                    String::new(),
                ))
            },
        )
        .expect("submit should succeed");

        let record = result.execution_records.last().expect("execution record");
        assert_eq!(record.status, "querying");
        assert!(record.finished_at.is_empty());
        assert!(is_active_execution_record(record));
        assert!(active_remote_queue_kinds(&data).contains(&ModelQueueKind::Standard));
    }

    #[test]
    fn recover_failed_task_with_live_record_restores_querying() {
        let mut task = make_migration_task("failed", "本地误判失败");
        task.finished_at = now_rfc3339();
        task.execution_records.push(TaskExecutionRecord {
            id: "rec-still-live".to_string(),
            submit_id: "sub-still-live".to_string(),
            status: "querying".to_string(),
            started_at: "2026-07-10T07:00:00Z".to_string(),
            finished_at: "2026-07-10T07:00:01Z".to_string(),
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

        recover_tasks_on_load(&mut data);

        let task = &data.tasks[0];
        assert_eq!(task.status, "submitted");
        assert_eq!(task.submit_id, "sub-still-live");
        assert!(task.finished_at.is_empty());
        assert!(task.last_error.is_empty());
        assert!(is_active_execution_record(&task.execution_records[0]));
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
            queued_at: None,
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
            prompt_doc: None,
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
                queued_at: None,
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
                prompt_doc: None,
            }],
            ..AppData::default()
        };
        recover_tasks_on_load(&mut data);
        let task = &data.tasks[0];
        assert_eq!(task.status, "succeeded");
        assert!(!task.auto_query_stopped);
    }

    // ── delete_execution_record cleans up task.attempts ──────────────────

    #[test]
    fn delete_execution_record_removes_matching_attempts() {
        let mut task = make_queued_task_for_submit("del-attempt");
        task.submit_id = "sid-abc".to_string();
        task.execution_records = vec![
            TaskExecutionRecord {
                id: "rec-1".to_string(),
                submit_id: "sid-abc".to_string(),
                status: "failed".to_string(),
                started_at: "2026-07-01T00:00:00Z".to_string(),
                finished_at: "2026-07-01T01:00:00Z".to_string(),
                input_snapshot: TaskExecutionInputSnapshot::default(),
                command_preview: vec!["multimodal2video".to_string()],
                query_records: vec![],
                result_paths: vec![],
                result_urls: vec![],
                error_kind: "Transient".to_string(),
                error_detail: "network error".to_string(),
            },
            TaskExecutionRecord {
                id: "rec-2".to_string(),
                submit_id: "sid-other".to_string(),
                status: "succeeded".to_string(),
                started_at: "2026-07-02T00:00:00Z".to_string(),
                finished_at: "2026-07-02T01:00:00Z".to_string(),
                input_snapshot: TaskExecutionInputSnapshot::default(),
                command_preview: vec!["multimodal2video".to_string()],
                query_records: vec![],
                result_paths: vec![],
                result_urls: vec![],
                error_kind: String::new(),
                error_detail: String::new(),
            },
        ];
        task.attempts = vec![
            TaskAttempt {
                id: "att-submit".to_string(),
                started_at: "2026-07-01T00:00:00Z".to_string(),
                finished_at: "2026-07-01T00:01:00Z".to_string(),
                status: "failed".to_string(),
                command_preview: vec!["multimodal2video".to_string()],
                stdout: r#"{"submit_id":"sid-abc"}"#.to_string(),
                stderr: String::new(),
                error_kind: String::new(),
                duration_seconds: 0.0,
                error_detail: String::new(),
            },
            TaskAttempt {
                id: "att-query".to_string(),
                started_at: "2026-07-01T00:05:00Z".to_string(),
                finished_at: "2026-07-01T00:06:00Z".to_string(),
                status: "querying".to_string(),
                command_preview: vec![
                    "query_result".to_string(),
                    "--submit_id=sid-abc".to_string(),
                ],
                stdout: String::new(),
                stderr: String::new(),
                error_kind: String::new(),
                duration_seconds: 0.0,
                error_detail: String::new(),
            },
            TaskAttempt {
                id: "att-other".to_string(),
                started_at: "2026-07-02T00:00:00Z".to_string(),
                finished_at: "2026-07-02T01:00:00Z".to_string(),
                status: "succeeded".to_string(),
                command_preview: vec!["multimodal2video".to_string()],
                stdout: r#"{"submit_id":"sid-other"}"#.to_string(),
                stderr: String::new(),
                error_kind: String::new(),
                duration_seconds: 0.0,
                error_detail: String::new(),
            },
        ];
        let mut data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };
        let result = delete_execution_record_from_data(&mut data, "del-attempt", "rec-1");
        assert!(result.is_ok(), "delete should succeed: {:?}", result.err());
        let task = &data.tasks[0];
        // att-submit and att-query have sid-abc → removed; att-other has sid-other → kept
        assert_eq!(
            task.attempts.len(),
            1,
            "only unrelated attempt (sid-other) should remain"
        );
        assert_eq!(task.attempts[0].id, "att-other");
        // rec-1 removed, rec-2 still there
        assert_eq!(task.execution_records.len(), 1);
        assert_eq!(task.execution_records[0].id, "rec-2");
    }

    // ── touch_asset_last_used ────────────────────────────────────────────

    #[test]
    fn touch_asset_last_used_updates_only_targeted_assets() {
        let mut data = AppData {
            assets: vec![
                Asset {
                    id: "img-1".to_string(),
                    kind: AssetKind::Image,
                    name: "a.png".to_string(),
                    aliases: vec![],
                    tags: vec![],
                    stored_path: "/a.png".to_string(),
                    source_path: "/a.png".to_string(),
                    mime: "image/png".to_string(),
                    size_bytes: 100,
                    duration_seconds: None,
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    content_hash: None,
                    last_used_at: None,
                },
                Asset {
                    id: "img-2".to_string(),
                    kind: AssetKind::Image,
                    name: "b.png".to_string(),
                    aliases: vec![],
                    tags: vec![],
                    stored_path: "/b.png".to_string(),
                    source_path: "/b.png".to_string(),
                    mime: "image/png".to_string(),
                    size_bytes: 200,
                    duration_seconds: None,
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    content_hash: None,
                    last_used_at: None,
                },
            ],
            ..AppData::default()
        };
        touch_asset_last_used(&mut data, &["img-1".to_string()], &[]);
        assert!(
            data.assets[0].last_used_at.is_some(),
            "img-1 should be touched"
        );
        assert!(
            data.assets[1].last_used_at.is_none(),
            "img-2 should be untouched"
        );
    }

    #[test]
    fn touch_asset_last_used_empty_lists_are_noop() {
        let mut data = AppData::default();
        touch_asset_last_used(&mut data, &[], &[]);
        // no panic
    }

    #[test]
    fn dedupe_query_records_removes_same_query_with_different_ids() {
        let mut records = vec![
            TaskAttempt {
                id: "legacy-attempt".to_string(),
                started_at: "2026-07-05T10:17:42Z".to_string(),
                finished_at: "2026-07-05T10:17:42Z".to_string(),
                status: "querying".to_string(),
                command_preview: vec!["query_result".to_string(), "--submit_id=sid".to_string()],
                stdout: r#"{"queue_info":{"queue_idx":1242,"queue_length":567978,"queue_status":"Queueing"}}"#.to_string(),
                stderr: String::new(),
                error_kind: String::new(),
                duration_seconds: 0.0,
                error_detail: String::new(),
            },
            TaskAttempt {
                id: "query-record".to_string(),
                started_at: "2026-07-05T10:17:42Z".to_string(),
                finished_at: "2026-07-05T10:17:42Z".to_string(),
                status: "querying".to_string(),
                command_preview: vec!["query_result".to_string(), "--submit_id=sid".to_string()],
                stdout: r#"{"queue_info":{"queue_idx":1242,"queue_length":567978,"queue_status":"Queueing"}}"#.to_string(),
                stderr: String::new(),
                error_kind: String::new(),
                duration_seconds: 0.0,
                error_detail: String::new(),
            },
        ];
        dedupe_query_records(&mut records);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "legacy-attempt");
    }

    #[test]
    fn app_data_serializes_lane_status_as_camel_case() {
        let mut data = AppData::default();
        data.lane_status = vec![LaneStatus {
            queue_kind: "standard".to_string(),
            model_version: "seedance2.0".to_string(),
            enabled: true,
            is_active: false,
            is_cooling_down: false,
            cooldown_reason: String::new(),
            current_task_id: String::new(),
            current_task_title: String::new(),
            submit_id: String::new(),
            queue_position: None,
            queue_length: None,
            next_check_at: String::new(),
            waiting_task_count: 0,
        }];
        let value = serde_json::to_value(&data).expect("serialize app data");
        assert!(value.get("laneStatus").is_some());
        assert!(value.get("lane_status").is_none());
    }

    #[test]
    fn lane_status_prefers_active_task_with_queue_progress() {
        let now = DateTime::parse_from_rfc3339("2026-07-11T00:55:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut no_progress = make_queued_task_for_submit("no-progress");
        no_progress.title = "没有进度的活跃任务".to_string();
        no_progress.status = "querying".to_string();
        no_progress.submit_id = "sub-no-progress".to_string();

        let mut with_progress = make_queued_task_for_submit("with-progress");
        with_progress.title = "有进度的活跃任务".to_string();
        with_progress.status = "querying".to_string();
        with_progress.submit_id = "sub-with-progress".to_string();
        with_progress.queue_info = Some(QueueInfo {
            queue_idx: Some(6932),
            priority: Some(1),
            queue_status: Some("Queueing".to_string()),
            queue_length: Some(298595),
        });
        let data = AppData {
            tasks: vec![no_progress, with_progress],
            ..AppData::default()
        };

        let standard = compute_lane_status(&data, now)
            .into_iter()
            .find(|lane| lane.queue_kind == "standard")
            .expect("standard lane");
        assert_eq!(standard.current_task_id, "with-progress");
        assert_eq!(standard.queue_position, Some(6932));
        assert_eq!(standard.queue_length, Some(298595));
    }

    #[test]
    fn lane_status_uses_active_fast_execution_record_details() {
        let now = DateTime::parse_from_rfc3339("2026-07-04T00:00:30Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut task = make_queued_task_for_submit("fast-record-task");
        task.title = "标准任务的 Fast 兜底".to_string();
        task.status = "submitted".to_string();
        task.submit_id = "standard-sub".to_string();
        task.submitted_at = Some("2026-07-04T00:00:00Z".to_string());
        task.queue_info = Some(QueueInfo {
            queue_idx: Some(200),
            priority: None,
            queue_status: Some("queueing".to_string()),
            queue_length: Some(500),
        });
        task.execution_records = vec![TaskExecutionRecord {
            id: "fast-rec".to_string(),
            submit_id: "fast-sub".to_string(),
            status: "querying".to_string(),
            started_at: "2026-07-04T00:00:00Z".to_string(),
            finished_at: String::new(),
            input_snapshot: TaskExecutionInputSnapshot {
                params: VideoParams {
                    model_version: FAST_FALLBACK_MODEL_VERSION.to_string(),
                    ..VideoParams::default()
                },
                ..TaskExecutionInputSnapshot::default()
            },
            command_preview: vec![],
            query_records: vec![TaskAttempt {
                id: "query-fast".to_string(),
                started_at: "2026-07-04T00:00:10Z".to_string(),
                finished_at: "2026-07-04T00:00:20Z".to_string(),
                status: "querying".to_string(),
                command_preview: vec![],
                stdout:
                    r#"{"queue_info":{"queue_idx":7,"queue_length":99,"queue_status":"queueing"}}"#
                        .to_string(),
                stderr: String::new(),
                error_kind: String::new(),
                duration_seconds: 0.0,
                error_detail: String::new(),
            }],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: String::new(),
            error_detail: String::new(),
        }];
        let data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };

        let statuses = compute_lane_status(&data, now);
        let fast = statuses
            .iter()
            .find(|status| status.queue_kind == "fast")
            .expect("fast lane status");
        assert!(fast.is_active);
        assert_eq!(fast.current_task_id, "fast-record-task");
        assert_eq!(fast.current_task_title, "标准任务的 Fast 兜底");
        assert_eq!(fast.submit_id, "fast-sub");
        assert_eq!(fast.queue_position, Some(7));
        assert_eq!(fast.queue_length, Some(99));
        assert_eq!(fast.next_check_at, "2026-07-04T00:01:20+00:00");
    }

    #[test]
    fn active_cross_lane_fast_submit_uses_matching_execution_record_lane_only() {
        let now = DateTime::parse_from_rfc3339("2026-07-04T00:00:30Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut task = make_queued_task_for_submit("active-cross-lane-fast");
        task.status = "querying".to_string();
        task.submit_id = "fast-current-submit".to_string();
        task.execution_records = vec![TaskExecutionRecord {
            id: "fast-current-record".to_string(),
            submit_id: "fast-current-submit".to_string(),
            status: "querying".to_string(),
            started_at: "2026-07-04T00:00:00Z".to_string(),
            finished_at: "2026-07-04T00:00:01Z".to_string(),
            input_snapshot: TaskExecutionInputSnapshot {
                params: VideoParams {
                    model_version: FAST_FALLBACK_MODEL_VERSION.to_string(),
                    ..VideoParams::default()
                },
                ..TaskExecutionInputSnapshot::default()
            },
            command_preview: vec![],
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: String::new(),
            error_detail: String::new(),
        }];
        let data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };

        let statuses = compute_lane_status(&data, now);
        let standard = statuses
            .iter()
            .find(|status| status.queue_kind == "standard")
            .expect("standard lane status");
        let fast = statuses
            .iter()
            .find(|status| status.queue_kind == "fast")
            .expect("fast lane status");

        assert!(!standard.is_active);
        assert!(fast.is_active);
        assert_eq!(fast.current_task_id, "active-cross-lane-fast");
    }

    #[test]
    fn retry_wait_lane_uses_newest_record_timestamp_when_records_are_out_of_order() {
        let now = DateTime::parse_from_rfc3339("2026-07-04T00:02:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut task = make_queued_task_for_submit("out-of-order-retry-records");
        task.status = "retry_wait".to_string();
        task.next_run_at = Some("2026-07-04T00:05:00Z".to_string());
        task.execution_records = vec![
            TaskExecutionRecord {
                id: "new-fast-retry".to_string(),
                submit_id: String::new(),
                status: "retry_wait".to_string(),
                started_at: "2026-07-04T00:01:00Z".to_string(),
                finished_at: "2026-07-04T00:01:10Z".to_string(),
                input_snapshot: TaskExecutionInputSnapshot {
                    params: VideoParams {
                        model_version: FAST_FALLBACK_MODEL_VERSION.to_string(),
                        ..VideoParams::default()
                    },
                    ..TaskExecutionInputSnapshot::default()
                },
                command_preview: vec![],
                query_records: vec![],
                result_paths: vec![],
                result_urls: vec![],
                error_kind: "Transient".to_string(),
                error_detail: String::new(),
            },
            TaskExecutionRecord {
                id: "old-standard-retry-appended-later".to_string(),
                submit_id: String::new(),
                status: "retry_wait".to_string(),
                started_at: "2026-07-04T00:00:00Z".to_string(),
                finished_at: "2026-07-04T00:00:10Z".to_string(),
                input_snapshot: TaskExecutionInputSnapshot::default(),
                command_preview: vec![],
                query_records: vec![],
                result_paths: vec![],
                result_urls: vec![],
                error_kind: "Transient".to_string(),
                error_detail: String::new(),
            },
        ];
        let data = AppData {
            tasks: vec![task],
            ..AppData::default()
        };

        let statuses = compute_lane_status(&data, now);
        let standard = statuses
            .iter()
            .find(|status| status.queue_kind == "standard")
            .expect("standard lane status");
        let fast = statuses
            .iter()
            .find(|status| status.queue_kind == "fast")
            .expect("fast lane status");

        assert_eq!(standard.waiting_task_count, 0);
        assert_eq!(fast.waiting_task_count, 1);
    }

    #[test]
    fn lane_status_cooldown_ignores_due_or_non_concurrency_retries() {
        let now = DateTime::parse_from_rfc3339("2026-07-04T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let cooldown_end = (now + Duration::seconds(90)).to_rfc3339();
        let transient_end = (now + Duration::seconds(10)).to_rfc3339();

        let mut concurrency = make_queued_task_for_submit("concurrency");
        concurrency.status = "retry_wait".to_string();
        concurrency.next_run_at = Some(cooldown_end.clone());
        concurrency.last_error = "ExceedConcurrencyLimit".to_string();

        let mut transient = make_queued_task_for_submit("transient");
        transient.status = "retry_wait".to_string();
        transient.next_run_at = Some(transient_end);
        transient.last_error = "temporary network error".to_string();

        let mut due_concurrency = make_queued_task_for_submit("due-concurrency");
        due_concurrency.status = "retry_wait".to_string();
        due_concurrency.next_run_at = Some((now - Duration::seconds(5)).to_rfc3339());
        due_concurrency.last_error = "ExceedConcurrencyLimit".to_string();

        let data = AppData {
            tasks: vec![transient, due_concurrency, concurrency],
            ..AppData::default()
        };
        let statuses = compute_lane_status(&data, now);
        let standard = statuses
            .iter()
            .find(|status| status.queue_kind == "standard")
            .expect("standard lane status");

        assert!(standard.is_cooling_down);
        assert_eq!(standard.next_check_at, cooldown_end);
        assert_eq!(standard.cooldown_reason, "并发限制，1 个任务等待重试");
    }

    #[test]
    fn lane_settings_default_to_both_enabled() {
        let settings = SchedulerSettings::default();
        assert!(settings.standard_lane_enabled);
        assert!(settings.fast_lane_enabled);
    }

    #[test]
    fn disabling_the_last_lane_is_rejected() {
        let mut data = AppData::default();
        data.settings.fast_lane_enabled = false;
        let error = set_lane_enabled(&mut data, ModelQueueKind::Standard, false)
            .expect_err("the final enabled lane must not be disabled");
        assert!(error.to_string().contains("至少保留一条车道"));
        assert!(data.settings.standard_lane_enabled);
        assert!(!data.settings.fast_lane_enabled);
    }
}
