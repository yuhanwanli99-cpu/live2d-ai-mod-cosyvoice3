//! 生命周期：探活 → 拉起 → 按秒轮询 → 就绪 / 失败；停用只收自己拉起的进程。
//!
//! 全部注入假 spawner + 假探活：**不 exec 真进程、不真连 sidecar**。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use live2d_ai_mod_system::{ModError, ModFactory, ModRuntime};
use serde_json::json;

use crate::tests_support::*;
use crate::*;

/// 临时启动器的路径字符串（塞进 `program`）。
fn program_str(program: &TempProgram) -> String {
    program.path.to_string_lossy().into_owned()
}

/// `state_json` 的 key 集合（**顺序无关，集合固定六个**）。
fn keys_of(value: &serde_json::Value) -> Vec<String> {
    value
        .as_object()
        .expect("state_json 必须是对象")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn fresh_runtime_reports_disabled_shape() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(false);
    let port = free_port();
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": port, "voice": "dafeiyu"}),
        &spawner,
        &probe,
    );
    let state = snapshot(&mut runtime);
    assert_eq!(
        keys_of(&state),
        vec![
            "last_error".to_string(),
            "pid".to_string(),
            "port".to_string(),
            "running".to_string(),
            "status".to_string(),
            "voice".to_string()
        ],
        "state_json 必须是这六个键"
    );
    assert_eq!(state["status"], json!("disabled"));
    assert_eq!(state["running"], json!(false));
    assert!(state["pid"].is_null());
    assert!(state["last_error"].is_null());
    assert_eq!(state["port"], json!(port));
    assert_eq!(state["voice"], json!("dafeiyu"));
}

#[test]
fn external_ready_sidecar_is_adopted_without_spawn() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(true);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    start_rt(&mut runtime).expect("已就绪就该直接 ready");
    let state = snapshot(&mut runtime);
    assert_eq!(state["status"], json!("ready"));
    assert_eq!(state["running"], json!(true));
    assert!(state["pid"].is_null(), "外部服务没有我们拉起的 pid");
    assert_eq!(spawner.calls(), 0, "已就绪就不许 spawn");
    // 停用只落状态，**不碰**外部服务。
    runtime.shutdown().expect("停用应成功");
    assert_eq!(spawner.calls(), 0);
    assert_eq!(snapshot(&mut runtime)["status"], json!("disabled"));
}

#[test]
fn spawn_then_ready_and_stop_own_child() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::new(vec![false, true], true);
    let port = free_port();
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": port}),
        &spawner,
        &probe,
    );
    start_rt(&mut runtime).expect("spawn 后立刻就绪 → Ok");
    assert_eq!(spawner.calls(), 1);
    let request = spawner.last_request();
    assert_eq!(
        request.argv,
        entry_argv(&program_str(&program), cfg!(windows)).expect("argv 应可解析"),
        "argv 必须由 entry_argv 决定（Windows 上 .cmd 经 cmd.exe /C）"
    );
    assert_eq!(
        request.workdir.as_deref(),
        Some(program.path.parent().expect("父目录")),
        "工作目录 = 启动器所在目录"
    );
    assert!(probe.calls() >= 2, "spawn 之后要按轮询再探活");
    let state = snapshot(&mut runtime);
    assert_eq!(state["status"], json!("ready"));
    assert_eq!(state["pid"], json!(4242));
    // 停用：收掉自己拉起的那个。
    runtime.shutdown().expect("停用应成功");
    assert_eq!(spawner.with_state(|s| s.stops), 1);
    assert_eq!(spawner.with_state(|s| s.waits), 1);
    let state = snapshot(&mut runtime);
    assert_eq!(state["status"], json!("disabled"));
    assert_eq!(state["running"], json!(false));
    assert!(state["pid"].is_null());
}

#[test]
fn health_timeout_fails_and_stops_owned_child() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(false);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port(), "health_timeout_s": 1}),
        &spawner,
        &probe,
    );
    let started = Instant::now();
    let error = start_rt(&mut runtime).expect_err("一直不就绪 → Err");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "超时必须按时收口"
    );
    let message = error.to_string();
    assert!(
        message.contains("health_timeout"),
        "错误要带原因码：{message}"
    );
    assert!(message.contains("/healthz"), "错误要带探活地址：{message}");
    assert_eq!(spawner.calls(), 1);
    assert_eq!(
        spawner.with_state(|s| s.stops),
        1,
        "超时窗口内拉起的进程必须被收掉（不留占端口的半成品）"
    );
    let state = snapshot(&mut runtime);
    assert_eq!(state["status"], json!("failed"));
    assert_eq!(state["running"], json!(false));
    assert_eq!(state["last_error"], json!("health_timeout"));
    assert!(state["pid"].is_null());
}

#[test]
fn program_missing_is_reported_before_spawn() {
    let missing = TempProgram::missing();
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(false);
    let mut runtime = build_runtime(
        json!({"program": missing.to_string_lossy(), "port": free_port()}),
        &spawner,
        &probe,
    );
    let error = start_rt(&mut runtime).expect_err("启动器不在 → Err");
    let message = error.to_string();
    assert!(message.contains("program_missing"), "{message}");
    assert!(message.contains("start.cmd"), "错误要给出下一步：{message}");
    assert_eq!(spawner.calls(), 0, "启动器不在就不许 spawn");
    assert_eq!(
        snapshot(&mut runtime)["last_error"],
        json!("program_missing")
    );
}

#[test]
fn port_in_use_is_reported_without_second_process() {
    let program = TempProgram::new("cmd");
    let (port, listener) = occupied_port();
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(false);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": port}),
        &spawner,
        &probe,
    );
    let error = start_rt(&mut runtime).expect_err("端口被占 → Err");
    let message = error.to_string();
    assert!(message.contains("port_in_use"), "{message}");
    assert_eq!(spawner.calls(), 0, "端口被别的进程占着就不许硬起第二个");
    assert_eq!(snapshot(&mut runtime)["last_error"], json!("port_in_use"));
    drop(listener);
}

#[test]
fn spawn_error_maps_to_spawn_failed() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    spawner.spawn_fails("python.exe 没有安装");
    let probe = ProbeScript::stable(false);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    let error = start_rt(&mut runtime).expect_err("拉不起来 → Err");
    let message = error.to_string();
    assert!(message.contains("spawn_failed"), "{message}");
    assert!(
        message.contains("python.exe 没有安装"),
        "错误要带底层原因：{message}"
    );
    let state = snapshot(&mut runtime);
    assert_eq!(state["status"], json!("failed"));
    assert_eq!(state["last_error"], json!("spawn_failed"));
}

#[test]
fn child_exiting_before_ready_maps_to_spawn_failed_with_stderr() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    spawner.child_exits_on_spawn(Some(1), "CUDA out of memory");
    let probe = ProbeScript::stable(false);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port(), "health_timeout_s": 5}),
        &spawner,
        &probe,
    );
    let error = start_rt(&mut runtime).expect_err("就绪前退出 → Err");
    let message = error.to_string();
    assert!(message.contains("spawn_failed"), "{message}");
    assert!(
        message.contains("CUDA out of memory"),
        "错误要带启动器 stderr 的尾巴：{message}"
    );
    let state = snapshot(&mut runtime);
    assert_eq!(state["last_error"], json!("spawn_failed"));
    assert!(state["pid"].is_null());
}

#[test]
fn auto_start_off_only_probes() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(false);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port(), "auto_start": false}),
        &spawner,
        &probe,
    );
    start_rt(&mut runtime).expect("auto_start=false 时 start 不该失败");
    assert_eq!(spawner.calls(), 0, "auto_start=false 绝不 spawn");
    let state = snapshot(&mut runtime);
    assert_eq!(state["status"], json!("disabled"));
    assert!(state["last_error"].is_null(), "没尝试就没失败原因");

    // 同一个开关，但服务已经在跑：如实报 ready（用户自己起的服务）。
    let probe = ProbeScript::stable(true);
    let spawner = Arc::new(SpawnerLog::default());
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port(), "auto_start": false}),
        &spawner,
        &probe,
    );
    start_rt(&mut runtime).expect("Ok");
    assert_eq!(snapshot(&mut runtime)["status"], json!("ready"));
    assert_eq!(spawner.calls(), 0);
}

#[test]
fn drop_stops_owned_child() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::new(vec![false, true], true);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    start_rt(&mut runtime).expect("Ok");
    assert_eq!(spawner.calls(), 1);
    drop(runtime);
    assert_eq!(
        spawner.with_state(|s| s.stops),
        1,
        "drop 也要收掉自己拉起的子进程（不留占端口的孤儿）"
    );
}

#[test]
fn host_status_mapping_matches_existing_mods() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(true);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    // 未启用 → Disabled
    assert_eq!(runtime.state().host_status().as_str(), "disabled");
    start_rt(&mut runtime).expect("Ok");
    assert_eq!(runtime.state().host_status().as_str(), "running");
    runtime.shutdown().expect("Ok");
    assert_eq!(runtime.state().host_status().as_str(), "disabled");
}

#[test]
fn apply_config_is_atomic() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(true);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    let before = snapshot(&mut runtime);
    // 1) 坏配置：可读错误 + 运行态一个字段都不改。
    let error = runtime
        .apply_config(&json!({"port": 70_000}))
        .expect_err("坏配置必须 Err");
    match error {
        ModError::InvalidSettings { mod_id, message } => {
            assert_eq!(mod_id, "cosyvoice3-tts");
            assert!(message.contains("port"), "{message}");
        }
        other => panic!("应是 InvalidSettings，实际 {other:?}"),
    }
    assert_eq!(snapshot(&mut runtime), before, "坏配置不许改运行态");
    // 2) 好配置：空闲时可以应用。
    runtime
        .apply_config(&json!({"port": 9011, "voice": "other"}))
        .expect("空闲时可应用");
    let state_after = snapshot(&mut runtime);
    assert_eq!(state_after["port"], json!(9011));
    assert_eq!(state_after["voice"], json!("other"));
    // 3) 运行中拒绝原地改（否则 state_json 会与真实端口不一致）。
    start_rt(&mut runtime).expect("Ok");
    let error = runtime
        .apply_config(&json!({"port": 9022}))
        .expect_err("运行中必须拒绝");
    assert!(error.to_string().contains("运行中"), "{error}");
    assert_eq!(snapshot(&mut runtime)["port"], json!(9011));
    runtime.shutdown().expect("Ok");
    // 4) 别名走同一条路径。
    runtime
        .apply_settings(&json!({"port": 9033}))
        .expect("apply_settings 是 apply_config 的别名");
    assert_eq!(snapshot(&mut runtime)["port"], json!(9033));
}

#[test]
fn commands_are_start_stop_probe_restart_only() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::new(vec![false, true, true, false, true], true);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    // start
    let result = runtime.command("start", &json!({})).expect("start 命令");
    assert_eq!(result["status"], json!("ready"));
    // probe（假探活继续回 true）
    let result = runtime.command("probe", &json!({})).expect("probe 命令");
    assert_eq!(result["healthy"], json!(true));
    // restart：先停再起（第二次 spawn）
    let result = runtime
        .command("restart", &json!({}))
        .expect("restart 命令");
    assert_eq!(result["status"], json!("ready"));
    assert_eq!(spawner.calls(), 2);
    assert_eq!(spawner.total_stops(), 1, "restart = 停旧的那个 + 起新的");
    // stop
    let result = runtime.command("stop", &json!({})).expect("stop 命令");
    assert_eq!(result["status"], json!("disabled"));
    // 未知命令 → UnsupportedCommand（宿主回 409 unsupported_command）
    match runtime.command("reboot", &json!({})) {
        Err(ModError::UnsupportedCommand { command }) => assert_eq!(command, "reboot"),
        other => panic!("应是 UnsupportedCommand，实际 {other:?}"),
    }
}

#[test]
fn probe_command_marks_failed_when_ready_service_disappears() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    // 先 ready（外部已就绪），之后探活一直 false。
    let probe = ProbeScript::new(vec![true], false);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    start_rt(&mut runtime).expect("Ok");
    assert_eq!(snapshot(&mut runtime)["status"], json!("ready"));
    let result = runtime.command("probe", &json!({})).expect("probe 命令");
    assert_eq!(result["healthy"], json!(false));
    assert_eq!(result["status"], json!("failed"));
    assert_eq!(
        snapshot(&mut runtime)["last_error"],
        json!("health_timeout")
    );
}

#[test]
fn stop_failure_keeps_handle_for_retry() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::new(vec![false, true], true);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    start_rt(&mut runtime).expect("Ok");
    // 制造「停不掉」：句柄必须保留，下一次停用仍会去停同一个句柄。
    spawner.with_state(|state| {
        state.stop_error = true;
        state.wait_error = true;
    });
    assert!(runtime.shutdown().is_err(), "停不干净必须报错");
    assert_eq!(spawner.with_state(|s| s.stops), 1);
    spawner.with_state(|state| {
        state.stop_error = false;
        state.wait_error = false;
    });
    runtime.shutdown().expect("第二次应停干净");
    assert_eq!(
        spawner.with_state(|s| s.stops),
        2,
        "句柄保留 → 第二次仍会去停同一个句柄"
    );
}

#[test]
fn apply_settings_is_never_called() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::new(vec![false, true, true], true);
    let applied = Arc::new(AtomicUsize::new(0));
    let mut runtime = build_runtime_with_services(
        counting_services(&applied),
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    start_rt(&mut runtime).expect("Ok");
    runtime.command("restart", &json!({})).expect("Ok");
    runtime.shutdown().expect("Ok");
    assert_eq!(
        applied.load(Ordering::SeqCst),
        0,
        "本 Mod 不许写 [tts]（含经由 ModServices.apply_settings）"
    );
}

#[test]
fn start_registers_settings_schema() {
    let program = TempProgram::new("cmd");
    let spawner = Arc::new(SpawnerLog::default());
    let probe = ProbeScript::stable(true);
    let mut runtime = build_runtime(
        json!({"program": program_str(&program), "port": free_port()}),
        &spawner,
        &probe,
    );
    let mut registrar = RecordingRegistrar::default();
    runtime.start(&mut registrar).expect("Ok");
    assert_eq!(registrar.specs.len(), 1);
    assert_eq!(registrar.specs[0].mod_id, "cosyvoice3-tts");
    assert_eq!(registrar.specs[0].validate(), Ok(()));
}

#[test]
fn factory_creates_a_disabled_runtime() {
    let mut mod_runtime = COSYVOICE3_FACTORY
        .create(services(), json!({}))
        .expect("缺省配置必须能建实例");
    let state = mod_runtime.state_json().expect("有 state_json");
    assert_eq!(state["status"], json!("disabled"));
    assert_eq!(state["port"], json!(8011));
    assert_eq!(state["voice"], json!("dafeiyu"));
}
