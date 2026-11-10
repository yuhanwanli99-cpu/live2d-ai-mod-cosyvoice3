//! `settings_spec`（纯数据 schema，前端通用表单渲染）。
//!
//! 本 Mod **没有专用面板**（口径）：因此这里的 label 是用户能看到的**唯一**说明。
//! 边界与模板 `local-tts` 一致：只声明五个字段，不多、不少；key 是稳定契约，
//! 中文只在 label 里。
//!
//! # 缺省值怎么表达
//!
//! 契约面 `ModSettingField` 只有 `Bool` / `String` / `Select` 带 `default` 槽，
//! **`Number` 没有**。所以 `port` 与 `health_timeout_s` 的缺省值写在 label 里，
//! 并由 [`crate::config::CosyVoiceConfig::parse`] 兜底：表单没提交这个键（或提了
//! 空）时按 8011 / 60 跑，**与面板写的字一致**。
//!
//! # 为什么 `voice` 只是展示
//!
//! 真正决定合成音色的字段是用户 `live2d-ai.toml` 的 `[tts].voice`——本 Mod
//! **不写** `[tts]`（见模块头注）。这里留一个同名占位是为了让用户在一个地方
//! 对照「sidecar 里预缓存的那份音色」和「`[tts]` 里填的那份」是否一致。

use live2d_ai_mod_system::{ModSettingField, ModSettingsSpec};

use crate::config::{
    DEFAULT_AUTO_START, DEFAULT_HEALTH_TIMEOUT_S, DEFAULT_PORT, DEFAULT_PROGRAM, DEFAULT_VOICE,
    MAX_HEALTH_TIMEOUT_S, MAX_PORT, MIN_HEALTH_TIMEOUT_S, MIN_PORT, MOD_ID,
};

/// 本 Mod 的设置 schema（静态：未启用也能拿到，前端可「先填配置、再启用」）。
pub fn settings_spec() -> ModSettingsSpec {
    ModSettingsSpec {
        mod_id: MOD_ID.to_string(),
        title: crate::DESCRIPTOR.name.to_string(),
        version: 1,
        fields: vec![
            ModSettingField::String {
                key: "program".to_string(),
                label: format!(
                    "sidecar 启动器（Windows 的 .cmd / .exe / .ps1；空 = 缺省 {DEFAULT_PROGRAM}）"
                ),
                secret: false,
                default: Some(DEFAULT_PROGRAM.to_string()),
            },
            ModSettingField::Number {
                key: "port".to_string(),
                label: format!(
                    "sidecar 的 HTTP 端口（缺省 {DEFAULT_PORT}；探活地址 http://127.0.0.1:<port>/healthz）"
                ),
                min: f64::from(MIN_PORT),
                max: f64::from(MAX_PORT),
            },
            ModSettingField::String {
                key: "voice".to_string(),
                label: format!(
                    "音色 id（只展示，缺省 {DEFAULT_VOICE}；合成用 [tts].voice，本 Mod 不写它）"
                ),
                secret: false,
                default: Some(DEFAULT_VOICE.to_string()),
            },
            ModSettingField::Bool {
                key: "auto_start".to_string(),
                label: "启用本 Mod 时拉起 sidecar（缺省开；关掉 = 只探活，服务由你自己起）"
                    .to_string(),
                default: DEFAULT_AUTO_START,
            },
            ModSettingField::Number {
                key: "health_timeout_s".to_string(),
                label: format!(
                    "拉起后等 /healthz 的上限，秒（缺省 {DEFAULT_HEALTH_TIMEOUT_S}；范围 {MIN_HEALTH_TIMEOUT_S}..={MAX_HEALTH_TIMEOUT_S}）"
                ),
                min: MIN_HEALTH_TIMEOUT_S as f64,
                max: MAX_HEALTH_TIMEOUT_S as f64,
            },
        ],
    }
}

/// 五个字段 key 的**稳定顺序**（改 schema 必须同时改这里与单测）。
pub const KEYS: [&str; 5] = ["program", "port", "voice", "auto_start", "health_timeout_s"];
