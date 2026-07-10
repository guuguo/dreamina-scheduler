#![recursion_limit = "256"]

use dreamina_scheduler_lib::{
    default_store_dir, parse_mcp_queue_videos_input, parse_mcp_video_task_input,
    process_queue_for_store_blocking, queue_mcp_video_task, queue_mcp_video_tasks, AppStore,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const ARGUMENT_WRAPPER_KEYS: &[&str] = &["arguments", "args", "input", "parameters"];
const TOOL_ARGUMENT_HINT_KEYS: &[&str] = &[
    "title",
    "prompt",
    "image_paths",
    "imagePaths",
    "images",
    "image",
    "image_path",
    "imagePath",
    "audio_paths",
    "audioPaths",
    "audios",
    "audio",
    "audio_path",
    "audioPath",
    "items",
    "tasks",
    "videos",
    "defaults",
    "start_at",
    "startAt",
    "scheduled_at",
    "scheduledAt",
    "orientation",
    "aspectRatio",
    "model",
    "duration",
    "video_resolution",
    "videoResolution",
    "planned_submit_count",
    "plannedSubmitCount",
    "alternate_fast_model",
    "alternateFastModel",
];

#[derive(Debug, Serialize)]
struct ToolTextContent {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

fn main() {
    let store = AppStore::load(default_store_dir());
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("read stdin failed: {error}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                let response = json_rpc_error(Value::Null, -32700, format!("Parse error: {error}"));
                write_response(&mut stdout, &response);
                continue;
            }
        };
        if request.get("id").is_none() {
            continue;
        }
        let response = handle_request(&store, request);
        write_response(&mut stdout, &response);
    }
}

fn write_response(stdout: &mut io::Stdout, response: &Value) {
    if let Err(error) = writeln!(stdout, "{response}") {
        eprintln!("write stdout failed: {error}");
        return;
    }
    let _ = stdout.flush();
}

fn handle_request(store: &AppStore, request: Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => json_rpc_result(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "dreamina-scheduler",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "server/discover" => json_rpc_result(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "dreamina-scheduler",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "tools/list" => json_rpc_result(id, json!({ "tools": tool_definitions() })),
        "tools/call" => handle_tool_call(
            store,
            id,
            request.get("params").cloned().unwrap_or_default(),
        ),
        _ => json_rpc_error(id, -32601, format!("Method not found: {method}")),
    }
}

fn handle_tool_call(store: &AppStore, id: Value, params: Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let (arguments, argument_source) = extract_tool_arguments(&params);
    let result = match name {
        "dreamina_queue_video" => call_queue_video(store, arguments, &params, &argument_source),
        "dreamina_queue_videos" => call_queue_videos(store, arguments, &params, &argument_source),
        "dreamina_process_queue_once" => call_process_queue_once(store),
        "dreamina_get_queue_snapshot" => call_get_queue_snapshot(store),
        _ => Err(format!("Unknown tool: {name}")),
    };
    match result {
        Ok(value) => json_rpc_result(id, tool_success(value)),
        Err(message) => json_rpc_result(id, tool_error(message)),
    }
}

fn call_queue_video(
    store: &AppStore,
    arguments: Value,
    params: &Value,
    argument_source: &str,
) -> Result<Value, String> {
    let argument_hint = argument_hint(params, &arguments, argument_source);
    let input = parse_mcp_video_task_input(arguments)
        .map_err(|error| format!("{error}；{argument_hint}"))?;
    let assets_dir = store.assets_dir();
    store
        .mutate(|data| {
            queue_mcp_video_task(data, &assets_dir, input).map(|queued| {
                json!({
                    "task": queued.task,
                    "imported_assets": queued.imported_assets,
                })
            })
        })
        .map_err(|error| format!("{error}；{argument_hint}"))
}

fn call_queue_videos(
    store: &AppStore,
    arguments: Value,
    params: &Value,
    argument_source: &str,
) -> Result<Value, String> {
    let argument_hint = argument_hint(params, &arguments, argument_source);
    let input = parse_mcp_queue_videos_input(arguments)
        .map_err(|error| format!("{error}；{argument_hint}"))?;
    let assets_dir = store.assets_dir();
    store
        .mutate(|data| {
            queue_mcp_video_tasks(data, &assets_dir, input).map(|queued| {
                json!({
                    "tasks": queued.iter().map(|item| item.task.clone()).collect::<Vec<_>>(),
                    "imported_assets": queued
                        .iter()
                        .flat_map(|item| item.imported_assets.clone())
                        .collect::<Vec<_>>(),
                })
            })
        })
        .map_err(|error| format!("{error}；{argument_hint}"))
}

fn extract_tool_arguments(params: &Value) -> (Value, String) {
    for key in ARGUMENT_WRAPPER_KEYS {
        if let Some(value) = params.get(*key) {
            if is_empty_argument_value(value) {
                continue;
            }
            return (
                normalize_argument_container(value.clone()),
                (*key).to_string(),
            );
        }
    }

    if looks_like_tool_arguments(params) {
        return (params.clone(), "params".to_string());
    }

    (json!({}), "empty".to_string())
}

fn normalize_argument_container(value: Value) -> Value {
    let mut current = parse_json_object_string(value);
    for _ in 0..3 {
        if looks_like_tool_arguments(&current) {
            return current;
        }
        let Some(map) = current.as_object() else {
            return current;
        };
        let Some((_, nested)) = ARGUMENT_WRAPPER_KEYS
            .iter()
            .find_map(|key| map.get(*key).map(|value| (*key, value)))
        else {
            return current;
        };
        if is_empty_argument_value(nested) {
            return current;
        }
        current = parse_json_object_string(nested.clone());
    }
    current
}

fn parse_json_object_string(value: Value) -> Value {
    let Value::String(content) = &value else {
        return value;
    };
    serde_json::from_str::<Value>(content)
        .ok()
        .filter(Value::is_object)
        .unwrap_or(value)
}

fn looks_like_tool_arguments(value: &Value) -> bool {
    value.as_object().is_some_and(|map| {
        TOOL_ARGUMENT_HINT_KEYS
            .iter()
            .any(|key| map.contains_key(*key))
    })
}

fn is_empty_argument_value(value: &Value) -> bool {
    value.is_null()
        || value.as_object().is_some_and(|map| map.is_empty())
        || value.as_array().is_some_and(|items| items.is_empty())
}

fn argument_keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

fn argument_hint(params: &Value, arguments: &Value, argument_source: &str) -> String {
    format!(
        "MCP 参数来源={argument_source}；外层字段={}；入参字段={}；关键入参值={}",
        format_keys(argument_keys(params)),
        format_keys(argument_keys(arguments)),
        argument_value_summary(arguments)
    )
}

fn format_keys(keys: Vec<String>) -> String {
    if keys.is_empty() {
        "(无)".to_string()
    } else {
        keys.join(", ")
    }
}

fn argument_value_summary(arguments: &Value) -> String {
    const SUMMARY_KEYS: &[&str] = &[
        "image_paths",
        "imagePaths",
        "images",
        "image",
        "audio_paths",
        "audioPaths",
        "audios",
        "audio",
        "items",
        "tasks",
        "videos",
    ];
    let Some(map) = arguments.as_object() else {
        return value_shape(arguments);
    };
    let parts = SUMMARY_KEYS
        .iter()
        .filter_map(|key| {
            map.get(*key)
                .map(|value| format!("{key}={}", value_shape(value)))
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "(无图片/音频字段值)".to_string()
    } else {
        parts.join(", ")
    }
}

fn value_shape(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(text) => {
            let preview = text.chars().take(80).collect::<String>();
            format!("string(len={}, preview={preview:?})", text.chars().count())
        }
        Value::Array(items) => {
            let first = items
                .first()
                .map(value_shape)
                .unwrap_or_else(|| "empty".to_string());
            format!("array(len={}, first={first})", items.len())
        }
        Value::Object(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            if keys.len() > 12 {
                keys.truncate(12);
                keys.push("...".to_string());
            }
            format!("object(keys={})", keys.join("|"))
        }
    }
}

fn call_process_queue_once(store: &AppStore) -> Result<Value, String> {
    process_queue_for_store_blocking(store, "mcp").map(|task| {
        json!({
            "task": task,
        })
    })
}

fn call_get_queue_snapshot(store: &AppStore) -> Result<Value, String> {
    let data = store.snapshot();
    Ok(json!({
        "tasks": data.tasks,
        "assets": data.assets,
        "logs": data.logs,
    }))
}

fn tool_success(value: Value) -> Value {
    json!({
        "content": [ToolTextContent {
            kind: "text",
            text: serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string()),
        }],
        "isError": false,
    })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [ToolTextContent {
            kind: "text",
            text: message,
        }],
        "isError": true,
    })
}

fn json_rpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn json_rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "dreamina_queue_video",
            "description": "Queue one Dreamina video task from local image/audio paths. Defaults: portrait, fast model, 15s, 720p. MCP args compat v3.",
            "inputSchema": {
                "type": "object",
                "required": ["prompt"],
                "properties": {
                    "title": { "type": "string" },
                    "prompt": { "type": "string" },
                    "image_paths": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                    "imagePaths": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                    "images": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                    "audio_paths": { "type": "array", "items": { "type": "string" } },
                    "audioPaths": { "type": "array", "items": { "type": "string" } },
                    "audios": { "type": "array", "items": { "type": "string" } },
                    "start_at": { "type": "string", "description": "RFC3339 time. Omit to queue immediately." },
                    "startAt": { "type": "string", "description": "RFC3339 time. Omit to queue immediately." },
                    "orientation": { "type": "string", "enum": ["portrait", "landscape"] },
                    "model": { "type": "string", "enum": ["fast", "standard"] },
                    "duration": { "type": "integer", "minimum": 4, "maximum": 15 },
                    "video_resolution": { "type": "string", "enum": ["720p"] },
                    "videoResolution": { "type": "string", "enum": ["720p"] },
                    "planned_submit_count": { "type": "integer", "minimum": 1 },
                    "plannedSubmitCount": { "type": "integer", "minimum": 1 }
                }
            }
        },
        {
            "name": "dreamina_queue_videos",
            "description": "Queue multiple Dreamina video tasks. A shared start_at makes them enter the existing scheduler queue together. MCP args compat v3.",
            "inputSchema": {
                "type": "object",
                "required": ["items"],
                "properties": {
                    "start_at": { "type": "string", "description": "RFC3339 time applied to items without their own start_at." },
                    "defaults": {
                        "type": "object",
                        "properties": {
                            "orientation": { "type": "string", "enum": ["portrait", "landscape"] },
                            "model": { "type": "string", "enum": ["fast", "standard"] },
                            "duration": { "type": "integer", "minimum": 4, "maximum": 15 },
                            "video_resolution": { "type": "string", "enum": ["720p"] },
                            "planned_submit_count": { "type": "integer", "minimum": 1 },
                            "alternate_fast_model": { "type": "boolean", "description": "When queueing multiple videos, alternate unspecified item models as standard, fast, standard, fast." },
                            "alternateFastModel": { "type": "boolean", "description": "Alias of alternate_fast_model." }
                        }
                    },
                    "alternate_fast_model": { "type": "boolean", "description": "Alternate unspecified item models as standard, fast, standard, fast so both Dreamina queues can be used." },
                    "alternateFastModel": { "type": "boolean", "description": "Alias of alternate_fast_model." },
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["prompt"],
                            "properties": {
                                "title": { "type": "string" },
                                "prompt": { "type": "string" },
                                "image_paths": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                                "imagePaths": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                                "images": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                                "audio_paths": { "type": "array", "items": { "type": "string" } },
                                "audioPaths": { "type": "array", "items": { "type": "string" } },
                                "audios": { "type": "array", "items": { "type": "string" } },
                                "start_at": { "type": "string" },
                                "startAt": { "type": "string" },
                                "orientation": { "type": "string", "enum": ["portrait", "landscape"] },
                                "model": { "type": "string", "enum": ["fast", "standard"] },
                                "duration": { "type": "integer", "minimum": 4, "maximum": 15 },
                                "video_resolution": { "type": "string", "enum": ["720p"] },
                                "videoResolution": { "type": "string", "enum": ["720p"] },
                                "planned_submit_count": { "type": "integer", "minimum": 1 },
                                "plannedSubmitCount": { "type": "integer", "minimum": 1 }
                            }
                        }
                    },
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["prompt"],
                            "properties": {
                                "title": { "type": "string" },
                                "prompt": { "type": "string" },
                                "imagePaths": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                                "images": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                                "audioPaths": { "type": "array", "items": { "type": "string" } }
                            }
                        }
                    },
                    "videos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["prompt"],
                            "properties": {
                                "title": { "type": "string" },
                                "prompt": { "type": "string" },
                                "image_paths": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                                "imagePaths": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                                "images": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                                "audio_paths": { "type": "array", "items": { "type": "string" } },
                                "audioPaths": { "type": "array", "items": { "type": "string" } },
                                "audios": { "type": "array", "items": { "type": "string" } },
                                "start_at": { "type": "string" },
                                "startAt": { "type": "string" },
                                "orientation": { "type": "string", "enum": ["portrait", "landscape"] },
                                "model": { "type": "string", "enum": ["fast", "standard"] },
                                "duration": { "type": "integer", "minimum": 4, "maximum": 15 },
                                "video_resolution": { "type": "string", "enum": ["720p"] },
                                "videoResolution": { "type": "string", "enum": ["720p"] },
                                "planned_submit_count": { "type": "integer", "minimum": 1 },
                                "plannedSubmitCount": { "type": "integer", "minimum": 1 }
                            }
                        }
                    }
                }
            }
        },
        {
            "name": "dreamina_process_queue_once",
            "description": "Process one due queued task or one query step using the existing scheduler core.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "dreamina_get_queue_snapshot",
            "description": "Return the current scheduler state snapshot: tasks, assets, and logs.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tool_arguments_accepts_standard_arguments() {
        let params = json!({
            "name": "dreamina_queue_video",
            "arguments": {
                "prompt": "第二幕",
                "images": ["/tmp/act2.png"]
            }
        });

        let (arguments, source) = extract_tool_arguments(&params);

        assert_eq!(source, "arguments");
        assert_eq!(arguments["images"][0], "/tmp/act2.png");
    }

    #[test]
    fn extract_tool_arguments_accepts_alternate_wrappers() {
        for wrapper in ["args", "input", "parameters"] {
            let params = json!({
                "name": "dreamina_queue_video",
                wrapper: {
                    "prompt": "第二幕",
                    "imagePaths": ["/tmp/act2.png"]
                }
            });

            let (arguments, source) = extract_tool_arguments(&params);

            assert_eq!(source, wrapper);
            assert_eq!(arguments["imagePaths"][0], "/tmp/act2.png");
        }
    }

    #[test]
    fn extract_tool_arguments_accepts_flat_params() {
        let params = json!({
            "name": "dreamina_queue_video",
            "prompt": "第二幕",
            "image_paths": ["/tmp/act2.png"]
        });

        let (arguments, source) = extract_tool_arguments(&params);

        assert_eq!(source, "params");
        assert_eq!(arguments["image_paths"][0], "/tmp/act2.png");
    }

    #[test]
    fn extract_tool_arguments_accepts_nested_and_json_string_payloads() {
        let params = json!({
            "name": "dreamina_queue_video",
            "arguments": {
                "input": "{\"prompt\":\"第二幕\",\"images\":[\"/tmp/act2.png\"]}"
            }
        });

        let (arguments, source) = extract_tool_arguments(&params);

        assert_eq!(source, "arguments");
        assert_eq!(arguments["images"][0], "/tmp/act2.png");
    }
}
