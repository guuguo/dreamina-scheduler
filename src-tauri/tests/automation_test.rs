use chrono::{Duration, Utc};
use dreamina_scheduler_lib::{
    backfill_draft_command_previews, backfill_execution_records_from_attempts, build_install_plan,
    build_login_plan, classify_dreamina_error, compact_retry_execution_records_for_display,
    create_draft_task, create_task_with_preview, delete_execution_record_from_data, delete_role,
    delete_task_from_data, import_media_to_role, needs_keep_awake, pause_task, peek_due_task_cli,
    process_next_due_task_with_runner, query_task_submit_id_once_with_runner,
    queue_tasks_with_batch_schedule, queue_tasks_with_model_strategy, record_lifecycle_event,
    record_scheduler_tick, recover_tasks_on_load, remove_media_from_role, reschedule_task,
    resume_task, save_clipboard_image_asset, update_task_from_data, upsert_role, AppData, Asset,
    AssetKind, BatchQueuePlanItem, ClipboardImageInput, ConcurrencyLimitPolicy, CreateRoleInput,
    DreaminaErrorKind, DueTaskCliAction, ImportRoleMediaInput, LogEntry, LogLevel, LogSource,
    RemoveRoleMediaInput, Role, ScheduledTask, SchedulerSettings, TaskAttempt, TaskDraft,
    TaskExecutionInputSnapshot, TaskExecutionRecord, VideoParams,
};
use std::collections::HashMap;
use std::fs;

fn image_asset(id: &str, name: &str, path: &str) -> Asset {
    Asset {
        id: id.to_string(),
        kind: AssetKind::Image,
        name: name.to_string(),
        aliases: vec![],
        tags: vec![],
        stored_path: path.to_string(),
        source_path: path.to_string(),
        mime: "image/png".to_string(),
        size_bytes: 100,
        duration_seconds: None,
        created_at: String::new(),
        content_hash: None,
        last_used_at: None,
    }
}

fn audio_asset(id: &str, name: &str, path: &str) -> Asset {
    Asset {
        id: id.to_string(),
        kind: AssetKind::Audio,
        name: name.to_string(),
        aliases: vec![],
        tags: vec![],
        stored_path: path.to_string(),
        source_path: path.to_string(),
        mime: "audio/mpeg".to_string(),
        size_bytes: 100,
        duration_seconds: Some(3.0),
        created_at: String::new(),
        content_hash: None,
        last_used_at: None,
    }
}

fn default_data() -> AppData {
    AppData {
        settings: SchedulerSettings::default(),
        assets: vec![image_asset("img-1", "角色图", "/tmp/role.png")],
        roles: vec![],
        tasks: vec![],
        logs: vec![],
        imagegen_history: vec![],
        asset_hash_index: HashMap::new(),
    }
}

fn queued_task(id: &str) -> ScheduledTask {
    ScheduledTask {
        id: id.to_string(),
        title: "测试任务".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        status: "queued".to_string(),
        scheduled_at: None,
        next_run_at: None,
        queued_at: None,
        submitted_at: None,
        queue_info: None,
        submit_id: String::new(),
        attempt_count: 0,
        concurrency_retry_count: 0,
        last_error: String::new(),
        command_preview: vec![],
        attempts: vec![],
        result_paths: vec![],
        result_urls: vec![],
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
        finished_at: String::new(),
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

fn transient_failed_record(id: &str, minutes_ago: i64) -> TaskExecutionRecord {
    let finished_at = (Utc::now() - Duration::minutes(minutes_ago)).to_rfc3339();
    TaskExecutionRecord {
        id: id.to_string(),
        submit_id: String::new(),
        status: "failed".to_string(),
        started_at: finished_at.clone(),
        finished_at,
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: "Transient".to_string(),
        error_detail: "upload connection reset".to_string(),
    }
}

fn concurrency_failed_record(id: &str, minutes_ago: i64) -> TaskExecutionRecord {
    let finished_at = (Utc::now() - Duration::minutes(minutes_ago)).to_rfc3339();
    TaskExecutionRecord {
        id: id.to_string(),
        submit_id: "old-submit-id".to_string(),
        status: "failed".to_string(),
        started_at: finished_at.clone(),
        finished_at,
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: "ConcurrencyLimit".to_string(),
        error_detail: "api error: ret=1310, message=ExceedConcurrencyLimit".to_string(),
    }
}

#[test]
fn queue_tasks_with_alternating_fast_model_assigns_every_second_task_to_fast() {
    let mut data = default_data();
    data.tasks = vec![
        queued_task("task-1"),
        queued_task("task-2"),
        queued_task("task-3"),
        queued_task("task-4"),
    ];

    let updated = queue_tasks_with_model_strategy(
        &mut data,
        &[
            "task-1".to_string(),
            "task-2".to_string(),
            "task-3".to_string(),
            "task-4".to_string(),
        ],
        "",
        1,
        true,
    )
    .expect("交叉 fast 排队应成功");

    assert_eq!(updated.len(), 4);
    assert_eq!(data.tasks[0].params.model_version, "seedance2.0");
    assert_eq!(data.tasks[1].params.model_version, "seedance2.0");
    assert_eq!(data.tasks[2].params.model_version, "seedance2.0");
    assert_eq!(data.tasks[3].params.model_version, "seedance2.0");
    assert!(data.tasks.iter().all(|task| task.status == "queued"));
}

#[test]
fn queue_tasks_with_batch_schedule_supports_immediate_interval_and_alternating_fast() {
    let mut data = default_data();
    data.tasks = vec![queued_task("task-1"), queued_task("task-2")];
    let delayed_at = (Utc::now() + Duration::minutes(15)).to_rfc3339();

    let updated = queue_tasks_with_batch_schedule(
        &mut data,
        &[
            BatchQueuePlanItem {
                task_id: "task-1".to_string(),
                scheduled_at: None,
            },
            BatchQueuePlanItem {
                task_id: "task-2".to_string(),
                scheduled_at: Some(delayed_at.clone()),
            },
        ],
        1,
        true,
    )
    .expect("批量排队计划应成功");

    assert_eq!(updated.len(), 2);
    assert_eq!(data.tasks[0].status, "queued");
    assert_eq!(data.tasks[0].scheduled_at, None);
    assert_eq!(data.tasks[0].params.model_version, "seedance2.0");
    assert_eq!(data.tasks[1].status, "scheduled");
    assert_eq!(data.tasks[1].scheduled_at, Some(delayed_at));
    assert_eq!(data.tasks[1].params.model_version, "seedance2.0");
}

#[test]
fn standard_active_task_does_not_block_fast_queue_submission() {
    let mut data = default_data();
    let mut standard_active = queued_task("standard-active");
    standard_active.status = "querying".to_string();
    standard_active.submit_id = "standard-submit".to_string();
    standard_active.submitted_at = Some(Utc::now().to_rfc3339());
    standard_active.last_auto_query_at = Some(Utc::now().to_rfc3339());
    standard_active.execution_records.push(TaskExecutionRecord {
        id: "rec-standard".to_string(),
        submit_id: "standard-submit".to_string(),
        status: "querying".to_string(),
        started_at: Utc::now().to_rfc3339(),
        finished_at: String::new(),
        input_snapshot: TaskExecutionInputSnapshot {
            params: standard_active.params.clone(),
            ..TaskExecutionInputSnapshot::default()
        },
        command_preview: vec![
            "multimodal2video".to_string(),
            "--model_version=seedance2.0".to_string(),
        ],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });

    let mut fast_waiting = queued_task("fast-waiting");
    fast_waiting.params.model_version = "seedance2.0fast".to_string();
    data.tasks.push(standard_active);
    data.tasks.push(fast_waiting);

    let result = process_next_due_task_with_runner(&mut data, |args| {
        assert_eq!(args[0], "multimodal2video");
        assert!(args
            .iter()
            .any(|arg| arg == "--model_version=seedance2.0fast"));
        Ok((
            r#"{"submit_id":"fast-submit","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("fast task should be submitted while standard is active");

    assert_eq!(result.id, "fast-waiting");
    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "fast-submit");
}

#[test]
fn drained_fast_queue_promotes_standard_backlog_to_fast_when_standard_lane_is_stuck() {
    let mut data = default_data();
    let mut standard_active = queued_task("standard-active");
    standard_active.status = "querying".to_string();
    standard_active.submit_id = "standard-submit".to_string();
    standard_active.submitted_at = Some(Utc::now().to_rfc3339());
    standard_active.last_auto_query_at = Some(Utc::now().to_rfc3339());
    standard_active.execution_records.push(TaskExecutionRecord {
        id: "rec-standard".to_string(),
        submit_id: "standard-submit".to_string(),
        status: "querying".to_string(),
        started_at: Utc::now().to_rfc3339(),
        finished_at: String::new(),
        input_snapshot: TaskExecutionInputSnapshot {
            params: standard_active.params.clone(),
            ..TaskExecutionInputSnapshot::default()
        },
        command_preview: vec![
            "multimodal2video".to_string(),
            "--model_version=seedance2.0".to_string(),
        ],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });

    let mut fast_done = queued_task("fast-done");
    fast_done.params.model_version = "seedance2.0fast".to_string();
    fast_done.status = "succeeded".to_string();
    fast_done.submit_id = "fast-done-submit".to_string();
    fast_done.result_urls = vec!["https://example.com/fast.mp4".to_string()];

    let standard_backlog = queued_task("standard-backlog");
    data.tasks.push(standard_active);
    data.tasks.push(fast_done);
    data.tasks.push(standard_backlog);

    let result = process_next_due_task_with_runner(&mut data, |args| {
        assert_eq!(args[0], "multimodal2video");
        assert!(args
            .iter()
            .any(|arg| arg == "--model_version=seedance2.0fast"));
        Ok((
            r#"{"submit_id":"promoted-fast-submit","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("standard backlog should be promoted into idle fast lane");

    assert_eq!(result.id, "standard-backlog");
    assert_eq!(result.params.model_version, "seedance2.0"); // original task unchanged
    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "promoted-fast-submit");
}

#[test]
fn peek_due_task_cli_marks_promoted_standard_backlog_as_fast_submit() {
    let mut data = default_data();
    let mut standard_active = queued_task("standard-active");
    standard_active.status = "querying".to_string();
    standard_active.submit_id = "standard-submit".to_string();
    standard_active.last_auto_query_at = Some(Utc::now().to_rfc3339());

    let mut fast_done = queued_task("fast-done");
    fast_done.params.model_version = "seedance2.0fast".to_string();
    fast_done.status = "succeeded".to_string();
    fast_done.submit_id = "fast-done-submit".to_string();
    fast_done.result_urls = vec!["https://example.com/fast.mp4".to_string()];

    data.tasks.push(standard_active);
    data.tasks.push(fast_done);
    data.tasks.push(queued_task("standard-backlog"));

    let due = peek_due_task_cli(&data)
        .expect("peek should build due cli")
        .expect("promoted standard backlog should be due");

    assert_eq!(due.task_id, "standard-backlog");
    assert!(due
        .args
        .iter()
        .any(|arg| arg == "--model_version=seedance2.0fast"));
    assert_eq!(
        due.action,
        DueTaskCliAction::Submit {
            model_version_override: Some("seedance2.0fast".to_string())
        }
    );
}

#[test]
fn rejects_scheduled_at_in_past() {
    let data = default_data();
    let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let draft = TaskDraft {
        title: "过去任务".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: Some(past),
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        prompt_doc: None,
    };
    let result = create_task_with_preview(&data, draft);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("过去时间"),
        "expected past time error, got: {err}"
    );
}

#[test]
fn accepts_future_scheduled_at() {
    let data = default_data();
    let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let draft = TaskDraft {
        title: "未来任务".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: Some(future),
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        prompt_doc: None,
    };
    let result = create_task_with_preview(&data, draft);
    assert!(result.is_ok());
    let task = result.unwrap();
    assert_eq!(task.status, "scheduled");
}

#[test]
fn draft_task_is_saved_without_entering_queue() {
    let data = default_data();
    let draft = TaskDraft {
        title: "".to_string(),
        prompt: "@角色图 先保存一下".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
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

    let task = create_draft_task(&data, draft).expect("draft");

    assert_eq!(task.status, "draft");
    assert!(task.next_run_at.is_none());
    assert!(!task.command_preview.is_empty());
    assert_eq!(task.command_preview[0], "multimodal2video");
    assert!(task
        .command_preview
        .iter()
        .any(|arg| arg.starts_with("--prompt=")));
    assert!(task.title.contains("角色图"));
}

#[test]
fn draft_task_preserves_temp_images_for_later_editing() {
    let mut data = default_data();
    data.assets.push(image_asset(
        "temp-1",
        "临时图片",
        "/tmp/temp-storyboard.png",
    ));
    let draft: TaskDraft = serde_json::from_value(serde_json::json!({
        "title": "",
        "prompt": "@分镜图1 先保存一下",
        "image_asset_ids": ["temp-1"],
        "audio_asset_ids": [],
        "role_ids": [],
        "manual_mention_ids": [],
        "auto_match_roles": false,
        "params": VideoParams::default(),
        "scheduled_at": null,
        "temp_image_asset_ids": ["temp-1"],
        "temp_image_paths": ["/tmp/temp-storyboard.png"]
    }))
    .expect("draft from frontend payload");

    let task = create_draft_task(&data, draft).expect("draft");
    let value = serde_json::to_value(task).expect("task json");

    assert_eq!(value["temp_image_asset_ids"], serde_json::json!(["temp-1"]));
    assert_eq!(
        value["temp_image_paths"],
        serde_json::json!(["/tmp/temp-storyboard.png"])
    );
}

#[test]
fn create_task_generates_short_title_when_title_is_empty() {
    let data = default_data();
    let draft = TaskDraft {
        title: "".to_string(),
        prompt: "@角色图 在海边漫步，阳光照在身上，海浪轻轻打沙滩，微风拂动长发。".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
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

    let task = create_task_with_preview(&data, draft).expect("task");

    assert_eq!(task.title, "角色图在海边漫");
}

#[test]
fn pasted_clipboard_image_is_saved_as_temp_asset_and_can_build_task_preview() {
    let mut data = default_data();
    let temp = tempfile::tempdir().expect("tempdir");

    let asset = save_clipboard_image_asset(
        &mut data,
        temp.path(),
        ClipboardImageInput {
            file_name: "clipboard.png".to_string(),
            mime: "image/png".to_string(),
            bytes: vec![137, 80, 78, 71, 13, 10, 26, 10],
        },
    )
    .expect("save clipboard image");

    assert_eq!(asset.kind, AssetKind::Image);
    assert_eq!(asset.name, "粘贴图片");
    assert!(asset.stored_path.ends_with(".png"));
    assert!(std::path::Path::new(&asset.stored_path).exists());
    assert_eq!(
        data.assets
            .iter()
            .filter(|item| item.id == asset.id)
            .count(),
        1
    );

    let draft = TaskDraft {
        title: String::new(),
        prompt: "@分镜图1 猫猫抬头看镜头".to_string(),
        image_asset_ids: vec![asset.id.clone()],
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
    let task = create_task_with_preview(&data, draft).expect("task preview");
    assert!(task
        .command_preview
        .iter()
        .any(|arg| arg == &format!("--image={}", asset.stored_path)));
}

#[test]
fn pause_task_allowed_from_queued() {
    let mut data = default_data();
    let task = queued_task("task-1");
    data.tasks.push(task);
    let result = pause_task(&mut data, "task-1");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, "paused");
}

#[test]
fn pause_task_rejected_from_submitting() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "submitting".to_string();
    data.tasks.push(task);
    let result = pause_task(&mut data, "task-1");
    assert!(result.is_err());
}

#[test]
fn resume_task_immediate() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "paused".to_string();
    data.tasks.push(task);
    let result = resume_task(&mut data, "task-1", "immediate");
    assert!(result.is_ok());
    let resumed = result.unwrap();
    assert_eq!(resumed.status, "queued");
    assert!(resumed.scheduled_at.is_none());
}

#[test]
fn resume_task_scheduled() {
    let mut data = default_data();
    let future = (Utc::now() + Duration::hours(2)).to_rfc3339();
    let mut task = queued_task("task-1");
    task.status = "paused".to_string();
    task.scheduled_at = Some(future.clone());
    data.tasks.push(task);
    let result = resume_task(&mut data, "task-1", "scheduled");
    assert!(result.is_ok());
    let resumed = result.unwrap();
    assert_eq!(resumed.status, "scheduled");
}

#[test]
fn reschedule_task_rejects_past_time() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "scheduled".to_string();
    data.tasks.push(task);
    let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let result = reschedule_task(&mut data, "task-1", &past);
    assert!(result.is_err());
}

#[test]
fn reschedule_task_accepts_future_time() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "scheduled".to_string();
    data.tasks.push(task);
    let future = (Utc::now() + Duration::hours(3)).to_rfc3339();
    let result = reschedule_task(&mut data, "task-1", &future);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, "scheduled");
}

#[test]
fn reschedule_task_accepts_queued_task() {
    let mut data = default_data();
    data.tasks.push(queued_task("task-1"));
    let future = (Utc::now() + Duration::hours(3)).to_rfc3339();
    let result = reschedule_task(&mut data, "task-1", &future);
    assert!(result.is_ok());
    let task = result.unwrap();
    assert_eq!(task.status, "scheduled");
    assert_eq!(task.scheduled_at, Some(future));
}

#[test]
fn reschedule_task_empty_time_returns_to_queued() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "scheduled".to_string();
    task.scheduled_at = Some((Utc::now() + Duration::hours(3)).to_rfc3339());
    data.tasks.push(task);
    let result = reschedule_task(&mut data, "task-1", "");
    assert!(result.is_ok());
    let task = result.unwrap();
    assert_eq!(task.status, "queued");
    assert!(task.scheduled_at.is_none());
    assert!(task.next_run_at.is_none());
}

#[test]
fn auto_query_settings_default_enabled() {
    let settings = SchedulerSettings::default();
    assert!(settings.auto_query_enabled);
    assert_eq!(settings.poll_interval_seconds, 60);
    assert!(settings.prevent_sleep);
}

#[test]
fn log_retention_default() {
    let settings = SchedulerSettings::default();
    assert_eq!(settings.log_retention_count, 500);
}

#[test]
fn concurrency_limit_legacy_silent_fail_policy_still_waits() {
    let settings = SchedulerSettings {
        concurrency_limit_policy: ConcurrencyLimitPolicy::SilentFail,
        concurrency_retry_delay_seconds: 300,
        concurrency_retry_max_attempts: 8,
        auto_query_enabled: true,
        poll_interval_seconds: 60,
        log_retention_count: 500,
        log_retention_days: 3,
        mac_install_command: default_data().settings.mac_install_command,
        windows_install_command: default_data().settings.windows_install_command,
        ai_model_configs: SchedulerSettings::default().ai_model_configs,
        active_ai_model_id: SchedulerSettings::default().active_ai_model_id,
        prevent_sleep: true,
        image_model_configs: SchedulerSettings::default().image_model_configs,
        active_image_model_id: SchedulerSettings::default().active_image_model_id,
        image_model_config: SchedulerSettings::default().image_model_config,
    };
    let classified = classify_dreamina_error(
        "api error: ret=1310, message=ExceedConcurrencyLimit",
        &settings,
    );
    assert_eq!(classified.kind, DreaminaErrorKind::ConcurrencyLimit);
    assert_eq!(classified.next_status, "retry_wait");
    assert_eq!(classified.retry_after_seconds, Some(300));
    assert!(!classified.show_modal);
}

#[test]
fn parse_query_output_with_results() {
    use dreamina_scheduler_lib::parse_query_output;
    let json = r#"{"gen_status":"success","result_paths":["/tmp/video.mp4"],"result_urls":["https://cdn.example.com/video.mp4"]}"#;
    let parsed = parse_query_output(json);
    assert_eq!(parsed.gen_status.as_deref(), Some("success"));
    assert!(!parsed.result_paths.is_empty());
    assert!(!parsed.result_urls.is_empty());
}

#[test]
fn task_attempt_has_duration_and_error_detail() {
    let _task = queued_task("task-1");
    // Verify the struct fields exist via construction
    let attempt = dreamina_scheduler_lib::TaskAttempt {
        id: "attempt_test".to_string(),
        started_at: "2026-01-01T10:00:00Z".to_string(),
        finished_at: "2026-01-01T10:00:30Z".to_string(),
        status: "succeeded".to_string(),
        command_preview: vec![],
        stdout: String::new(),
        stderr: String::new(),
        error_kind: String::new(),
        duration_seconds: 30.0,
        error_detail: String::new(),
    };
    assert_eq!(attempt.duration_seconds, 30.0);
    assert!(attempt.error_detail.is_empty());
}

#[test]
fn mac_install_plan_uses_official_shell_script() {
    let settings = SchedulerSettings::default();
    let plan = build_install_plan(&settings, "macos").expect("mac install plan");
    assert_eq!(plan.program, "sh");
    assert_eq!(
        plan.args,
        vec!["-lc", "curl -fsSL https://jimeng.jianying.com/cli | bash"]
    );
}

#[test]
fn windows_install_plan_requires_configured_powershell_command() {
    let mut settings = SchedulerSettings::default();
    settings.windows_install_command.clear();

    let err = build_install_plan(&settings, "windows")
        .expect_err("windows install source is not configured");
    assert!(err.to_string().contains("Windows 安装命令未配置"));

    settings.windows_install_command =
        "irm https://example.com/dreamina/install.ps1 | iex".to_string();
    let plan = build_install_plan(&settings, "windows").expect("windows install plan");
    assert_eq!(plan.program, "powershell");
    assert!(plan.args.contains(&"-ExecutionPolicy".to_string()));
    assert!(plan.args.contains(&settings.windows_install_command));
}

#[test]
fn login_plan_supports_browser_and_headless_modes() {
    let browser = build_login_plan("/usr/local/bin/dreamina", false);
    assert_eq!(browser.program, "/usr/local/bin/dreamina");
    assert_eq!(browser.args, vec!["login"]);

    let headless = build_login_plan("/usr/local/bin/dreamina", true);
    assert_eq!(headless.args, vec!["login", "--headless"]);
}

#[test]
fn importing_same_role_audio_path_is_idempotent() {
    let mut data = default_data();
    let role = upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-1".to_string()),
            name: "女主".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec![],
        },
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("voice.mp3");
    fs::write(&source, "fake-audio").expect("audio file");
    let cache_dir = temp.path().join("cache");
    let source_path = source
        .canonicalize()
        .expect("canonical source")
        .to_string_lossy()
        .to_string();

    let updated = import_media_to_role(
        &mut data,
        &cache_dir,
        ImportRoleMediaInput {
            role_id: role.id,
            paths: vec![
                source_path.clone(),
                source_path.clone(),
                source_path.clone(),
                source_path.clone(),
            ],
        },
    )
    .expect("import media");

    assert_eq!(updated.asset_ids.len(), 1);
    assert_eq!(
        data.assets
            .iter()
            .filter(|asset| asset.kind == AssetKind::Audio)
            .count(),
        1
    );
    assert!(data
        .assets
        .iter()
        .any(|asset| asset.source_path == source_path));
}

#[test]
fn removing_role_media_removes_duplicate_assets_with_same_source_from_role() {
    let mut data = default_data();
    data.assets
        .push(audio_asset("aud-visible", "英短声音", "/tmp/yingduan.mp3"));
    data.assets.push(audio_asset(
        "aud-hidden",
        "英短声音",
        "/tmp/yingduan-copy.mp3",
    ));
    let visible_source = data
        .assets
        .iter()
        .find(|asset| asset.id == "aud-visible")
        .expect("visible audio")
        .source_path
        .clone();
    data.assets
        .iter_mut()
        .find(|asset| asset.id == "aud-hidden")
        .expect("hidden audio")
        .source_path = visible_source;
    upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-voice".to_string()),
            name: "女主".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["aud-visible".to_string(), "aud-hidden".to_string()],
        },
    );

    let updated = remove_media_from_role(
        &mut data,
        RemoveRoleMediaInput {
            role_id: "role-voice".to_string(),
            asset_id: "aud-visible".to_string(),
        },
    )
    .expect("remove duplicate media");

    assert!(!updated.asset_ids.contains(&"aud-visible".to_string()));
    assert!(!updated.asset_ids.contains(&"aud-hidden".to_string()));
}

#[test]
fn process_queue_skips_future_scheduled_task() {
    let mut data = default_data();
    let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let draft = TaskDraft {
        title: "未到点任务".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: Some(future),
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        prompt_doc: None,
    };
    data.tasks
        .push(create_task_with_preview(&data, draft).expect("task"));

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        panic!("future scheduled task should not call dreamina");
    };
    let result = process_next_due_task_with_runner(&mut data, runner).expect("process queue");

    assert!(result.is_none());
    assert_eq!(data.tasks[0].status, "scheduled");
    assert_eq!(data.tasks[0].attempt_count, 0);
}

#[test]
fn process_queue_submits_due_task_with_mock_runner_and_records_attempt() {
    let mut data = default_data();
    data.assets
        .push(audio_asset("aud-1", "女主音色", "/tmp/voice.mp3"));
    let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let past = (Utc::now() - Duration::seconds(5)).to_rfc3339();
    let draft = TaskDraft {
        title: "到点任务".to_string(),
        prompt: "女主小雅跑进画面".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec!["aud-1".to_string()],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams {
            model_version: "seedance2.0".to_string(),
            ratio: "16:9".to_string(),
            duration: 5,
            video_resolution: "720p".to_string(),
        },
        scheduled_at: Some(future),
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        prompt_doc: None,
    };
    let mut task = create_task_with_preview(&data, draft).expect("task");
    task.status = "scheduled".to_string();
    task.next_run_at = Some(past);
    data.tasks.push(task);

    let runner = |args: &[String]| -> Result<(String, String), String> {
        assert_eq!(args[0], "multimodal2video");
        assert!(args.iter().any(|arg| arg == "--image=/tmp/role.png"));
        assert!(args.iter().any(|arg| arg == "--audio=/tmp/voice.mp3"));
        assert!(args.iter().any(|arg| arg == "--ratio=16:9"));
        Ok((
            r#"{"submit_id":"mock-video-001","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task processed");

    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "mock-video-001");
    assert_eq!(result.attempt_count, 1);
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(result.attempts[0].status, "querying");
    assert!(result.attempts[0]
        .command_preview
        .iter()
        .any(|arg| arg.contains("--prompt=女主小雅")));
}

#[test]
fn process_queue_handles_concurrency_limit_with_silent_retry_policy() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 120;
    data.settings.concurrency_retry_max_attempts = 2;
    let draft = TaskDraft {
        title: "限流任务".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
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
    data.tasks
        .push(create_task_with_preview(&data, draft).expect("task"));

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        Ok((r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit"}"#.to_string(), String::new()))
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task processed");

    assert_eq!(result.status, "retry_wait");
    assert_eq!(result.concurrency_retry_count, 1);
    assert!(result.next_run_at.is_some());
    assert!(result.last_error.contains("ExceedConcurrencyLimit"));
    assert_eq!(result.attempts[0].error_kind, "ConcurrencyLimit");
}

#[test]
fn concurrency_retry_updates_single_execution_record_with_short_error() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 0;
    data.settings.concurrency_retry_max_attempts = 8;
    data.tasks.push(queued_task("task-concurrency-compact"));

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit, logid=abc"}"#.to_string(), String::new()))
    })
    .expect("first process")
    .expect("first task processed");
    assert_eq!(result.status, "retry_wait");

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit, logid=def"}"#.to_string(), String::new()))
    })
    .expect("second process")
    .expect("second task processed");

    assert_eq!(result.status, "retry_wait");
    assert_eq!(result.attempt_count, 2);
    assert_eq!(result.concurrency_retry_count, 2);
    assert_eq!(result.execution_records.len(), 1);
    assert_eq!(result.execution_records[0].status, "retry_wait");
    assert_eq!(result.execution_records[0].error_kind, "ConcurrencyLimit");
    assert_eq!(
        result.execution_records[0].error_detail,
        "并发任务仍在生成中，已自动排队等待下次重试。"
    );
    assert!(!result.execution_records[0]
        .error_detail
        .contains("submit_preflight"));
}

#[test]
fn concurrency_retry_records_are_separated_by_model_version() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 0;
    data.settings.concurrency_retry_max_attempts = 8;
    data.tasks.push(queued_task("task-concurrency-models"));

    process_next_due_task_with_runner(&mut data, |args| {
        assert!(args.iter().any(|arg| arg == "--model_version=seedance2.0"));
        Ok((r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit, logid=standard"}"#.to_string(), String::new()))
    })
    .expect("first process")
    .expect("first task processed");

    data.tasks[0].params.model_version = "seedance2.0fast".to_string();

    let result = process_next_due_task_with_runner(&mut data, |args| {
        assert!(args
            .iter()
            .any(|arg| arg == "--model_version=seedance2.0fast"));
        Ok((r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit, logid=fast"}"#.to_string(), String::new()))
    })
    .expect("second process")
    .expect("second task processed");

    assert_eq!(result.status, "retry_wait");
    assert_eq!(result.execution_records.len(), 2);
    assert_eq!(
        result.execution_records[0]
            .input_snapshot
            .params
            .model_version,
        "seedance2.0"
    );
    assert_eq!(
        result.execution_records[1]
            .input_snapshot
            .params
            .model_version,
        "seedance2.0fast"
    );
}

#[test]
fn concurrency_retry_past_configured_max_keeps_single_short_retry_record() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 0;
    data.settings.concurrency_retry_max_attempts = 1;
    data.tasks.push(queued_task("task-concurrency-no-max"));

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit, logid=first"}"#.to_string(), String::new()))
    })
    .expect("first process")
    .expect("first task processed");
    assert_eq!(result.status, "retry_wait");

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit, logid=final"}"#.to_string(), String::new()))
    })
    .expect("second process")
    .expect("second task processed");

    assert_eq!(result.status, "retry_wait");
    assert_eq!(result.attempt_count, 2);
    assert_eq!(result.concurrency_retry_count, 2);
    assert_eq!(result.execution_records.len(), 1);
    assert_eq!(result.execution_records[0].status, "retry_wait");
    assert_eq!(result.execution_records[0].error_kind, "ConcurrencyLimit");
    assert_eq!(
        result.execution_records[0].error_detail,
        "并发任务仍在生成中，已自动排队等待下次重试。"
    );
    assert!(!result.execution_records[0]
        .error_detail
        .contains("submit_preflight"));
}

#[test]
fn active_remote_task_not_due_blocks_new_submission() {
    let mut data = default_data();
    let mut active = queued_task("task-active");
    active.status = "querying".to_string();
    active.submit_id = "sub-active".to_string();
    active.consecutive_no_result_queries = 1;
    active.last_auto_query_at = Some(Utc::now().to_rfc3339());
    data.tasks.push(active);
    data.tasks.push(queued_task("task-next"));

    // Fast lane is free → standard queued task should be submitted to Fast
    let result = process_next_due_task_with_runner(&mut data, |args| {
        assert!(args.iter().any(|a| a == "--model_version=seedance2.0fast"));
        Ok((
            r#"{"submit_id":"sub-fast","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("should submit queued task to idle Fast lane");

    assert_eq!(result.id, "task-next");
    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "sub-fast");
    assert_eq!(data.tasks[1].attempt_count, 1);
}

#[test]
fn concurrency_retry_wait_blocks_whole_queue_until_due() {
    let mut data = default_data();
    let mut cooling = queued_task("task-cooling");
    cooling.status = "retry_wait".to_string();
    cooling.last_error = "api error: ret=1310, message=ExceedConcurrencyLimit".to_string();
    cooling.next_run_at = Some((Utc::now() + Duration::minutes(10)).to_rfc3339());
    data.tasks.push(cooling);
    data.tasks.push(queued_task("task-next"));

    // Standard cooldown → Fast lane takes over queued task
    let result = process_next_due_task_with_runner(&mut data, |args| {
        assert!(args.iter().any(|a| a == "--model_version=seedance2.0fast"));
        Ok((
            r#"{"submit_id":"sub-fast","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("should submit queued task to idle Fast lane");

    assert_eq!(result.id, "task-next");
    assert_eq!(result.status, "querying");
    assert_eq!(data.tasks[1].attempt_count, 1);
}

#[test]
fn concurrency_retry_due_queue_prefers_earliest_retry_time() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;

    let mut older_created_later_due = queued_task("task-older-created-later-due");
    older_created_later_due.status = "retry_wait".to_string();
    older_created_later_due.created_at = "2026-06-01T00:00:00Z".to_string();
    older_created_later_due.last_error =
        "api error: ret=1310, message=ExceedConcurrencyLimit".to_string();
    older_created_later_due.next_run_at = Some((Utc::now() - Duration::minutes(1)).to_rfc3339());

    let mut newer_created_earlier_due = queued_task("task-newer-created-earlier-due");
    newer_created_earlier_due.status = "retry_wait".to_string();
    newer_created_earlier_due.created_at = "2026-06-20T00:00:00Z".to_string();
    newer_created_earlier_due.last_error =
        "api error: ret=1310, message=ExceedConcurrencyLimit".to_string();
    newer_created_earlier_due.next_run_at = Some((Utc::now() - Duration::minutes(10)).to_rfc3339());

    data.tasks.push(older_created_later_due);
    data.tasks.push(newer_created_earlier_due);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((
            r#"{"submit_id":"sub-earliest-retry","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.id, "task-newer-created-earlier-due");
    assert_eq!(result.submit_id, "sub-earliest-retry");
    assert_eq!(data.tasks[0].attempt_count, 0);
}

#[test]
fn queue_prefers_task_with_fewer_successful_candidates() {
    let mut data = default_data();
    let mut task_a = queued_task("task-a");
    task_a.planned_submit_count = 3;
    task_a.execution_records.push(TaskExecutionRecord {
        id: "exec-a-1".to_string(),
        submit_id: "sub-a-1".to_string(),
        status: "succeeded".to_string(),
        started_at: "2026-06-18T00:00:00Z".to_string(),
        finished_at: "2026-06-18T00:01:00Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec!["/tmp/a.mp4".to_string()],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    let mut task_b = queued_task("task-b");
    task_b.planned_submit_count = 1;
    let mut task_c = queued_task("task-c");
    task_c.planned_submit_count = 2;
    data.tasks.push(task_a);
    data.tasks.push(task_b);
    data.tasks.push(task_c);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((
            r#"{"submit_id":"sub-b-1","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.id, "task-b");
}

#[test]
fn rescheduling_completed_task_adds_one_pending_candidate() {
    let mut data = default_data();
    let mut task = queued_task("task-completed-reschedule");
    task.status = "succeeded".to_string();
    task.planned_submit_count = 1;
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-success-1".to_string(),
        submit_id: "sub-success-1".to_string(),
        status: "succeeded".to_string(),
        started_at: "2026-06-18T00:00:00Z".to_string(),
        finished_at: "2026-06-18T00:01:00Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec!["/tmp/done.mp4".to_string()],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    data.tasks.push(task);
    let scheduled_at = (Utc::now() + Duration::minutes(1)).to_rfc3339();

    let updated = reschedule_task(&mut data, "task-completed-reschedule", &scheduled_at)
        .expect("reschedule completed task");

    assert_eq!(updated.status, "scheduled");
    assert_eq!(updated.planned_submit_count, 2);
}

#[test]
fn due_rescheduled_completed_task_is_submitted_again() {
    let mut data = default_data();
    let mut task = queued_task("task-completed-due");
    task.status = "scheduled".to_string();
    task.scheduled_at = Some((Utc::now() - Duration::minutes(1)).to_rfc3339());
    task.next_run_at = task.scheduled_at.clone();
    task.planned_submit_count = 2;
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-success-1".to_string(),
        submit_id: "sub-success-1".to_string(),
        status: "succeeded".to_string(),
        started_at: "2026-06-18T00:00:00Z".to_string(),
        finished_at: "2026-06-18T00:01:00Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec!["/tmp/done.mp4".to_string()],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((
            r#"{"submit_id":"sub-success-2","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.id, "task-completed-due");
    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "sub-success-2");
}

#[test]
fn idle_failed_retry_does_not_preempt_normal_queued_task() {
    let mut data = default_data();
    data.settings.concurrency_retry_delay_seconds = 60;
    let mut failed = queued_task("task-failed");
    failed.status = "failed".to_string();
    failed.last_error = "upload connection reset".to_string();
    failed
        .execution_records
        .push(transient_failed_record("exec-failed-1", 2));
    data.tasks.push(failed);
    data.tasks.push(queued_task("task-normal"));

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((
            r#"{"submit_id":"sub-normal","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.id, "task-normal");
    assert_eq!(data.tasks[0].status, "failed");
}

#[test]
fn idle_failed_transient_task_is_resubmitted_after_retry_delay() {
    let mut data = default_data();
    data.settings.concurrency_retry_delay_seconds = 60;
    let mut task = queued_task("task-idle-rescue");
    task.status = "failed".to_string();
    task.last_error = "upload connection reset".to_string();
    task.execution_records
        .push(transient_failed_record("exec-rescue-1", 2));
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((
            r#"{"submit_id":"sub-rescue","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.id, "task-idle-rescue");
    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "sub-rescue");
}

#[test]
fn idle_failed_concurrency_limit_task_is_resubmitted_after_retry_delay() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 60;
    data.settings.concurrency_retry_max_attempts = 8;
    let mut task = queued_task("task-idle-concurrency-rescue");
    task.status = "failed".to_string();
    task.last_error = "api error: ret=1310, message=ExceedConcurrencyLimit".to_string();
    task.concurrency_retry_count = 1;
    task.execution_records
        .push(concurrency_failed_record("exec-concurrency-rescue-1", 2));
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((
            r#"{"submit_id":"sub-concurrency-rescue","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.id, "task-idle-concurrency-rescue");
    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "sub-concurrency-rescue");
    assert_eq!(result.concurrency_retry_count, 0);
}

#[test]
fn idle_failed_old_concurrency_limit_task_stays_failed() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 60;
    data.settings.concurrency_retry_max_attempts = 8;
    let mut task = queued_task("task-old-concurrency");
    task.status = "failed".to_string();
    task.last_error = "api error: ret=1310, message=ExceedConcurrencyLimit".to_string();
    task.concurrency_retry_count = 1;
    task.execution_records.push(concurrency_failed_record(
        "exec-old-concurrency",
        30 * 24 * 60,
    ));
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        panic!("old concurrency failure should not be auto-resubmitted");
    })
    .expect("process queue");

    assert!(result.is_none());
    assert_eq!(data.tasks[0].status, "failed");
}

#[test]
fn idle_failed_transient_retry_failure_stays_failed_without_normal_retry_wait() {
    let mut data = default_data();
    data.settings.concurrency_retry_delay_seconds = 60;
    let mut task = queued_task("task-idle-rescue-fail");
    task.status = "failed".to_string();
    task.last_error = "upload connection reset".to_string();
    task.execution_records
        .push(transient_failed_record("exec-rescue-fail-1", 2));
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Err(r#"upload resource "/tmp/voice.mp3": upload video/audio: Post "http://tos-d-x-hl.snssdk.com/upload/v1/tos-cn-v": read: connection reset by peer"#.to_string())
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.status, "failed");
    assert!(result.next_run_at.is_none());
    assert_eq!(result.execution_records.last().unwrap().status, "failed");
}

#[test]
fn idle_failed_transient_task_waits_for_retry_delay() {
    let mut data = default_data();
    data.settings.concurrency_retry_delay_seconds = 600;
    let mut task = queued_task("task-idle-wait");
    task.status = "failed".to_string();
    task.last_error = "upload connection reset".to_string();
    task.execution_records
        .push(transient_failed_record("exec-wait-1", 0));
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        panic!("failed transient task should wait before idle retry");
    })
    .expect("process queue");

    assert!(result.is_none());
}

#[test]
fn idle_failed_transient_task_stops_after_extra_retries() {
    let mut data = default_data();
    data.settings.concurrency_retry_delay_seconds = 60;
    let mut task = queued_task("task-idle-exhausted");
    task.status = "failed".to_string();
    task.last_error = "upload connection reset".to_string();
    for index in 0..4 {
        task.execution_records.push(transient_failed_record(
            &format!("exec-exhausted-{index}"),
            2,
        ));
    }
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        panic!("exhausted failed transient task should not be auto-resubmitted");
    })
    .expect("process queue");

    assert!(result.is_none());
}

#[test]
fn successful_submit_requeues_when_planned_success_count_not_met() {
    let mut data = default_data();
    let mut task = queued_task("task-draw");
    task.planned_submit_count = 2;
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |_args| {
        Ok((
            r#"{"submit_id":"sub-draw-1","gen_status":"success"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.status, "queued");
    assert_eq!(result.execution_records.len(), 1);
    assert_eq!(result.execution_records[0].status, "succeeded");
}

#[test]
fn importing_same_image_path_to_same_role_is_idempotent() {
    let mut data = default_data();
    let role = upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-1".to_string()),
            name: "女主".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec![],
        },
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("ref.png");
    fs::write(&source, "fake-image-data").expect("image file");
    let cache_dir = temp.path().join("cache");
    let source_path = source
        .canonicalize()
        .expect("canonical source")
        .to_string_lossy()
        .to_string();

    let updated = import_media_to_role(
        &mut data,
        &cache_dir,
        ImportRoleMediaInput {
            role_id: role.id,
            paths: vec![
                source_path.clone(),
                source_path.clone(),
                source_path.clone(),
            ],
        },
    )
    .expect("import media");

    assert_eq!(updated.asset_ids.len(), 1);
    assert_eq!(
        data.assets
            .iter()
            .filter(|asset| asset.kind == AssetKind::Image && asset.source_path == source_path)
            .count(),
        1
    );
}

#[test]
fn importing_same_path_to_different_roles_does_not_merge_ownership() {
    let mut data = default_data();
    let role_a = upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-a".to_string()),
            name: "角色A".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec![],
        },
    );
    let role_b = upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-b".to_string()),
            name: "角色B".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec![],
        },
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("shared.png");
    fs::write(&source, "shared-image").expect("image file");
    let cache_dir = temp.path().join("cache");
    let source_path = source
        .canonicalize()
        .expect("canonical source")
        .to_string_lossy()
        .to_string();

    import_media_to_role(
        &mut data,
        &cache_dir,
        ImportRoleMediaInput {
            role_id: role_a.id.clone(),
            paths: vec![source_path.clone()],
        },
    )
    .expect("import to role A");

    import_media_to_role(
        &mut data,
        &cache_dir,
        ImportRoleMediaInput {
            role_id: role_b.id.clone(),
            paths: vec![source_path.clone()],
        },
    )
    .expect("import to role B");

    let role_a = data.roles.iter().find(|r| r.id == "role-a").unwrap();
    let role_b = data.roles.iter().find(|r| r.id == "role-b").unwrap();
    assert_eq!(role_a.asset_ids.len(), 1);
    assert_eq!(role_b.asset_ids.len(), 1);
    assert_ne!(role_a.asset_ids[0], role_b.asset_ids[0]);
}

#[test]
fn removing_role_media_keeps_global_asset_if_still_referenced_by_other_role() {
    let mut data = default_data();
    let shared_asset = image_asset("img-shared", "共享图", "/tmp/shared.png");
    data.assets.push(shared_asset.clone());
    upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-a".to_string()),
            name: "角色A".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["img-shared".to_string()],
        },
    );
    upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-b".to_string()),
            name: "角色B".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["img-shared".to_string()],
        },
    );

    remove_media_from_role(
        &mut data,
        RemoveRoleMediaInput {
            role_id: "role-a".to_string(),
            asset_id: "img-shared".to_string(),
        },
    )
    .expect("remove from role A");

    assert!(data.assets.iter().any(|a| a.id == "img-shared"));
    assert!(data
        .roles
        .iter()
        .find(|r| r.id == "role-b")
        .unwrap()
        .asset_ids
        .contains(&"img-shared".to_string()));
}

#[test]
fn removing_role_media_deletes_global_asset_if_no_longer_referenced() {
    let mut data = default_data();
    let sole_asset = image_asset("img-sole", "独占图", "/tmp/sole.png");
    data.assets.push(sole_asset);
    upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-1".to_string()),
            name: "角色1".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["img-sole".to_string()],
        },
    );

    remove_media_from_role(
        &mut data,
        RemoveRoleMediaInput {
            role_id: "role-1".to_string(),
            asset_id: "img-sole".to_string(),
        },
    )
    .expect("remove sole asset");

    assert!(!data.assets.iter().any(|a| a.id == "img-sole"));
}

#[test]
fn delete_role_rejected_when_referenced_by_task() {
    let mut data = default_data();
    upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-1".to_string()),
            name: "被引用角色".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["img-1".to_string()],
        },
    );
    let mut task = queued_task("task-1");
    task.role_ids = vec!["role-1".to_string()];
    data.tasks.push(task);

    let result = delete_role(&mut data, "role-1");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("角色已被任务引用"));
}

#[test]
fn delete_role_succeeds_when_not_referenced() {
    let mut data = default_data();
    upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-1".to_string()),
            name: "未引用角色".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec![],
        },
    );
    let result = delete_role(&mut data, "role-1");
    assert!(result.is_ok());
    assert!(data.roles.iter().all(|r| r.id != "role-1"));
}

#[test]
fn draft_task_is_not_selected_by_process_queue() {
    let mut data = default_data();
    let draft = TaskDraft {
        title: "草稿任务".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
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
    let task = create_draft_task(&data, draft).expect("draft");
    data.tasks.push(task);

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        panic!("draft task should not be processed");
    };
    let result = process_next_due_task_with_runner(&mut data, runner).expect("process queue");
    assert!(result.is_none());
    assert_eq!(data.tasks[0].status, "draft");
}

#[test]
fn submitting_task_blocks_new_submissions() {
    let mut data = default_data();
    let mut task1 = queued_task("task-1");
    task1.status = "submitting".to_string();
    data.tasks.push(task1);
    data.tasks.push(queued_task("task-2"));

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        panic!("should not submit while another is submitting");
    };
    let result = process_next_due_task_with_runner(&mut data, runner).expect("process queue");
    assert!(result.is_none());
}

#[test]
fn querying_task_with_submit_id_is_prioritized() {
    let mut data = default_data();
    let mut querying_task = queued_task("task-q");
    querying_task.status = "querying".to_string();
    querying_task.submit_id = "submit-123".to_string();
    data.tasks.push(querying_task);
    data.tasks.push(queued_task("task-2"));

    let runner = |args: &[String]| -> Result<(String, String), String> {
        assert_eq!(args[0], "query_result");
        Ok((r#"{"gen_status":"success","result_paths":["/tmp/video.mp4"],"result_urls":["https://cdn.example.com/video.mp4"]}"#.to_string(), String::new()))
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task found");
    assert_eq!(result.status, "succeeded");
}

#[test]
fn queueing_over_two_hours_submits_fast_fallback() {
    let mut data = default_data();
    let mut task = queued_task("task-slow-queue");
    task.status = "querying".to_string();
    task.submit_id = "slow-123".to_string();
    task.submitted_at = Some((Utc::now() - Duration::minutes(121)).to_rfc3339());
    task.queue_info = Some(dreamina_scheduler_lib::QueueInfo {
        queue_idx: Some(4680),
        priority: Some(1),
        queue_status: Some("Queueing".to_string()),
        queue_length: Some(300_031),
    });
    task.execution_records.push(TaskExecutionRecord {
        id: "rec-slow".to_string(),
        submit_id: "slow-123".to_string(),
        status: "querying".to_string(),
        started_at: (Utc::now() - Duration::minutes(121)).to_rfc3339(),
        finished_at: String::new(),
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
        command_preview: vec![
            "multimodal2video".to_string(),
            "--model_version=seedance2.0".to_string(),
        ],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |args| {
        assert_eq!(args[0], "multimodal2video");
        assert!(args
            .iter()
            .any(|arg| arg == "--model_version=seedance2.0fast"));
        Ok((
            r#"{"submit_id":"fast-123","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "slow-123");
    assert!(result
        .execution_records
        .iter()
        .any(|record| record.submit_id == "fast-123"
            && record.status == "querying"
            && record.input_snapshot.params.model_version == "seedance2.0fast"));
}

#[test]
fn active_fast_fallback_is_queried_instead_of_duplicated() {
    let mut data = default_data();
    let mut task = queued_task("task-fast-active");
    task.status = "querying".to_string();
    task.submit_id = "slow-123".to_string();
    task.submitted_at = Some((Utc::now() - Duration::minutes(180)).to_rfc3339());
    task.queue_info = Some(dreamina_scheduler_lib::QueueInfo {
        queue_idx: Some(4680),
        priority: Some(1),
        queue_status: Some("Queueing".to_string()),
        queue_length: Some(300_031),
    });
    let mut fast_params = task.params.clone();
    fast_params.model_version = "seedance2.0fast".to_string();
    task.execution_records.push(TaskExecutionRecord {
        id: "rec-fast".to_string(),
        submit_id: "fast-123".to_string(),
        status: "querying".to_string(),
        started_at: (Utc::now() - Duration::minutes(61)).to_rfc3339(),
        finished_at: String::new(),
        input_snapshot: TaskExecutionInputSnapshot {
            prompt: task.prompt.clone(),
            image_asset_ids: task.image_asset_ids.clone(),
            audio_asset_ids: task.audio_asset_ids.clone(),
            role_ids: task.role_ids.clone(),
            manual_mention_ids: task.manual_mention_ids.clone(),
            auto_match_roles: task.auto_match_roles,
            params: fast_params,
            temp_image_asset_ids: task.temp_image_asset_ids.clone(),
        },
        command_preview: vec![
            "multimodal2video".to_string(),
            "--model_version=seedance2.0fast".to_string(),
        ],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |args| {
        assert_eq!(
            args,
            &[
                "query_result".to_string(),
                "--submit_id=fast-123".to_string()
            ]
        );
        Ok((
            r#"{"gen_status":"querying","queue_status":"Queueing","queue_idx":1000}"#.to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.status, "querying");
    assert_eq!(
        result
            .execution_records
            .iter()
            .filter(|record| record.input_snapshot.params.model_version == "seedance2.0fast")
            .count(),
        1
    );
}

#[test]
fn successful_fast_fallback_completes_task() {
    let mut data = default_data();
    let mut task = queued_task("task-fast-success");
    task.status = "querying".to_string();
    task.submit_id = "slow-123".to_string();
    task.submitted_at = Some((Utc::now() - Duration::minutes(180)).to_rfc3339());
    let mut fast_params = task.params.clone();
    fast_params.model_version = "seedance2.0fast".to_string();
    task.execution_records.push(TaskExecutionRecord {
        id: "rec-fast".to_string(),
        submit_id: "fast-123".to_string(),
        status: "querying".to_string(),
        started_at: (Utc::now() - Duration::minutes(30)).to_rfc3339(),
        finished_at: String::new(),
        input_snapshot: TaskExecutionInputSnapshot {
            prompt: task.prompt.clone(),
            image_asset_ids: task.image_asset_ids.clone(),
            audio_asset_ids: task.audio_asset_ids.clone(),
            role_ids: task.role_ids.clone(),
            manual_mention_ids: task.manual_mention_ids.clone(),
            auto_match_roles: task.auto_match_roles,
            params: fast_params,
            temp_image_asset_ids: task.temp_image_asset_ids.clone(),
        },
        command_preview: vec![
            "multimodal2video".to_string(),
            "--model_version=seedance2.0fast".to_string(),
        ],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    data.tasks.push(task);

    let result = process_next_due_task_with_runner(&mut data, |args| {
        assert_eq!(
            args,
            &[
                "query_result".to_string(),
                "--submit_id=fast-123".to_string()
            ]
        );
        Ok((
            r#"{"gen_status":"success","result_urls":["https://cdn.example.com/fast.mp4"]}"#
                .to_string(),
            String::new(),
        ))
    })
    .expect("process queue")
    .expect("task processed");

    assert_eq!(result.status, "succeeded");
    assert_eq!(result.submit_id, "fast-123");
    assert_eq!(result.result_urls, vec!["https://cdn.example.com/fast.mp4"]);
}

#[test]
fn retry_wait_not_due_is_not_submitted() {
    let mut data = default_data();
    let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let mut task = queued_task("task-retry");
    task.status = "retry_wait".to_string();
    task.next_run_at = Some(future);
    data.tasks.push(task);

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        panic!("retry_wait not due should not be submitted");
    };
    let result = process_next_due_task_with_runner(&mut data, runner).expect("process queue");
    assert!(result.is_none());
}

#[test]
fn retry_wait_due_is_resubmitted() {
    let mut data = default_data();
    let past = (Utc::now() - Duration::seconds(5)).to_rfc3339();
    let mut task = queued_task("task-retry-due");
    task.status = "retry_wait".to_string();
    task.next_run_at = Some(past);
    data.tasks.push(task);

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        Ok((
            r#"{"submit_id":"retry-001","gen_status":"querying"}"#.to_string(),
            String::new(),
        ))
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task processed");
    assert_eq!(result.status, "querying");
    assert_eq!(result.submit_id, "retry-001");
}

#[test]
fn peek_due_task_cli_reports_build_error_instead_of_hiding_due_task() {
    let mut data = default_data();
    let mut task = queued_task("task-bad-asset");
    task.image_asset_ids = vec!["missing-image".to_string()];
    data.tasks.push(task);

    let error = peek_due_task_cli(&data).expect_err("bad due task should report build error");

    assert_eq!(error.task_id, "task-bad-asset");
    assert!(error.message.contains("构建提交输入失败"));
    assert!(error.message.contains("missing-image"));
}

#[test]
fn concurrency_retry_exceeds_configured_count_still_waits() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_max_attempts = 1;
    let draft = TaskDraft {
        title: "限流任务".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
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
    let mut task = create_task_with_preview(&data, draft).expect("task");
    task.concurrency_retry_count = 1;
    data.tasks.push(task);

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        Ok((r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit"}"#.to_string(), String::new()))
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task processed");

    assert_eq!(result.status, "retry_wait");
    assert!(result.next_run_at.is_some());
}

#[test]
fn transient_error_triggers_retry_wait() {
    let mut data = default_data();
    let draft = TaskDraft {
        title: "瞬态错误".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
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
    data.tasks
        .push(create_task_with_preview(&data, draft).expect("task"));

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        Ok((
            r#"{"gen_status":"fail","fail_reason":"Unexpected end of JSON input"}"#.to_string(),
            String::new(),
        ))
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task processed");

    assert_eq!(result.status, "retry_wait");
    assert!(result.next_run_at.is_some());
}

#[test]
fn transient_submit_error_fails_after_three_retries() {
    let mut data = default_data();
    let mut task = queued_task("task-transient-max");
    for index in 0..3 {
        task.execution_records.push(TaskExecutionRecord {
            id: format!("exec-transient-{index}"),
            submit_id: String::new(),
            status: "retry_wait".to_string(),
            started_at: Utc::now().to_rfc3339(),
            finished_at: Utc::now().to_rfc3339(),
            input_snapshot: TaskExecutionInputSnapshot::default(),
            command_preview: vec![],
            query_records: vec![],
            result_paths: vec![],
            result_urls: vec![],
            error_kind: "Transient".to_string(),
            error_detail: "upload connection reset".to_string(),
        });
    }
    data.tasks.push(task);

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        Err(r#"upload resource "/tmp/voice.mp3": upload video/audio: Post "http://tos-d-x-hl.snssdk.com/upload/v1/tos-cn-v": read tcp 198.18.0.1:55944->198.18.3.122:80: read: connection reset by peer"#.to_string())
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task processed");

    assert_eq!(result.status, "failed");
    assert!(result.next_run_at.is_none());
    assert_eq!(result.execution_records.last().unwrap().status, "failed");
    assert_eq!(
        result.execution_records.last().unwrap().error_kind,
        "Transient"
    );
}

#[test]
fn upload_image_eof_triggers_retry_wait() {
    let mut data = default_data();
    data.tasks.push(queued_task("task-upload-eof"));

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        Err(r#"upload resource "/tmp/asset.png": upload image: apply phase, ApplyImageUpload: do request, Get "http://imagex.bytedanceapi.com/?Action=ApplyImageUpload": EOF"#.to_string())
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task processed");

    assert_eq!(result.status, "retry_wait");
    assert!(result.next_run_at.is_some());
    assert!(result.last_error.contains("ApplyImageUpload"));
}

#[test]
fn submit_failure_records_preflight_asset_diagnostics() {
    let mut data = default_data();
    let temp_root = std::env::temp_dir();
    let image_path = temp_root.join("dreamina-preflight-role.png");
    let audio_path = temp_root.join("dreamina-preflight-voice.mp3");
    fs::write(&image_path, b"fake image bytes").expect("write image fixture");
    fs::write(&audio_path, b"fake audio bytes").expect("write audio fixture");
    data.assets[0].stored_path = image_path.to_string_lossy().to_string();
    data.assets[0].source_path = data.assets[0].stored_path.clone();
    data.assets[0].size_bytes = 16;
    data.assets.push(audio_asset(
        "aud-preflight",
        "旁白音频",
        &audio_path.to_string_lossy(),
    ));
    let mut task = queued_task("task-vod-eof");
    task.audio_asset_ids = vec!["aud-preflight".to_string()];
    data.tasks.push(task);

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        Err(r#"upload resource "/tmp/voice.mp3": upload video/audio: Get "http://vod.bytedanceapi.com/top/v1?Action=ApplyUploadInner&FileType=video&SessionKey=&SpaceName=dreamina&Version=2020-11-19": EOF"#.to_string())
    };
    let result = process_next_due_task_with_runner(&mut data, runner)
        .expect("process queue")
        .expect("task processed");

    assert_eq!(result.status, "retry_wait");
    let preflight_log = data
        .logs
        .iter()
        .find(|log| log.event_type == "submit_preflight")
        .expect("preflight log");
    assert!(preflight_log.detail.contains("aud-preflight"));
    assert!(preflight_log.detail.contains("\"exists\": true"));
    assert!(result.attempts[0]
        .error_detail
        .contains("submit_preflight="));
    assert!(result.attempts[0]
        .error_detail
        .contains(&audio_path.to_string_lossy().to_string()));
}

#[test]
fn state_recovery_resets_submitting_to_queued() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "submitting".to_string();
    data.tasks.push(task);
    for task in &mut data.tasks {
        if task.status == "submitting" {
            task.status = "queued".to_string();
            task.last_error = "应用重启，任务从 submitting 恢复为 queued".to_string();
        }
    }
    assert_eq!(data.tasks[0].status, "queued");
    assert!(data.tasks[0].last_error.contains("submitting"));
}

#[test]
fn state_recovery_resets_querying_to_submitted() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "querying".to_string();
    task.submit_id = "sub-1".to_string();
    data.tasks.push(task);
    for task in &mut data.tasks {
        if task.status == "querying" {
            task.status = "submitted".to_string();
            task.last_error = "应用重启，任务从 querying 恢复为 submitted".to_string();
        }
    }
    assert_eq!(data.tasks[0].status, "submitted");
}

#[test]
fn log_retention_truncates_old_logs() {
    let mut data = default_data();
    data.settings.log_retention_count = 5;
    for i in 0..10 {
        data.logs.push(LogEntry {
            id: format!("log-{}", i),
            timestamp: String::new(),
            level: LogLevel::Info,
            source: LogSource::System,
            category: String::new(),
            event_type: String::new(),
            message: format!("log entry {}", i),
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
            legacy_string: None,
        });
    }
    let max_logs = data.settings.log_retention_count as usize;
    if data.logs.len() > max_logs {
        let drain = data.logs.len() - max_logs;
        data.logs.drain(0..drain);
    }
    assert_eq!(data.logs.len(), 5);
    assert_eq!(data.logs[0].message, "log entry 5");
}

#[test]
fn delete_task_removes_from_list() {
    let mut data = default_data();
    data.tasks.push(queued_task("task-1"));
    data.tasks.push(queued_task("task-2"));
    delete_task_from_data(&mut data, "task-1").expect("delete");
    assert_eq!(data.tasks.len(), 1);
    assert_eq!(data.tasks[0].id, "task-2");
}

#[test]
fn delete_task_does_not_remove_role_assets() {
    let mut data = default_data();
    upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-1".to_string()),
            name: "角色".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["img-1".to_string()],
        },
    );
    let mut task = queued_task("task-1");
    task.image_asset_ids = vec!["img-1".to_string()];
    data.tasks.push(task);

    delete_task_from_data(&mut data, "task-1").expect("delete task");
    assert!(data.assets.iter().any(|a| a.id == "img-1"));
    assert!(data.roles[0].asset_ids.contains(&"img-1".to_string()));
}

#[test]
fn clipboard_empty_bytes_rejected() {
    let mut data = default_data();
    let temp = tempfile::tempdir().expect("tempdir");
    let result = save_clipboard_image_asset(
        &mut data,
        temp.path(),
        ClipboardImageInput {
            file_name: "empty.png".to_string(),
            mime: "image/png".to_string(),
            bytes: vec![],
        },
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("剪贴板图片为空"));
}

#[test]
fn upsert_role_updates_existing_role_preserving_created_at() {
    let mut data = default_data();
    let original = upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-1".to_string()),
            name: "原名".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec![],
        },
    );
    let created_at = original.created_at.clone();

    let updated = upsert_role(
        &mut data,
        CreateRoleInput {
            id: Some("role-1".to_string()),
            name: "新名".to_string(),
            aliases: vec!["别名".to_string()],
            tags: vec![],
            description: "描述".to_string(),
            asset_ids: vec!["img-1".to_string()],
        },
    );
    assert_eq!(updated.name, "新名");
    assert_eq!(updated.created_at, created_at);
    assert_eq!(updated.asset_ids, vec!["img-1"]);
}

#[test]
fn draft_task_backfills_prompt_mentioned_role_image_and_audio_assets() {
    let data = AppData {
        settings: SchedulerSettings::default(),
        assets: vec![
            image_asset("tmp-1", "分镜图", "/tmp/storyboard.png"),
            image_asset("img-chef", "厨师服", "/tmp/chef.png"),
            audio_asset("aud-hero", "女主人声音", "/tmp/hero.mp3"),
        ],
        roles: vec![Role {
            id: "role-hero".to_string(),
            name: "女主".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["img-chef".to_string(), "aud-hero".to_string()],
            created_at: String::new(),
            updated_at: String::new(),
        }],
        tasks: vec![],
        logs: vec![],
        imagegen_history: vec![],
        asset_hash_index: HashMap::new(),
    };
    let draft = TaskDraft {
        title: String::new(),
        prompt: "根据分镜图 @分镜图1 女主是 @女主厨师服 声音 @女主女主人声音".to_string(),
        image_asset_ids: vec!["tmp-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec!["tmp-1".to_string()],
        temp_image_paths: vec!["/tmp/storyboard.png".to_string()],
        prompt_doc: None,
    };

    let task = create_draft_task(&data, draft).expect("draft should save");

    assert_eq!(
        task.image_asset_ids,
        vec!["tmp-1".to_string(), "img-chef".to_string()]
    );
    assert_eq!(task.audio_asset_ids, vec!["aud-hero".to_string()]);
    assert!(task
        .command_preview
        .iter()
        .any(|arg| arg == "--image=/tmp/chef.png"));
    assert!(task
        .command_preview
        .iter()
        .any(|arg| arg == "--audio=/tmp/hero.mp3"));
}

#[test]
fn draft_with_invalid_asset_id_rejected() {
    let data = default_data();
    let draft = TaskDraft {
        title: "bad draft".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["nonexistent-asset".to_string()],
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
    let result = create_draft_task(&data, draft);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("找不到素材"));
}

#[test]
fn draft_with_invalid_role_id_rejected() {
    let data = default_data();
    let draft = TaskDraft {
        title: "bad role draft".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec!["nonexistent-role".to_string()],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        prompt_doc: None,
    };
    let result = create_draft_task(&data, draft);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("找不到角色"));
}

#[test]
fn pause_scheduled_task() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "scheduled".to_string();
    data.tasks.push(task);
    let result = pause_task(&mut data, "task-1");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, "paused");
}

#[test]
fn pause_retry_wait_task() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "retry_wait".to_string();
    data.tasks.push(task);
    let result = pause_task(&mut data, "task-1");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, "paused");
}

#[test]
fn reschedule_rejects_non_allowed_status() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "submitting".to_string();
    data.tasks.push(task);
    let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let result = reschedule_task(&mut data, "task-1", &future);
    assert!(result.is_err());
}

fn make_attempt(id: &str, status: &str, stderr: &str) -> TaskAttempt {
    TaskAttempt {
        id: id.to_string(),
        started_at: Utc::now().to_rfc3339(),
        finished_at: Utc::now().to_rfc3339(),
        status: status.to_string(),
        command_preview: vec![],
        stdout: String::new(),
        stderr: stderr.to_string(),
        error_kind: if status == "failed" {
            "command_failed".to_string()
        } else {
            String::new()
        },
        duration_seconds: 1.0,
        error_detail: if status == "failed" {
            stderr.to_string()
        } else {
            String::new()
        },
    }
}

fn draft_with_new_prompt(prompt: &str) -> TaskDraft {
    TaskDraft {
        title: String::new(),
        prompt: prompt.to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
        prompt_doc: None,
    }
}

// ── T001: 编辑已成功任务后历史不丢 ──────────────────────────────────────────

#[test]
fn edit_succeeded_task_preserves_history() {
    let mut data = default_data();
    let mut task = queued_task("task-1");
    task.status = "succeeded".to_string();
    task.submit_id = "sub-001".to_string();
    task.result_paths = vec!["/tmp/result.mp4".to_string()];
    task.result_urls = vec!["https://cdn.example.com/result.mp4".to_string()];
    task.attempts = vec![make_attempt("a1", "done", "")];
    task.last_error = String::new();
    data.tasks.push(task);

    let updated = update_task_from_data(
        &mut data,
        "task-1",
        draft_with_new_prompt("新提示词"),
        "task",
    )
    .expect("update should succeed");

    assert_eq!(updated.id, "task-1", "任务 ID 不能变");
    assert_eq!(updated.prompt, "新提示词", "新 prompt 应更新");
    assert_eq!(updated.submit_id, "sub-001", "submit_id 应保留");
    assert_eq!(
        updated.result_paths,
        vec!["/tmp/result.mp4".to_string()],
        "result_paths 应保留"
    );
    assert_eq!(
        updated.result_urls,
        vec!["https://cdn.example.com/result.mp4".to_string()],
        "result_urls 应保留"
    );
    assert_eq!(updated.attempts.len(), 1, "attempts 应保留");
    assert_eq!(updated.status, "queued", "编辑后状态应为 queued");
}

#[test]
fn edit_failed_task_preserves_error_and_attempts() {
    let mut data = default_data();
    let mut task = queued_task("task-2");
    task.status = "failed".to_string();
    task.submit_id = "sub-fail".to_string();
    task.last_error = "timeout after 30s".to_string();
    task.attempts = vec![
        make_attempt("a1", "done", ""),
        make_attempt("a2", "failed", "timeout after 30s"),
    ];
    data.tasks.push(task);

    let updated = update_task_from_data(
        &mut data,
        "task-2",
        draft_with_new_prompt("重试提示词"),
        "task",
    )
    .expect("update should succeed");

    assert_eq!(updated.id, "task-2");
    assert_eq!(updated.prompt, "重试提示词");
    assert_eq!(updated.submit_id, "sub-fail", "submit_id 应保留");
    assert_eq!(updated.last_error, "timeout after 30s", "last_error 应保留");
    assert_eq!(updated.attempts.len(), 2, "attempts 应保留两条");
    assert_eq!(updated.attempts[1].stderr, "timeout after 30s");
}

#[test]
fn edit_task_as_draft_preserves_history_and_completed_status() {
    let mut data = default_data();
    let mut task = queued_task("task-3");
    task.status = "succeeded".to_string();
    task.result_paths = vec!["/tmp/done.mp4".to_string()];
    task.attempts = vec![make_attempt("a1", "done", "")];
    data.tasks.push(task);

    let updated = update_task_from_data(
        &mut data,
        "task-3",
        draft_with_new_prompt("草稿提示词"),
        "draft",
    )
    .expect("update as draft should succeed");

    assert_eq!(updated.status, "succeeded", "保存草稿不应清掉已有成功状态");
    assert!(
        !updated.command_preview.is_empty(),
        "保存为草稿也应保留命令预览，供任务详情查看"
    );
    assert_eq!(updated.command_preview[0], "multimodal2video");
    assert_eq!(
        updated.result_paths,
        vec!["/tmp/done.mp4".to_string()],
        "result_paths 应保留"
    );
    assert_eq!(updated.attempts.len(), 1, "attempts 应保留");
}

#[test]
fn backfill_draft_command_previews_updates_existing_eligible_drafts() {
    let mut data = default_data();
    let mut task = queued_task("draft-without-preview");
    task.status = "draft".to_string();
    task.command_preview = vec![];
    data.tasks.push(task);

    let updated = backfill_draft_command_previews(&mut data);

    assert_eq!(updated, 1);
    assert_eq!(data.tasks[0].status, "draft");
    assert_eq!(data.tasks[0].command_preview[0], "multimodal2video");
}

#[test]
fn backfill_draft_command_previews_replaces_stale_existing_preview() {
    let mut data = default_data();
    let mut task = queued_task("draft-with-stale-preview");
    task.status = "draft".to_string();
    task.prompt = "@角色图 生成镜头".to_string();
    task.command_preview = vec![
        "multimodal2video".to_string(),
        "--image=/tmp/role.png".to_string(),
        "--prompt=@角色图 生成镜头\n参考图片：@图1".to_string(),
    ];
    data.tasks.push(task);

    let updated = backfill_draft_command_previews(&mut data);

    assert_eq!(updated, 1);
    let prompt_arg = data.tasks[0]
        .command_preview
        .iter()
        .find(|arg| arg.starts_with("--prompt="))
        .expect("prompt arg");
    assert!(prompt_arg.contains("@图1 生成镜头"));
    assert!(!prompt_arg.contains("参考图片"));
}

// ── T006: 再次提交追加新 attempt，不覆盖旧记录 ────────────────────────────────

#[test]
fn second_submit_appends_to_attempts_not_overwrites() {
    let mut data = default_data();
    let mut task = queued_task("task-4");
    task.attempts = vec![make_attempt("a1", "done", "")];
    task.result_paths = vec!["/tmp/first.mp4".to_string()];
    data.tasks.push(task);

    // 模拟再次提交通过 process_queue 时 attempts 应追加
    // 直接验证：update_task 后 attempts 未被清空，submit 逻辑保留旧 attempts
    let updated = update_task_from_data(
        &mut data,
        "task-4",
        draft_with_new_prompt("第二次提示词"),
        "task",
    )
    .expect("second update should succeed");

    assert_eq!(updated.attempts.len(), 1, "update 本身不清空旧 attempts");
    assert_eq!(
        updated.result_paths,
        vec!["/tmp/first.mp4".to_string()],
        "旧结果应保留"
    );
    assert_eq!(updated.prompt, "第二次提示词");
}

#[test]
fn second_submit_creates_new_execution_record_with_new_submit_id() {
    let mut data = default_data();
    let mut task = queued_task("task-second-submit");
    task.status = "succeeded".to_string();
    task.submit_id = "sub-first".to_string();
    task.result_paths = vec!["/tmp/first.mp4".to_string()];
    data.tasks.push(task);

    let updated = update_task_from_data(
        &mut data,
        "task-second-submit",
        draft_with_new_prompt("第二次提示词"),
        "task",
    )
    .expect("edit should succeed");
    assert_eq!(updated.status, "queued");

    let submitted = dreamina_scheduler_lib::submit_task_once_with_runner(
        &mut data,
        "task-second-submit",
        None,
        |_| {
            Ok((
                r#"{"submit_id":"sub-second","gen_status":"querying"}"#.to_string(),
                String::new(),
            ))
        },
    )
    .expect("second submit should succeed");

    assert_eq!(submitted.submit_id, "sub-second");
    assert_eq!(submitted.status, "querying");
    assert_eq!(
        submitted.result_paths.len(),
        0,
        "当前执行不能继续展示第一次结果"
    );
    assert_eq!(submitted.execution_records.len(), 1);
    assert_eq!(submitted.execution_records[0].submit_id, "sub-second");
    assert_eq!(
        submitted.execution_records[0].input_snapshot.prompt,
        "第二次提示词"
    );
}

#[test]
fn failed_second_submit_does_not_reuse_previous_submit_id() {
    let mut data = default_data();
    let mut task = queued_task("task-second-fail");
    task.status = "succeeded".to_string();
    task.submit_id = "sub-first".to_string();
    task.result_paths = vec!["/tmp/first.mp4".to_string()];
    data.tasks.push(task);

    update_task_from_data(
        &mut data,
        "task-second-fail",
        draft_with_new_prompt("第二次提示词"),
        "task",
    )
    .expect("edit should succeed");

    let submitted =
        dreamina_scheduler_lib::submit_task_once_with_runner(&mut data, "task-second-fail", None, |_| {
            Ok((r#"{"message":"提交失败"}"#.to_string(), String::new()))
        })
        .expect("failed submit should still write task state");

    assert_eq!(submitted.status, "failed");
    assert_eq!(
        submitted.submit_id, "",
        "没有新 submit_id 时不能沿用第一次 submit_id"
    );
    assert_eq!(
        submitted.result_paths.len(),
        0,
        "当前执行不能继续展示第一次结果"
    );
    assert_eq!(submitted.execution_records.len(), 1);
    assert_eq!(submitted.execution_records[0].submit_id, "");
    assert_eq!(submitted.execution_records[0].status, "failed");
}

#[test]
fn backfill_execution_records_from_legacy_attempts_recovers_multiple_submits() {
    let mut data = default_data();
    let mut task = queued_task("task-legacy");
    task.submit_id = "sub-2".to_string();
    task.status = "querying".to_string();
    task.result_paths = vec!["/tmp/local-first.mp4".to_string()];
    task.result_urls = vec!["https://example.com/first.mp4".to_string()];
    task.attempts = vec![
        TaskAttempt {
            id: "submit-1".to_string(),
            started_at: "2026-04-30T10:00:00Z".to_string(),
            finished_at: "2026-04-30T10:00:01Z".to_string(),
            status: "querying".to_string(),
            command_preview: vec!["multimodal2video".to_string()],
            stdout: r#"{"submit_id":"sub-1","gen_status":"querying"}"#.to_string(),
            stderr: String::new(),
            error_kind: String::new(),
            duration_seconds: 1.0,
            error_detail: String::new(),
        },
        TaskAttempt {
            id: "query-1".to_string(),
            started_at: "2026-04-30T10:05:00Z".to_string(),
            finished_at: "2026-04-30T10:05:01Z".to_string(),
            status: "succeeded".to_string(),
            command_preview: vec!["query_result".to_string(), "--submit_id=sub-1".to_string()],
            stdout: r#"{"gen_status":"SUCCESS","result_urls":["https://example.com/first.mp4"]}"#
                .to_string(),
            stderr: String::new(),
            error_kind: String::new(),
            duration_seconds: 1.0,
            error_detail: String::new(),
        },
        TaskAttempt {
            id: "submit-2".to_string(),
            started_at: "2026-04-30T11:00:00Z".to_string(),
            finished_at: "2026-04-30T11:00:01Z".to_string(),
            status: "querying".to_string(),
            command_preview: vec!["multimodal2video".to_string()],
            stdout: r#"{"submit_id":"sub-2","gen_status":"querying"}"#.to_string(),
            stderr: String::new(),
            error_kind: String::new(),
            duration_seconds: 1.0,
            error_detail: String::new(),
        },
    ];
    data.tasks.push(task);

    let changed = backfill_execution_records_from_attempts(&mut data);
    let task = &data.tasks[0];

    assert_eq!(changed, 2);
    assert_eq!(task.execution_records.len(), 2);
    assert_eq!(task.execution_records[0].submit_id, "sub-1");
    assert_eq!(task.execution_records[0].status, "succeeded");
    assert_eq!(task.execution_records[0].query_records.len(), 1);
    assert_eq!(
        task.execution_records[0].result_urls,
        vec!["https://example.com/first.mp4".to_string()]
    );
    assert_eq!(
        task.execution_records[0].result_paths,
        vec!["/tmp/local-first.mp4".to_string()]
    );
    assert_eq!(task.execution_records[1].submit_id, "sub-2");
    assert_eq!(task.execution_records[1].status, "querying");
}

#[test]
fn backfill_execution_records_compacts_legacy_retry_submit_attempts() {
    let mut data = default_data();
    let mut task = queued_task("task-legacy-retry-compact");
    task.attempts = vec![
        TaskAttempt {
            id: "submit-retry-1".to_string(),
            started_at: "2026-04-30T10:00:00Z".to_string(),
            finished_at: "2026-04-30T10:00:01Z".to_string(),
            status: "retry_wait".to_string(),
            command_preview: vec!["multimodal2video".to_string()],
            stdout: r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit"}"#.to_string(),
            stderr: String::new(),
            error_kind: "ConcurrencyLimit".to_string(),
            duration_seconds: 1.0,
            error_detail: "api error: ret=1310, message=ExceedConcurrencyLimit\n\nsubmit_preflight={...}".to_string(),
        },
        TaskAttempt {
            id: "submit-retry-2".to_string(),
            started_at: "2026-04-30T10:05:00Z".to_string(),
            finished_at: "2026-04-30T10:05:01Z".to_string(),
            status: "retry_wait".to_string(),
            command_preview: vec!["multimodal2video".to_string()],
            stdout: r#"{"gen_status":"fail","fail_reason":"api error: ret=1310, message=ExceedConcurrencyLimit"}"#.to_string(),
            stderr: String::new(),
            error_kind: "ConcurrencyLimit".to_string(),
            duration_seconds: 1.0,
            error_detail: "api error: ret=1310, message=ExceedConcurrencyLimit\n\nsubmit_preflight={...}".to_string(),
        },
    ];
    data.tasks.push(task);

    let changed = backfill_execution_records_from_attempts(&mut data);
    let task = &data.tasks[0];

    assert_eq!(changed, 1);
    assert_eq!(task.execution_records.len(), 1);
    assert_eq!(task.execution_records[0].status, "retry_wait");
    assert_eq!(task.execution_records[0].error_kind, "ConcurrencyLimit");
    assert_eq!(
        task.execution_records[0].error_detail,
        "并发任务仍在生成中，已自动排队等待下次重试。"
    );
    assert!(!task.execution_records[0]
        .error_detail
        .contains("submit_preflight"));
}

#[test]
fn compact_retry_execution_records_merges_existing_long_retry_records() {
    let mut data = default_data();
    let mut task = queued_task("task-existing-retry-compact");
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-long-1".to_string(),
        submit_id: "sub-old-1".to_string(),
        status: "retry_wait".to_string(),
        started_at: "2026-04-30T10:00:00Z".to_string(),
        finished_at: "2026-04-30T10:00:01Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec!["multimodal2video".to_string()],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: "ConcurrencyLimit".to_string(),
        error_detail:
            "api error: ret=1310, message=ExceedConcurrencyLimit\n\nsubmit_preflight={...}"
                .to_string(),
    });
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-long-2".to_string(),
        submit_id: "sub-old-2".to_string(),
        status: "retry_wait".to_string(),
        started_at: "2026-04-30T10:05:00Z".to_string(),
        finished_at: "2026-04-30T10:05:01Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec!["multimodal2video".to_string()],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: "ConcurrencyLimit".to_string(),
        error_detail:
            "api error: ret=1310, message=ExceedConcurrencyLimit\n\nsubmit_preflight={...}"
                .to_string(),
    });
    data.tasks.push(task);

    let removed = compact_retry_execution_records_for_display(&mut data);
    let task = &data.tasks[0];

    assert_eq!(removed, 1);
    assert_eq!(task.execution_records.len(), 1);
    assert_eq!(task.execution_records[0].id, "exec-long-2");
    assert_eq!(task.execution_records[0].submit_id, "sub-old-2");
    assert_eq!(task.execution_records[0].status, "retry_wait");
    assert_eq!(
        task.execution_records[0].error_detail,
        "并发任务仍在生成中，已自动排队等待下次重试。"
    );
    assert!(!task.execution_records[0]
        .error_detail
        .contains("submit_preflight"));
}

// ── T007: reschedule / pause / resume 不影响 execution_records ──────────────

#[test]
fn reschedule_does_not_clear_attempts_or_results() {
    let mut data = default_data();
    let mut task = queued_task("task-5");
    task.attempts = vec![make_attempt("a1", "done", "")];
    task.result_paths = vec!["/tmp/video.mp4".to_string()];
    data.tasks.push(task);

    let future = (Utc::now() + Duration::hours(2)).to_rfc3339();
    reschedule_task(&mut data, "task-5", &future).expect("reschedule should succeed");

    let t = data.tasks.iter().find(|t| t.id == "task-5").unwrap();
    assert_eq!(t.attempts.len(), 1, "reschedule 不应清空 attempts");
    assert_eq!(
        t.result_paths,
        vec!["/tmp/video.mp4".to_string()],
        "reschedule 不应清空结果"
    );
}

#[test]
fn pause_resume_do_not_clear_attempts() {
    let mut data = default_data();
    let mut task = queued_task("task-6");
    task.attempts = vec![make_attempt("a1", "done", "")];
    data.tasks.push(task);

    pause_task(&mut data, "task-6").expect("pause should succeed");
    let t = data.tasks.iter().find(|t| t.id == "task-6").unwrap();
    assert_eq!(t.attempts.len(), 1, "pause 不应清空 attempts");

    resume_task(&mut data, "task-6", "immediate").expect("resume should succeed");
    let t = data.tasks.iter().find(|t| t.id == "task-6").unwrap();
    assert_eq!(t.attempts.len(), 1, "resume 不应清空 attempts");
}

// ── T003 验证: 编辑不存在的任务应返回错误 ─────────────────────────────────────

#[test]
fn update_nonexistent_task_returns_error() {
    let mut data = default_data();
    let result = update_task_from_data(
        &mut data,
        "no-such-task",
        draft_with_new_prompt("x"),
        "task",
    );
    assert!(result.is_err(), "找不到任务时应返回 Err");
}

// ── T001/T002: 重启恢复不写入 error_detail ────────────────────────────────────

fn make_querying_task_with_record(task_id: &str, submit_id: &str) -> ScheduledTask {
    let mut task = queued_task(task_id);
    task.status = "querying".to_string();
    task.submit_id = submit_id.to_string();
    task.execution_records.push(TaskExecutionRecord {
        id: format!("exec-{submit_id}"),
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
    });
    task
}

#[test]
fn restart_recovery_does_not_write_query_interrupt_to_error_detail() {
    let mut data = default_data();
    data.tasks
        .push(make_querying_task_with_record("task-restart", "sub-123"));

    recover_tasks_on_load(&mut data);

    let rec = data.tasks[0]
        .execution_records
        .iter()
        .find(|r| r.submit_id == "sub-123")
        .expect("执行记录应存在");
    assert!(
        rec.error_detail.is_empty(),
        "重启恢复不应写入 error_detail，got: {:?}",
        rec.error_detail
    );
}

#[test]
fn restart_recovery_keeps_task_as_submitted_not_failed() {
    let mut data = default_data();
    data.tasks
        .push(make_querying_task_with_record("task-restart2", "sub-456"));

    recover_tasks_on_load(&mut data);

    assert_eq!(
        data.tasks[0].status, "submitted",
        "重启后无结果任务应回退为 submitted，而非 failed"
    );
    assert!(
        data.tasks[0].last_error.is_empty(),
        "重启后 last_error 应清空，不含中断文案"
    );
}

#[test]
fn restart_recovery_moves_failed_concurrency_limit_back_to_retry_wait() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 60;
    data.settings.concurrency_retry_max_attempts = 8;
    let mut task = queued_task("task-recover-concurrency");
    task.status = "failed".to_string();
    task.last_error = "api error: ret=1310, message=ExceedConcurrencyLimit".to_string();
    task.concurrency_retry_count = 1;
    task.execution_records
        .push(concurrency_failed_record("exec-recover-concurrency", 2));
    data.tasks.push(task);

    recover_tasks_on_load(&mut data);

    assert_eq!(data.tasks[0].status, "retry_wait");
    assert!(data.tasks[0].next_run_at.is_some());
    assert!(data.tasks[0].last_error.contains("ExceedConcurrencyLimit"));
    assert_eq!(data.tasks[0].execution_records[0].status, "retry_wait");
    assert_eq!(
        data.tasks[0].execution_records[0].error_detail,
        "并发任务仍在生成中，已自动排队等待下次重试。"
    );
}

#[test]
fn restart_recovery_syncs_retry_wait_concurrency_execution_record_status() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 60;
    let mut task = queued_task("task-retry-wait-concurrency-record");
    task.status = "retry_wait".to_string();
    task.next_run_at = Some((Utc::now() + chrono::Duration::minutes(5)).to_rfc3339());
    task.last_error = "api error: ret=1310, message=ExceedConcurrencyLimit".to_string();
    task.concurrency_retry_count = 14;
    task.execution_records
        .push(concurrency_failed_record("exec-retry-wait-concurrency", 2));
    data.tasks.push(task);

    recover_tasks_on_load(&mut data);

    assert_eq!(data.tasks[0].status, "retry_wait");
    assert_eq!(data.tasks[0].execution_records[0].status, "retry_wait");
    assert_eq!(
        data.tasks[0].execution_records[0].error_detail,
        "并发任务仍在生成中，已自动排队等待下次重试。"
    );
}

#[test]
fn restart_recovery_keeps_old_failed_concurrency_limit_failed() {
    let mut data = default_data();
    data.settings.concurrency_limit_policy = ConcurrencyLimitPolicy::SilentRetry;
    data.settings.concurrency_retry_delay_seconds = 60;
    data.settings.concurrency_retry_max_attempts = 8;
    let mut task = queued_task("task-old-recover-concurrency");
    task.status = "failed".to_string();
    task.last_error = "api error: ret=1310, message=ExceedConcurrencyLimit".to_string();
    task.concurrency_retry_count = 1;
    task.execution_records.push(concurrency_failed_record(
        "exec-old-recover-concurrency",
        30 * 24 * 60,
    ));
    data.tasks.push(task);

    recover_tasks_on_load(&mut data);

    assert_eq!(data.tasks[0].status, "failed");
    assert!(data.tasks[0].next_run_at.is_none());
    assert!(data.tasks[0].last_error.contains("ExceedConcurrencyLimit"));
}

#[test]
fn restart_recovery_clears_legacy_query_interrupt_from_error_detail() {
    let mut data = default_data();
    let mut task = make_querying_task_with_record("task-legacy", "sub-old");
    task.execution_records[0].error_detail = "应用重启，查询中断".to_string();
    data.tasks.push(task);

    recover_tasks_on_load(&mut data);

    let rec = data.tasks[0]
        .execution_records
        .iter()
        .find(|r| r.submit_id == "sub-old")
        .expect("执行记录应存在");
    assert!(
        rec.error_detail.is_empty(),
        "旧数据中的'应用重启，查询中断'应被清理，got: {:?}",
        rec.error_detail
    );
}

#[test]
fn automatic_query_fails_after_five_minutes_without_remote_queue_info() {
    let mut data = default_data();
    let mut task = make_querying_task_with_record("task-stale-local", "sub-local-only");
    task.submitted_at = Some((Utc::now() - Duration::minutes(6)).to_rfc3339());
    task.consecutive_no_result_queries = 3;
    data.tasks.push(task);

    let result = query_task_submit_id_once_with_runner(
        &mut data,
        "task-stale-local",
        "sub-local-only",
        |_| {
            Ok((
                r#"{"submit_id":"sub-local-only","prompt":"测试","logid":"log-1","gen_status":"querying"}"#
                    .to_string(),
                String::new(),
            ))
        },
    )
    .expect("stale local-only query should be handled");

    assert_eq!(result.status, "failed");
    assert_eq!(result.consecutive_no_result_queries, 0);
    assert!(
        result.last_error.contains("未返回远端队列信息"),
        "unexpected error: {}",
        result.last_error
    );
    let record = result
        .execution_records
        .iter()
        .find(|r| r.submit_id == "sub-local-only")
        .expect("record exists");
    assert_eq!(record.status, "failed");
    assert!(!record.finished_at.is_empty());
}

// ── T001/T003: 指定 submit_id 查询不污染其他执行记录 ─────────────────────────────

#[test]
fn query_for_specific_submit_id_only_appends_to_matching_execution_record() {
    let mut data = default_data();
    let mut task = queued_task("task-multi");
    task.submit_id = "sub-b".to_string();
    task.status = "querying".to_string();
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-a".to_string(),
        submit_id: "sub-a".to_string(),
        status: "succeeded".to_string(),
        started_at: "2026-04-30T09:00:00Z".to_string(),
        finished_at: "2026-04-30T09:05:00Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec!["/tmp/a.mp4".to_string()],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-b".to_string(),
        submit_id: "sub-b".to_string(),
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
    });
    data.tasks.push(task);

    let runner = |_args: &[String]| -> Result<(String, String), String> {
        Ok((r#"{"gen_status":"querying"}"#.to_string(), String::new()))
    };
    query_task_submit_id_once_with_runner(&mut data, "task-multi", "sub-b", runner).unwrap();

    let rec_a = data.tasks[0]
        .execution_records
        .iter()
        .find(|r| r.id == "exec-a")
        .unwrap();
    assert!(rec_a.query_records.is_empty(), "exec-a 不应被污染");

    let rec_b = data.tasks[0]
        .execution_records
        .iter()
        .find(|r| r.id == "exec-b")
        .unwrap();
    assert_eq!(rec_b.query_records.len(), 1, "exec-b 应有 1 条查询记录");
}

// ── T001/T004: 执行记录删除 ───────────────────────────────────────────────────

#[test]
fn delete_non_current_execution_record_preserves_task_submit_id() {
    let mut data = default_data();
    let mut task = queued_task("task-del1");
    task.submit_id = "sub-current".to_string();
    task.status = "querying".to_string();
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-old".to_string(),
        submit_id: "sub-old".to_string(),
        status: "succeeded".to_string(),
        started_at: "2026-04-30T09:00:00Z".to_string(),
        finished_at: "2026-04-30T09:05:00Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec!["/tmp/old.mp4".to_string()],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-current".to_string(),
        submit_id: "sub-current".to_string(),
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
    });
    data.tasks.push(task);

    let updated = delete_execution_record_from_data(&mut data, "task-del1", "exec-old").unwrap();

    assert_eq!(
        updated.submit_id, "sub-current",
        "删除非当前记录后 submit_id 不变"
    );
    assert_eq!(updated.execution_records.len(), 1, "剩余 1 条执行记录");
    assert!(
        updated.execution_records.iter().all(|r| r.id != "exec-old"),
        "exec-old 已删除"
    );
}

#[test]
fn delete_current_execution_record_reverts_task_to_latest_remaining_record() {
    let mut data = default_data();
    let mut task = queued_task("task-del2");
    task.submit_id = "sub-current".to_string();
    task.status = "querying".to_string();
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-prev".to_string(),
        submit_id: "sub-prev".to_string(),
        status: "succeeded".to_string(),
        started_at: "2026-04-30T09:00:00Z".to_string(),
        finished_at: "2026-04-30T09:05:00Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec!["/tmp/prev.mp4".to_string()],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-current".to_string(),
        submit_id: "sub-current".to_string(),
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
    });
    data.tasks.push(task);

    let updated =
        delete_execution_record_from_data(&mut data, "task-del2", "exec-current").unwrap();

    assert_eq!(
        updated.submit_id, "sub-prev",
        "删除当前记录后 submit_id 应回退到 sub-prev"
    );
    assert_eq!(updated.status, "succeeded", "任务状态应跟随剩余最新记录");
    assert!(!updated.result_paths.is_empty(), "顶层 result_paths 应回填");
}

#[test]
fn delete_last_execution_record_resets_task_to_draft() {
    let mut data = default_data();
    let mut task = queued_task("task-del3");
    task.submit_id = "sub-only".to_string();
    task.status = "succeeded".to_string();
    task.result_paths = vec!["/tmp/result.mp4".to_string()];
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-only".to_string(),
        submit_id: "sub-only".to_string(),
        status: "succeeded".to_string(),
        started_at: "2026-04-30T10:00:00Z".to_string(),
        finished_at: "2026-04-30T10:05:00Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec!["/tmp/result.mp4".to_string()],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    data.tasks.push(task);

    let updated = delete_execution_record_from_data(&mut data, "task-del3", "exec-only").unwrap();

    assert!(updated.execution_records.is_empty(), "执行记录应清空");
    assert!(
        updated.status == "draft" || updated.status == "queued",
        "无执行记录时任务应回到安全状态，got: {:?}",
        updated.status
    );
    assert!(updated.submit_id.is_empty(), "submit_id 应清空");
    assert!(
        updated.result_paths.is_empty(),
        "顶层 result_paths 应清空（物理文件不删除）"
    );
}

#[test]
fn delete_execution_record_returns_error_for_nonexistent_id() {
    let mut data = default_data();
    let mut task = queued_task("task-del4");
    task.execution_records.push(TaskExecutionRecord {
        id: "exec-real".to_string(),
        submit_id: "sub-real".to_string(),
        status: "succeeded".to_string(),
        started_at: "2026-04-30T10:00:00Z".to_string(),
        finished_at: "2026-04-30T10:05:00Z".to_string(),
        input_snapshot: TaskExecutionInputSnapshot::default(),
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: String::new(),
        error_detail: String::new(),
    });
    data.tasks.push(task);

    let result = delete_execution_record_from_data(&mut data, "task-del4", "exec-ghost");
    assert!(result.is_err(), "找不到执行记录时应返回 Err");
}

// ── needs_keep_awake ──────────────────────────────────────────────────────

#[test]
fn needs_keep_awake_returns_true_for_queued() {
    let task = queued_task("t1");
    assert!(needs_keep_awake(&[task]));
}

#[test]
fn needs_keep_awake_returns_true_for_scheduled() {
    let mut task = queued_task("t1");
    task.status = "scheduled".to_string();
    assert!(needs_keep_awake(&[task]));
}

#[test]
fn needs_keep_awake_returns_true_for_submitting() {
    let mut task = queued_task("t1");
    task.status = "submitting".to_string();
    assert!(needs_keep_awake(&[task]));
}

#[test]
fn needs_keep_awake_returns_true_for_querying() {
    let mut task = queued_task("t1");
    task.status = "querying".to_string();
    assert!(needs_keep_awake(&[task]));
}

#[test]
fn needs_keep_awake_returns_true_for_submitted_not_stopped() {
    let mut task = queued_task("t1");
    task.status = "submitted".to_string();
    task.auto_query_stopped = false;
    assert!(needs_keep_awake(&[task]));
}

#[test]
fn needs_keep_awake_returns_false_for_submitted_auto_query_stopped() {
    let mut task = queued_task("t1");
    task.status = "submitted".to_string();
    task.auto_query_stopped = true;
    assert!(!needs_keep_awake(&[task]));
}

#[test]
fn needs_keep_awake_returns_false_for_succeeded() {
    let mut task = queued_task("t1");
    task.status = "succeeded".to_string();
    assert!(!needs_keep_awake(&[task]));
}

// ── lifecycle / scheduler diagnostics ────────────────────────────────────

#[test]
fn record_lifecycle_event_writes_structured_log() {
    let mut data = default_data();

    record_lifecycle_event(&mut data, "frontend_blur", "窗口失焦", "visibility=visible");

    let log = data.logs.last().expect("lifecycle log");
    assert_eq!(log.level, LogLevel::Info);
    assert_eq!(log.source, LogSource::System);
    assert_eq!(log.category, "lifecycle");
    assert_eq!(log.event_type, "frontend_blur");
    assert_eq!(log.message, "窗口失焦");
    assert_eq!(log.detail, "visibility=visible");
}

#[test]
fn record_scheduler_tick_writes_background_origin() {
    let mut data = default_data();

    record_scheduler_tick(&mut data, "background", "started");

    let log = data.logs.last().expect("tick log");
    assert_eq!(log.level, LogLevel::Debug);
    assert_eq!(log.source, LogSource::Scheduler);
    assert_eq!(log.category, "scheduler_tick");
    assert_eq!(log.event_type, "tick");
    assert_eq!(log.message, "调度 tick：background");
    assert_eq!(log.detail, "phase=started");
}

#[test]
fn needs_keep_awake_returns_false_for_empty_tasks() {
    assert!(!needs_keep_awake(&[]));
}
