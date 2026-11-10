//! 配置解析 / 校验、`api_version` 门禁、`settings_spec` 形状、Windows 入口解析。
//!
//! 这一组全是**纯函数**：不 spawn、不连端口、不需要 sidecar。

use live2d_ai_mod_system::{MOD_API_VERSION, ModError, ModFactory, ModSettingField};
use serde_json::json;

use crate::tests_support::services;
use crate::*;

#[test]
fn empty_config_is_defaults() {
    for config in [json!({}), json!(null)] {
        let parsed = CosyVoiceConfig::parse(&config).expect("空配置必须能解析");
        assert_eq!(parsed, CosyVoiceConfig::defaults());
        assert_eq!(parsed.program, "D:/live2dai/tts-cosyvoice3/start.cmd");
        assert_eq!(parsed.port, 8011);
        assert_eq!(parsed.voice, "dafeiyu");
        assert!(parsed.auto_start);
        assert_eq!(parsed.health_timeout_s, 60);
    }
}

#[test]
fn unknown_keys_are_ignored() {
    // namespaced 配置里可能有宿主写的附加键：不认识就忽略，不报错。
    let parsed = CosyVoiceConfig::parse(&json!({"port": 8081, "extra": {"a": 1}})).expect("可解析");
    assert_eq!(parsed.port, 8081);
}

#[test]
fn non_object_config_is_rejected_with_readable_message() {
    for config in [json!([]), json!("x"), json!(42), json!(true)] {
        let error = CosyVoiceConfig::parse(&config).expect_err("非对象必须报错");
        assert!(
            error.contains("JSON 对象"),
            "错误消息应说明期望对象：{error}"
        );
    }
}

#[test]
fn port_must_be_integer_in_range() {
    for bad in [
        json!(0),
        json!(65_536),
        json!(-1),
        json!(8011.5),
        json!("8011"),
        json!(true),
    ] {
        let error = CosyVoiceConfig::parse(&json!({ "port": bad })).expect_err("坏端口必须报错");
        assert!(error.contains("port"), "错误消息应点名 port：{error}");
    }
    for good in [1u16, 8011, 65_535] {
        let parsed = CosyVoiceConfig::parse(&json!({ "port": good })).expect("可解析");
        assert_eq!(parsed.port, good);
    }
}

#[test]
fn health_timeout_bounds() {
    for bad in [json!(0), json!(-5), json!(601), json!(1.5), json!("60")] {
        let error = CosyVoiceConfig::parse(&json!({ "health_timeout_s": bad }))
            .expect_err("坏超时必须报错");
        assert!(
            error.contains("health_timeout_s"),
            "错误消息应点名 health_timeout_s：{error}"
        );
    }
    for (good, expected) in [(1u64, 1u64), (60, 60), (600, 600)] {
        let parsed = CosyVoiceConfig::parse(&json!({ "health_timeout_s": good })).expect("可解析");
        assert_eq!(parsed.health_timeout_s, expected);
    }
}

#[test]
fn voice_and_auto_start_are_validated() {
    let error = CosyVoiceConfig::parse(&json!({"voice": "   "})).expect_err("空音色必须报错");
    assert!(error.contains("voice"), "错误消息应点名 voice：{error}");
    let error = CosyVoiceConfig::parse(&json!({"auto_start": "yes"})).expect_err("类型错必须报错");
    assert!(error.contains("auto_start"));
    let parsed = CosyVoiceConfig::parse(&json!({"voice": "  dafeiyu  ", "auto_start": false}))
        .expect("可解析");
    assert_eq!(parsed.voice, "dafeiyu");
    assert!(!parsed.auto_start);
}

#[test]
fn empty_program_falls_back_to_default_but_bad_type_errors() {
    let parsed = CosyVoiceConfig::parse(&json!({"program": "  "})).expect("空 program = 缺省");
    assert_eq!(parsed.program, DEFAULT_PROGRAM);
    let error = CosyVoiceConfig::parse(&json!({"program": 42})).expect_err("类型错必须报错");
    assert!(error.contains("program"));
    let parsed = CosyVoiceConfig::parse(&json!({"program": " E:/tts/start.cmd "})).expect("可解析");
    assert_eq!(parsed.program, "E:/tts/start.cmd");
}

#[test]
fn derived_urls_and_workdir() {
    let mut config = CosyVoiceConfig::defaults();
    config.port = 8123;
    assert_eq!(config.health_url(), "http://127.0.0.1:8123/healthz");
    assert_eq!(config.openai_base_url(), "http://127.0.0.1:8123/v1");
    assert_eq!(
        config.workdir().expect("缺省启动器有父目录"),
        std::path::PathBuf::from("D:/live2dai/tts-cosyvoice3")
    );
    let config = CosyVoiceConfig::parse(&json!({"program": "start.cmd"})).expect("可解析");
    assert_eq!(
        config.workdir(),
        None,
        "相对路径没有父目录 → 不设 current_dir"
    );
}

#[test]
fn descriptor_is_the_agreed_identity() {
    // id 必须是「小写加连字符」：只允许 a-z / 0-9 / '-'，且首字符是字母。
    let id = DESCRIPTOR.id;
    assert_eq!(id, "cosyvoice3-tts");
    assert!(
        id.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    );
    assert!(id.starts_with(|c: char| c.is_ascii_lowercase()));
    assert_eq!(DESCRIPTOR.api_version, MOD_API_VERSION);
    assert_eq!(MOD_API_VERSION, 1, "本 Mod 对齐的 API 版本是 1");
    assert!(!DESCRIPTOR.name.trim().is_empty());
    assert!(!DESCRIPTOR.version.trim().is_empty());
    assert_eq!(COSYVOICE3_FACTORY.descriptor().id, id);
}

#[test]
fn api_version_gate() {
    // 声明值与宿主一致 → 放行。
    assert!(check_api_version(DESCRIPTOR.id, MOD_API_VERSION).is_ok());
    for wrong in [MOD_API_VERSION + 1, MOD_API_VERSION - 1, 0] {
        let error = check_api_version(DESCRIPTOR.id, wrong).expect_err("版本不符必须 Err");
        match error {
            ModError::IncompatibleApi {
                mod_id,
                got,
                expected,
            } => {
                assert_eq!(mod_id, DESCRIPTOR.id);
                assert_eq!(got, wrong);
                assert_eq!(expected, MOD_API_VERSION);
            }
            other => panic!("应回 IncompatibleApi，实际 {other:?}"),
        }
    }
}

#[test]
fn settings_spec_shape() {
    let spec = spec::settings_spec();
    assert_eq!(spec.validate(), Ok(()), "字段 key 必须唯一");
    assert_eq!(spec.mod_id, "cosyvoice3-tts");
    assert_eq!(spec.title, DESCRIPTOR.name);
    assert_eq!(spec.version, 1);
    let keys: Vec<&str> = spec.fields.iter().map(ModSettingField::key).collect();
    assert_eq!(keys, spec::KEYS.to_vec(), "字段顺序/键是稳定契约");
    assert_eq!(keys.len(), 5);

    // program / voice / auto_start 的缺省值必须在 schema 里（表单回填用）。
    match &spec.fields[0] {
        ModSettingField::String {
            secret, default, ..
        } => {
            assert!(!secret);
            assert_eq!(default.as_deref(), Some(DEFAULT_PROGRAM));
        }
        other => panic!("program 应是 String，实际 {other:?}"),
    }
    assert_eq!(spec.fields[1].key(), "port");
    assert!(matches!(spec.fields[1], ModSettingField::Number { .. }));
    match &spec.fields[2] {
        ModSettingField::String { default, .. } => {
            assert_eq!(default.as_deref(), Some("dafeiyu"));
        }
        other => panic!("voice 应是 String，实际 {other:?}"),
    }
    match &spec.fields[3] {
        ModSettingField::Bool { default, .. } => assert!(*default),
        other => panic!("auto_start 应是 Bool，实际 {other:?}"),
    }
    // Number 没有 default 槽（契约如此）：缺省值写进 label，并由 parse 兜底。
    let label = match &spec.fields[4] {
        ModSettingField::Number {
            label, min, max, ..
        } => {
            assert_eq!(*min, 1.0);
            assert_eq!(*max, 600.0);
            label
        }
        other => panic!("health_timeout_s 应是 Number，实际 {other:?}"),
    };
    assert!(label.contains("60"), "label 里要写缺省值：{label}");
    let port_label = match &spec.fields[1] {
        ModSettingField::Number {
            label, min, max, ..
        } => {
            assert_eq!(*min, 1.0);
            assert_eq!(*max, 65_535.0);
            label
        }
        other => unreachable!("{other:?}"),
    };
    assert!(
        port_label.contains("8011"),
        "label 里要写缺省端口：{port_label}"
    );
    assert!(port_label.contains("/healthz"));
}

#[test]
fn factory_settings_spec_is_static_and_available_before_create() {
    let spec = COSYVOICE3_FACTORY
        .settings_spec()
        .expect("未启用也必须能拿到 schema");
    assert_eq!(spec.mod_id, DESCRIPTOR.id);
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn factory_rejects_bad_config_with_readable_error() {
    let error = COSYVOICE3_FACTORY
        .create(services(), json!({"port": 70_000}))
        .err()
        .expect("坏配置必须 create 失败");
    match error {
        ModError::Init { mod_id, message } => {
            assert_eq!(mod_id, DESCRIPTOR.id);
            assert!(message.contains("port"), "错误应点名 port：{message}");
        }
        other => panic!("应是 ModError::Init，实际 {other:?}"),
    }
    // 非对象配置同样在 create 就失败（不 spawn、不折成缺省）。
    assert!(COSYVOICE3_FACTORY.create(services(), json!([])).is_err());
}

#[test]
fn entry_argv_on_non_windows_is_passthrough() {
    assert_eq!(
        entry_argv(" /home/me/start.sh ", false).expect("可通过"),
        vec!["/home/me/start.sh".to_string()]
    );
    assert!(entry_argv("   ", false).is_err(), "空 program 必须报错");
}

#[test]
fn entry_argv_on_windows_picks_the_interpreter() {
    // .cmd / .bat → cmd.exe /C（缺省启动器就是这条路）
    assert_eq!(
        entry_argv("D:/live2dai/tts-cosyvoice3/start.cmd", true).expect("可通过"),
        vec![
            "cmd.exe".to_string(),
            "/C".to_string(),
            "D:/live2dai/tts-cosyvoice3/start.cmd".to_string()
        ]
    );
    assert_eq!(
        entry_argv("C:/x/start.BAT", true).expect("扩展名大小写不敏感"),
        vec![
            "cmd.exe".to_string(),
            "/C".to_string(),
            "C:/x/start.BAT".to_string()
        ]
    );
    // .exe → 原样
    assert_eq!(
        entry_argv("C:/x/server.exe", true).expect("可通过"),
        vec!["C:/x/server.exe".to_string()]
    );
    // .sh → 报错：本 Mod 是 Windows 原生，不经过 wsl.exe / bash（没有 WSL2 也能跑）。
    let error = entry_argv("D:/x/start.sh", true).expect_err("不能把 .sh 交给 wsl/bash");
    assert!(error.contains(".sh"), "错误应点名扩展名：{error}");
    assert!(error.contains("wsl.exe"), "错误应说明不经过 wsl：{error}");
    // .ps1 → powershell -File
    assert_eq!(
        entry_argv("D:/x/start.ps1", true).expect("可通过"),
        vec![
            "powershell.exe".to_string(),
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-File".to_string(),
            "D:/x/start.ps1".to_string()
        ]
    );
    // 不认识的扩展名 → 报错（不猜解释器）
    let error = entry_argv("D:/x/server.py", true).expect_err("不能猜解释器");
    assert!(error.contains(".py"), "错误应点名扩展名：{error}");
    assert!(
        error.contains("start.cmd"),
        "错误应给出可执行的下一步：{error}"
    );
    // 无扩展名 → 原样（可能真是个 exe）
    assert_eq!(
        entry_argv("C:/x/launcher", true).expect("可通过"),
        vec!["C:/x/launcher".to_string()]
    );
}
