use dreamina_scheduler_lib::{
    build_ai_title_request, build_multimodal2video_args, classify_dreamina_error,
    extract_generated_task_title, format_ai_model_test_log, format_image_model_settings_log,
    parse_credit_info, parse_imagegen_json_response, parse_submit_output, resolve_task_inputs,
    sanitize_generated_task_title, AiModelConfig, Asset, AssetKind, ImageModelConfig,
    ConcurrencyLimitPolicy, DreaminaErrorKind, Role, SchedulerSettings, TaskDraft, VideoParams,
};

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
    }
}

#[test]
fn image_model_settings_log_summarizes_active_config_without_secret() {
    let settings = SchedulerSettings {
        image_model_configs: vec![
            ImageModelConfig {
                id: "old".to_string(),
                name: "旧供应商".to_string(),
                base_url: "https://old.example/v1".to_string(),
                api_key: "old-secret".to_string(),
                model: "old-image".to_string(),
            },
            ImageModelConfig {
                id: "new".to_string(),
                name: "新供应商".to_string(),
                base_url: "https://new.example/v1".to_string(),
                api_key: "new-secret".to_string(),
                model: "new-image".to_string(),
            },
        ],
        active_image_model_id: "new".to_string(),
        image_model_config: None,
        ..SchedulerSettings::default()
    };

    let log = format_image_model_settings_log(&settings);

    assert!(log.contains("图片模型数量=2"));
    assert!(log.contains("active_image_model_id=new"));
    assert!(log.contains("当前图片模型=新供应商 / new-image"));
    assert!(log.contains("base_url=https://new.example/v1"));
    assert!(log.contains("api_key=已填写"));
    assert!(!log.contains("new-secret"));
}

#[test]
fn parse_imagegen_json_response_reports_html_admin_page() {
    let err = parse_imagegen_json_response(
        "<!doctype html><html lang=\"zh\"><head><title>New API</title></head></html>",
        "https://model.indata.cc/v2/images/generations",
    )
    .expect_err("html response should not parse as image generation json");

    assert!(err.contains("图片生成接口返回 HTML 页面"));
    assert!(err.contains("https://model.indata.cc/v2/images/generations"));
    assert!(err.contains("Base URL"));
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
        duration_seconds: Some(4.0),
        created_at: String::new(),
    }
}

#[test]
fn command_uses_only_prompt_mentioned_assets_and_rewrites_mentions_inline() {
    let task = TaskDraft {
        title: "semantic mentions".to_string(),
        prompt: "根据 @分镜图1 和 @女主厨师服 生成，声音用 @女主女主人声音，未匹配 @不存在"
            .to_string(),
        image_asset_ids: vec![
            "temp-1".to_string(),
            "img-chef".to_string(),
            "img-unused".to_string(),
        ],
        audio_asset_ids: vec!["aud-voice".to_string()],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: true,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec!["temp-1".to_string()],
        temp_image_paths: vec!["/tmp/shot.png".to_string()],
    };
    let assets = vec![
        image_asset("temp-1", "粘贴图片", "/tmp/shot.png"),
        image_asset("img-chef", "厨师服", "/tmp/chef.png"),
        image_asset("img-unused", "居家服", "/tmp/home.png"),
        audio_asset("aud-voice", "女主人声音", "/tmp/voice.mp3"),
    ];
    let roles = vec![Role {
        id: "role-hero".to_string(),
        name: "女主".to_string(),
        aliases: vec![],
        tags: vec![],
        description: String::new(),
        asset_ids: vec![
            "img-chef".to_string(),
            "img-unused".to_string(),
            "aud-voice".to_string(),
        ],
        created_at: String::new(),
        updated_at: String::new(),
    }];

    let resolved = resolve_task_inputs(&task, &assets, &roles).expect("inputs resolve");
    let args = build_multimodal2video_args(&task, &resolved).expect("args build");
    let prompt_arg = args
        .iter()
        .find(|arg| arg.starts_with("--prompt="))
        .expect("prompt arg");

    assert_eq!(resolved.image_asset_ids, vec!["temp-1", "img-chef"]);
    assert_eq!(resolved.audio_asset_ids, vec!["aud-voice"]);
    assert!(args.iter().any(|arg| arg == "--image=/tmp/shot.png"));
    assert!(args.iter().any(|arg| arg == "--image=/tmp/chef.png"));
    assert!(!args.iter().any(|arg| arg == "--image=/tmp/home.png"));
    assert!(args.iter().any(|arg| arg == "--audio=/tmp/voice.mp3"));
    assert!(prompt_arg.contains("根据 @图1 和 @图2 生成，声音用 @音频1"));
    assert!(prompt_arg.contains("@不存在"));
    assert!(!prompt_arg.contains("参考图片"));
    assert!(!prompt_arg.contains("女主厨师服"));
    assert!(!prompt_arg.contains("女主女主人声音"));
}

#[test]
fn video_params_default_duration_is_15_seconds() {
    assert_eq!(VideoParams::default().duration, 15);
}

#[test]
fn ai_title_request_supports_responses_and_chat_modes() {
    let responses = AiModelConfig {
        id: "r1".to_string(),
        name: "Responses".to_string(),
        api_mode: "responses".to_string(),
        api_key: "sk-test".to_string(),
        base_url: "https://example.com/v1/".to_string(),
        model: "gpt-test".to_string(),
    };
    let responses_request =
        build_ai_title_request(&responses, "女主和猫猫做饭").expect("responses request");
    assert_eq!(responses_request.url, "https://example.com/v1/responses");
    assert_eq!(responses_request.body["model"], "gpt-test");
    assert!(responses_request.body.get("input").is_some());

    let chat = AiModelConfig {
        api_mode: "chat".to_string(),
        ..responses
    };
    let chat_request = build_ai_title_request(&chat, "女主和猫猫做饭").expect("chat request");
    assert_eq!(chat_request.url, "https://example.com/v1/chat/completions");
    assert!(chat_request.body.get("messages").is_some());
}

#[test]
fn generated_task_title_is_extracted_and_sanitized() {
    let chat_payload = serde_json::json!({
        "choices": [{"message": {"content": "{\"title\":\"猫猫餐馆反转夸菜\"}"}}]
    });
    assert_eq!(
        extract_generated_task_title(&chat_payload).as_deref(),
        Some("猫猫餐馆反转夸菜")
    );

    assert_eq!(
        sanitize_generated_task_title("《女主做菜被猫夸到破防》\n多余解释"),
        "女主做菜被猫夸到破防"
    );
}

#[test]
fn ai_model_test_log_keeps_raw_response_without_secret() {
    let log = format_ai_model_test_log(
        "responses",
        "gpt-test",
        "https://example.com/v1/responses",
        "{\"status\":\"completed\",\"finish_reason\":\"stop\"}",
        Some("stop"),
        None,
    );

    assert!(log.contains("AI 模型测试"));
    assert!(log.contains("mode=responses"));
    assert!(log.contains("model=gpt-test"));
    assert!(log.contains("finish_reason"));
    assert!(log.contains("parsed=stop"));
    assert!(!log.contains("sk-"));
}

#[test]
fn ai_model_test_log_records_error_and_truncates_long_raw_response() {
    let raw = "x".repeat(5000);
    let log = format_ai_model_test_log(
        "chat",
        "gpt-test",
        "https://example.com/v1/chat/completions",
        &raw,
        None,
        Some("模型已响应但未返回文本"),
    );

    assert!(log.contains("error=模型已响应但未返回文本"));
    assert!(log.contains("日志已截断"));
    assert!(log.len() < 4600);
}

#[test]
fn multimodal_command_uses_only_images_and_audio_for_mvp() {
    let task = TaskDraft {
        title: "test".to_string(),
        prompt: "让警车威威在街边巡逻".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec!["aud-1".to_string()],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams {
            model_version: "seedance2.0".to_string(),
            ratio: "9:16".to_string(),
            duration: 5,
            video_resolution: "720p".to_string(),
        },
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![
        image_asset("img-1", "警车威威", "/tmp/role.png"),
        audio_asset("aud-1", "背景音乐", "/tmp/music.mp3"),
    ];

    let resolved = resolve_task_inputs(&task, &assets, &[]).expect("inputs resolve");
    let args = build_multimodal2video_args(&task, &resolved).expect("args build");

    assert_eq!(args[0], "multimodal2video");
    assert!(args.iter().any(|arg| arg == "--model_version=seedance2.0"));
    assert!(args.iter().any(|arg| arg == "--ratio=9:16"));
    assert!(args.iter().any(|arg| arg == "--image=/tmp/role.png"));
    assert!(args.iter().any(|arg| arg == "--audio=/tmp/music.mp3"));
    assert!(!args.iter().any(|arg| arg.starts_with("--video=")));
}

#[test]
fn multimodal_prompt_rewrites_image_mentions_inline() {
    let task = TaskDraft {
        title: "reference order".to_string(),
        prompt: "@女主小雅 跑进画面，@助手阿灯 举起提示灯".to_string(),
        image_asset_ids: vec!["img-1".to_string(), "img-2".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![
        image_asset("img-1", "女主小雅", "/tmp/xiaoya.png"),
        image_asset("img-2", "助手阿灯", "/tmp/adeng.png"),
    ];

    let resolved = resolve_task_inputs(&task, &assets, &[]).expect("inputs resolve");
    let args = build_multimodal2video_args(&task, &resolved).expect("args build");
    let prompt_arg = args
        .iter()
        .find(|arg| arg.starts_with("--prompt="))
        .expect("prompt arg");

    assert!(prompt_arg.contains("@图1 跑进画面，@图2 举起提示灯"));
    assert!(!prompt_arg.contains("参考图片"));
    assert!(!prompt_arg.contains("女主小雅"));
    assert!(!prompt_arg.contains("助手阿灯"));
}

#[test]
fn multimodal_prompt_rewrites_ordered_image_and_audio_mentions_inline() {
    let task = TaskDraft {
        title: "ordered mentions".to_string(),
        prompt: "根据 @素材名A 和 @素材名B 生成，声音 @音频名A @音频名B".to_string(),
        image_asset_ids: vec!["img-1".to_string(), "img-2".to_string()],
        audio_asset_ids: vec!["aud-1".to_string(), "aud-2".to_string()],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![
        image_asset("img-1", "素材名A", "/tmp/a.png"),
        image_asset("img-2", "素材名B", "/tmp/b.png"),
        audio_asset("aud-1", "音频名A", "/tmp/a.mp3"),
        audio_asset("aud-2", "音频名B", "/tmp/b.mp3"),
    ];

    let resolved = resolve_task_inputs(&task, &assets, &[]).expect("inputs resolve");
    let args = build_multimodal2video_args(&task, &resolved).expect("args build");
    let prompt_arg = args
        .iter()
        .find(|arg| arg.starts_with("--prompt="))
        .expect("prompt arg");

    assert!(prompt_arg.contains("根据 @图1 和 @图2 生成，声音 @音频1 @音频2"));
    assert!(!prompt_arg.contains("素材名A"));
    assert!(!prompt_arg.contains("音频名A"));
    assert!(!prompt_arg.contains("参考图片"));
    assert!(!prompt_arg.contains("参考音频"));
}

#[test]
fn storyboard_mentions_use_temp_images_not_role_image_order() {
    let task = TaskDraft {
        title: "storyboard temp image".to_string(),
        prompt: "根据分镜图 @分镜图2 女主是 @女主居家服2，猫猫是 @黑白猫和灰猫黑白猫和灰猫参考图"
            .to_string(),
        image_asset_ids: vec!["cat-ref".to_string(), "hero-home".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec!["storyboard-temp".to_string()],
        temp_image_paths: vec!["/tmp/storyboard.png".to_string()],
    };
    let assets = vec![
        image_asset("storyboard-temp", "粘贴图片", "/tmp/storyboard.png"),
        image_asset("cat-ref", "黑白猫和灰猫参考图", "/tmp/cat.png"),
        image_asset("hero-home", "居家服2", "/tmp/hero.png"),
    ];
    let roles = vec![
        Role {
            id: "hero".to_string(),
            name: "女主".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["hero-home".to_string()],
            created_at: String::new(),
            updated_at: String::new(),
        },
        Role {
            id: "cat".to_string(),
            name: "黑白猫和灰猫".to_string(),
            aliases: vec![],
            tags: vec![],
            description: String::new(),
            asset_ids: vec!["cat-ref".to_string()],
            created_at: String::new(),
            updated_at: String::new(),
        },
    ];

    let resolved = resolve_task_inputs(&task, &assets, &roles).expect("inputs resolve");
    let args = build_multimodal2video_args(&task, &resolved).expect("args build");
    let prompt_arg = args
        .iter()
        .find(|arg| arg.starts_with("--prompt="))
        .expect("prompt arg");

    assert_eq!(
        resolved.image_asset_ids,
        vec!["storyboard-temp", "hero-home", "cat-ref"]
    );
    assert!(prompt_arg.contains("根据分镜图 @图1 女主是 @图2，猫猫是 @图3"));
}

#[test]
fn rejects_unsupported_ratio_before_command_build() {
    let task = TaskDraft {
        title: "bad ratio".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams {
            model_version: "seedance2.0".to_string(),
            ratio: "2:1".to_string(),
            duration: 5,
            video_resolution: "720p".to_string(),
        },
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![image_asset("img-1", "角色图", "/tmp/role.png")];
    let resolved = resolve_task_inputs(&task, &assets, &[]).expect("inputs resolve");

    let err = build_multimodal2video_args(&task, &resolved).expect_err("bad ratio rejected");
    assert!(err.to_string().contains("ratio"));
}

#[test]
fn auto_match_does_not_add_unmentioned_role_media_to_command_inputs() {
    let task = TaskDraft {
        title: "auto match".to_string(),
        prompt: "警车威威开进画面，陌生角色在远处挥手".to_string(),
        image_asset_ids: vec![],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: true,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![image_asset("img-wewei", "威威正面", "/tmp/wewei.png")];
    let roles = vec![Role {
        id: "role-wewei".to_string(),
        name: "警车威威".to_string(),
        aliases: vec!["威威".to_string()],
        tags: vec!["警车".to_string()],
        description: "蓝白警车".to_string(),
        asset_ids: vec!["img-wewei".to_string()],
        created_at: String::new(),
        updated_at: String::new(),
    }];

    let result = resolve_task_inputs(&task, &assets, &roles);

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("至少需要 1 个图片"));
}

#[test]
fn manual_role_mentions_do_not_bind_assets_without_asset_reference() {
    let task = TaskDraft {
        title: "mentions".to_string(),
        prompt: "@警车威威 和 @不存在角色 一起出场".to_string(),
        image_asset_ids: vec![],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![image_asset("img-wewei", "威威正面", "/tmp/wewei.png")];
    let roles = vec![Role {
        id: "role-wewei".to_string(),
        name: "警车威威".to_string(),
        aliases: vec![],
        tags: vec![],
        description: "".to_string(),
        asset_ids: vec!["img-wewei".to_string()],
        created_at: String::new(),
        updated_at: String::new(),
    }];

    let result = resolve_task_inputs(&task, &assets, &roles);

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("至少需要 1 个图片"));
}

#[test]
fn classifies_concurrency_limit_for_silent_retry_policy() {
    let settings = SchedulerSettings {
        concurrency_limit_policy: ConcurrencyLimitPolicy::SilentRetry,
        concurrency_retry_delay_seconds: 300,
        concurrency_retry_max_attempts: 8,
        auto_query_enabled: true,
        poll_interval_seconds: 60,
        log_retention_count: 500,
        mac_install_command: "curl -fsSL https://jimeng.jianying.com/cli | bash".to_string(),
        windows_install_command: String::new(),
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
fn parses_submit_id_from_json_or_text_output() {
    let json = parse_submit_output(r#"{"submit_id":"abc123","gen_status":"querying"}"#);
    assert_eq!(json.submit_id.as_deref(), Some("abc123"));
    assert_eq!(json.gen_status.as_deref(), Some("querying"));

    let text = parse_submit_output("submit_id: xyz789\ngen_status: success");
    assert_eq!(text.submit_id.as_deref(), Some("xyz789"));
    assert_eq!(text.gen_status.as_deref(), Some("success"));
}

#[test]
fn rejects_video_asset_type() {
    let task = TaskDraft {
        title: "video test".to_string(),
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
    };
    let video_asset = Asset {
        id: "vid-1".to_string(),
        kind: AssetKind::Image,
        name: "视频".to_string(),
        aliases: vec![],
        tags: vec![],
        stored_path: "/tmp/video.mp4".to_string(),
        source_path: "/tmp/video.mp4".to_string(),
        mime: "video/mp4".to_string(),
        size_bytes: 100,
        duration_seconds: Some(5.0),
        created_at: String::new(),
    };
    let assets = vec![image_asset("img-1", "角色图", "/tmp/role.png"), video_asset];
    let result = resolve_task_inputs(&task, &assets, &[]);
    assert!(result.is_ok());
}

#[test]
fn missing_asset_id_returns_error() {
    let task = TaskDraft {
        title: "missing asset".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["nonexistent".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let result = resolve_task_inputs(&task, &[], &[]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("找不到素材"));
}

#[test]
fn role_ids_are_ignored_by_command_input_resolution() {
    let task = TaskDraft {
        title: "missing role".to_string(),
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
    };
    let assets = vec![image_asset("img-1", "角色图", "/tmp/role.png")];
    let result = resolve_task_inputs(&task, &assets, &[]);
    let resolved = result.expect("role ids should not affect command input resolution");
    assert_eq!(resolved.image_asset_ids, vec!["img-1"]);
}

#[test]
fn no_image_input_returns_error() {
    let task = TaskDraft {
        title: "no image".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec![],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let result = resolve_task_inputs(&task, &[], &[]);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("至少需要 1 个图片输入"));
}

#[test]
fn too_many_images_returns_error() {
    let mut image_ids = Vec::new();
    let mut assets = Vec::new();
    for i in 0..10 {
        let id = format!("img-{}", i);
        image_ids.push(id.clone());
        assets.push(image_asset(
            &id,
            &format!("图{}", i),
            &format!("/tmp/{}.png", i),
        ));
    }
    let task = TaskDraft {
        title: "too many images".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: image_ids,
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let result = resolve_task_inputs(&task, &assets, &[]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("图片最多支持"));
}

#[test]
fn too_many_audio_returns_error() {
    let mut audio_ids = Vec::new();
    let mut assets = vec![image_asset("img-1", "角色图", "/tmp/role.png")];
    for i in 0..4 {
        let id = format!("aud-{}", i);
        audio_ids.push(id.clone());
        assets.push(audio_asset(
            &id,
            &format!("音{}", i),
            &format!("/tmp/{}.mp3", i),
        ));
    }
    let task = TaskDraft {
        title: "too many audio".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: audio_ids,
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let result = resolve_task_inputs(&task, &assets, &[]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("音频最多支持"));
}

#[test]
fn rejects_unsupported_model_version() {
    let task = TaskDraft {
        title: "bad model".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams {
            model_version: "unknown-model".to_string(),
            ratio: "9:16".to_string(),
            duration: 5,
            video_resolution: "720p".to_string(),
        },
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![image_asset("img-1", "角色图", "/tmp/role.png")];
    let resolved = resolve_task_inputs(&task, &assets, &[]).expect("inputs resolve");
    let err = build_multimodal2video_args(&task, &resolved).expect_err("bad model rejected");
    assert!(err.to_string().contains("model_version"));
}

#[test]
fn rejects_unsupported_duration() {
    let task = TaskDraft {
        title: "bad duration".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams {
            model_version: "seedance2.0".to_string(),
            ratio: "9:16".to_string(),
            duration: 20,
            video_resolution: "720p".to_string(),
        },
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![image_asset("img-1", "角色图", "/tmp/role.png")];
    let resolved = resolve_task_inputs(&task, &assets, &[]).expect("inputs resolve");
    let err = build_multimodal2video_args(&task, &resolved).expect_err("bad duration rejected");
    assert!(err.to_string().contains("duration"));
}

#[test]
fn rejects_unsupported_resolution() {
    let task = TaskDraft {
        title: "bad resolution".to_string(),
        prompt: "测试".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams {
            model_version: "seedance2.0".to_string(),
            ratio: "9:16".to_string(),
            duration: 5,
            video_resolution: "1080p".to_string(),
        },
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![image_asset("img-1", "角色图", "/tmp/role.png")];
    let resolved = resolve_task_inputs(&task, &assets, &[]).expect("inputs resolve");
    let err = build_multimodal2video_args(&task, &resolved).expect_err("bad resolution rejected");
    assert!(err.to_string().contains("720p"));
}

#[test]
fn duplicate_at_mentions_do_not_produce_duplicate_asset_ids() {
    let task = TaskDraft {
        title: "dup mentions".to_string(),
        prompt: "@威威正面 @威威正面 一起巡逻".to_string(),
        image_asset_ids: vec!["img-wewei".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![image_asset("img-wewei", "威威正面", "/tmp/wewei.png")];
    let roles = vec![Role {
        id: "role-wewei".to_string(),
        name: "警车威威".to_string(),
        aliases: vec![],
        tags: vec![],
        description: String::new(),
        asset_ids: vec!["img-wewei".to_string()],
        created_at: String::new(),
        updated_at: String::new(),
    }];
    let resolved = resolve_task_inputs(&task, &assets, &roles).expect("inputs resolve");
    assert_eq!(resolved.image_asset_ids.len(), 1);
    assert!(resolved.manual_mention_ids.is_empty());
    assert_eq!(resolved.prompt_rewrites.len(), 1);
}

#[test]
fn auto_match_off_does_not_bind_role_media() {
    let task = TaskDraft {
        title: "auto off".to_string(),
        prompt: "警车威威开进画面".to_string(),
        image_asset_ids: vec!["img-1".to_string()],
        audio_asset_ids: vec![],
        role_ids: vec![],
        manual_mention_ids: vec![],
        auto_match_roles: false,
        params: VideoParams::default(),
        scheduled_at: None,
        temp_image_asset_ids: vec![],
        temp_image_paths: vec![],
    };
    let assets = vec![
        image_asset("img-1", "角色图", "/tmp/role.png"),
        image_asset("img-wewei", "威威正面", "/tmp/wewei.png"),
    ];
    let roles = vec![Role {
        id: "role-wewei".to_string(),
        name: "警车威威".to_string(),
        aliases: vec![],
        tags: vec![],
        description: String::new(),
        asset_ids: vec!["img-wewei".to_string()],
        created_at: String::new(),
        updated_at: String::new(),
    }];
    let resolved = resolve_task_inputs(&task, &assets, &roles).expect("inputs resolve");
    assert_eq!(resolved.image_asset_ids, vec!["img-1"]);
    assert!(resolved.matched_role_ids.is_empty());
}

#[test]
fn transient_error_classified_as_retry() {
    let settings = SchedulerSettings::default();
    let classified = classify_dreamina_error("Unexpected end of JSON input", &settings);
    assert_eq!(classified.kind, DreaminaErrorKind::Transient);
    assert_eq!(classified.next_status, "retry_wait");
    assert!(classified.retry_after_seconds.is_some());
}

#[test]
fn compliance_required_error_classified_as_blocked() {
    let settings = SchedulerSettings::default();
    let classified =
        classify_dreamina_error("api error: AigcComplianceConfirmationRequired", &settings);
    assert_eq!(classified.kind, DreaminaErrorKind::ComplianceRequired);
    assert_eq!(classified.next_status, "blocked");
}

#[test]
fn generic_error_classified_as_failed() {
    let settings = SchedulerSettings::default();
    let classified = classify_dreamina_error("some unknown error occurred", &settings);
    assert_eq!(classified.kind, DreaminaErrorKind::Generic);
    assert_eq!(classified.next_status, "failed");
}

#[test]
fn parse_credit_info_from_json() {
    let info = parse_credit_info(r#"{"total":"100","used":"30","remaining":"70"}"#);
    assert!(info.available);
    assert_eq!(info.total, "100");
    assert_eq!(info.used, "30");
    assert_eq!(info.remaining, "70");
}

#[test]
fn parse_credit_info_from_text() {
    let text = "总额度: 200\n已使用: 50\n剩余: 150";
    let info = parse_credit_info(text);
    assert!(info.available);
    assert_eq!(info.total, "200");
    assert_eq!(info.used, "50");
    assert_eq!(info.remaining, "150");
}

#[test]
fn parse_credit_info_empty_returns_unavailable() {
    let info = parse_credit_info("");
    assert!(!info.available);
}

#[test]
fn concurrency_limit_chinese_keywords_detected() {
    let settings = SchedulerSettings::default();
    let classified = classify_dreamina_error("并发上限已达到", &settings);
    assert_eq!(classified.kind, DreaminaErrorKind::ConcurrencyLimit);
}

#[test]
fn timeout_error_classified_as_transient() {
    let settings = SchedulerSettings::default();
    let classified = classify_dreamina_error("context deadline exceeded", &settings);
    assert_eq!(classified.kind, DreaminaErrorKind::Transient);
    assert_eq!(classified.next_status, "retry_wait");
}
