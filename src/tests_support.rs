//! 单测共享脚手架：假子进程 / 假 spawner / 假探活 / 注册器 / 临时启动器 / 空闲端口。
//!
//! 拆出来的理由与模板 `local-tts` 相同：`crates/*/src` 下的 `.rs` 按**文件总行数**
//! 计，测试按场景拆成三个文件（配置 / 探活 / 生命周期），共享件放这里。
//!
//! 全程**不 exec 真进程**（spawner 可注入），探活也**不真起 sidecar**
//! （`ProbeScript` 按脚本回答；真 HTTP 那一层由 `tests_health.rs` 用本地假服务测）。

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use live2d_ai_mod_system::*;

use crate::*;

/// 假子进程的内部状态（测试直接改它来制造「已退出 / 停不掉 / 探测失败」）。
#[derive(Debug)]
pub struct FakeState {
    /// 假 pid。
    pub pid: u32,
    /// 是否在跑。
    pub running: bool,
    /// 退出码（`None` = 被信号终止）。
    pub exit_code: Option<i32>,
    /// `try_wait` 直接报错。
    pub probe_error: bool,
    /// `stop` 报错。
    pub stop_error: bool,
    /// `wait` 报错。
    pub wait_error: bool,
    /// `stop` 被调用几次。
    pub stops: usize,
    /// `wait` 被调用几次。
    pub waits: usize,
    /// 假句柄的 stderr 尾巴。
    pub stderr: String,
}

impl FakeState {
    fn running(pid: u32) -> Self {
        Self {
            pid,
            running: true,
            exit_code: None,
            probe_error: false,
            stop_error: false,
            wait_error: false,
            stops: 0,
            waits: 0,
            stderr: String::new(),
        }
    }
}

/// 假句柄：`stop()` 成功即置「已退出（码 0）」，并计数。
struct FakeChild {
    state: Arc<Mutex<FakeState>>,
}

impl ChildProcess for FakeChild {
    fn pid(&self) -> Option<u32> {
        Some(self.state.lock().unwrap().pid)
    }

    fn try_wait(&mut self) -> io::Result<ChildProbe> {
        let state = self.state.lock().unwrap();
        if state.probe_error {
            return Err(io::Error::other("probe 失败"));
        }
        if state.running {
            Ok(ChildProbe::Running)
        } else {
            Ok(ChildProbe::Exited {
                code: state.exit_code,
            })
        }
    }

    fn stop(&mut self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.stops += 1;
        if state.stop_error {
            return Err(io::Error::other("kill 失败"));
        }
        state.running = false;
        if state.exit_code.is_none() {
            state.exit_code = Some(0);
        }
        Ok(())
    }

    fn wait(&mut self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.waits += 1;
        if state.wait_error {
            return Err(io::Error::other("wait 失败"));
        }
        Ok(())
    }

    fn stderr_tail(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        if state.stderr.trim().is_empty() {
            None
        } else {
            Some(state.stderr.clone())
        }
    }
}

/// 记录 spawner 调用：每次产出**新的**假句柄，并留下它的共享状态。
#[derive(Default)]
pub struct SpawnerLog {
    requests: Mutex<Vec<SpawnRequest>>,
    children: Mutex<Vec<Arc<Mutex<FakeState>>>>,
    spawn_error: Mutex<Option<String>>,
    spawn_exit: Mutex<Option<Option<i32>>>,
    spawn_stderr: Mutex<Option<String>>,
}

impl SpawnerLog {
    /// 可注入的 spawner（不 exec 真进程）。
    pub fn spawner(self: &Arc<Self>) -> Spawner {
        let log = Arc::clone(self);
        Box::new(move |request: &SpawnRequest| {
            if let Some(message) = log.spawn_error.lock().unwrap().clone() {
                return Err(io::Error::other(message));
            }
            let mut fresh = FakeState::running(4242);
            if let Some(code) = *log.spawn_exit.lock().unwrap() {
                fresh.running = false;
                fresh.exit_code = code;
            }
            if let Some(text) = log.spawn_stderr.lock().unwrap().clone() {
                fresh.stderr = text;
            }
            let state = Arc::new(Mutex::new(fresh));
            log.children.lock().unwrap().push(Arc::clone(&state));
            log.requests.lock().unwrap().push(request.clone());
            Ok(Box::new(FakeChild { state }) as Box<dyn ChildProcess>)
        })
    }

    /// spawner 被调用了几次。
    pub fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// 最近一次 spawn 的请求（argv / workdir）。
    pub fn last_request(&self) -> SpawnRequest {
        self.requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("没有 spawn 过")
    }

    /// 对最近一次 spawn 出来的假子进程状态做一次读/改。
    pub fn with_state<T>(&self, f: impl FnOnce(&mut FakeState) -> T) -> T {
        let state = Arc::clone(self.children.lock().unwrap().last().expect("没有 spawn 过"));
        let mut guard = state.lock().unwrap();
        f(&mut guard)
    }

    /// **全部**假子进程被 stop 的次数合计（restart 会换新句柄，单看最后一个会漏）。
    pub fn total_stops(&self) -> usize {
        self.children
            .lock()
            .unwrap()
            .iter()
            .map(|state| state.lock().unwrap().stops)
            .sum()
    }

    /// 让**之后** spawn 出来的假句柄一出生就已退出（退出码 + 启动器输出）。
    pub fn child_exits_on_spawn(&self, code: Option<i32>, stderr: &str) {
        *self.spawn_exit.lock().unwrap() = Some(code);
        *self.spawn_stderr.lock().unwrap() = Some(stderr.to_string());
    }

    /// 让 spawner 直接报错（模拟「cmd.exe 拉不起来」）。
    pub fn spawn_fails(&self, message: &str) {
        *self.spawn_error.lock().unwrap() = Some(message.to_string());
    }
}

/// 假探活：按脚本给答案（用完 → `fallback`），并计数。
pub struct ProbeScript {
    answers: Mutex<Vec<bool>>,
    fallback: bool,
    calls: AtomicUsize,
}

impl ProbeScript {
    /// `answers` 依次回答，用完之后一律回 `fallback`。
    pub fn new(answers: Vec<bool>, fallback: bool) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers),
            fallback,
            calls: AtomicUsize::new(0),
        })
    }

    /// 一直回同一个答案。
    pub fn stable(healthy: bool) -> Arc<Self> {
        Self::new(Vec::new(), healthy)
    }

    /// 可注入的探活函数。
    pub fn prober(self: &Arc<Self>) -> Prober {
        let script = Arc::clone(self);
        Box::new(move |_port: u16, _timeout: Duration| {
            script.calls.fetch_add(1, Ordering::SeqCst);
            let mut answers = script.answers.lock().unwrap();
            if answers.is_empty() {
                script.fallback
            } else {
                answers.remove(0)
            }
        })
    }

    /// 探活被调用了几次。
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

/// `apply_settings` 计数间谍（口径：本 Mod 的调用次数恒为 0）。
pub fn counting_services(applied: &Arc<AtomicUsize>) -> ModServices {
    let counter = Arc::clone(applied);
    ModServices::new(
        ModActionSender::new(|_| false),
        SaySender::new(|_| true),
        ModEventSender::new(|_, _| true),
        ModLogger::new(|_, _| {}),
    )
    .with_apply_settings(ModSettingsApplier::new(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        true
    }))
}

/// 普通 services（日志吞掉）。
pub fn services() -> ModServices {
    counting_services(&Arc::new(AtomicUsize::new(0)))
}

/// 记录 `register_settings` 的测试注册器。
#[derive(Default)]
pub struct RecordingRegistrar {
    /// 注册过的 schema。
    pub specs: Vec<ModSettingsSpec>,
}

impl ModRegistrar for RecordingRegistrar {
    fn register_settings(&mut self, spec: ModSettingsSpec) -> Result<(), ModError> {
        self.specs.push(spec);
        Ok(())
    }

    fn subscribe(&mut self, _: ModEventTopic) -> Result<SubscriptionId, ModError> {
        Ok(SubscriptionId(1))
    }

    fn unsubscribe(&mut self, _: SubscriptionId) -> Result<(), ModError> {
        Ok(())
    }
}

/// 造一个注入了假 spawner / 假探活的 runtime（轮询压到 10 毫秒）。
pub fn build_runtime(
    config: serde_json::Value,
    spawner: &Arc<SpawnerLog>,
    probe: &Arc<ProbeScript>,
) -> CosyVoice3Runtime {
    build_runtime_with_services(services(), config, spawner, probe)
}

/// 同 [`runtime`]，但用调用方给的 services（计数间谍用）。
pub fn build_runtime_with_services(
    services: ModServices,
    config: serde_json::Value,
    spawner: &Arc<SpawnerLog>,
    probe: &Arc<ProbeScript>,
) -> CosyVoice3Runtime {
    let config = CosyVoiceConfig::parse(&config).expect("配置应可解析");
    CosyVoice3Runtime::with_hooks(services, config, spawner.spawner(), probe.prober())
        .with_poll_interval(Duration::from_millis(10))
}

/// 走一次 `ModRuntime::start`（用一次性注册器）。
pub fn start_rt(runtime: &mut CosyVoice3Runtime) -> Result<(), ModError> {
    let mut registrar = RecordingRegistrar::default();
    runtime.start(&mut registrar)
}

/// 取一次 `state_json` 并断言它是对象。
pub fn snapshot(runtime: &mut CosyVoice3Runtime) -> serde_json::Value {
    runtime.state_json().expect("本 Mod 必须提供 state_json")
}

/// 临时启动器文件（`program` 存在的那个分支用）。
pub struct TempProgram {
    dir: PathBuf,
    /// 启动器路径。
    pub path: PathBuf,
}

static TEMP_ID: AtomicUsize = AtomicUsize::new(0);

impl TempProgram {
    /// 在系统临时目录里造一个 `start.<ext>` 并返回它。
    pub fn new(ext: &str) -> Self {
        let id = TEMP_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "live2d-ai-mod-cosyvoice3-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join(format!("start.{ext}"));
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("写临时启动器");
        Self { dir, path }
    }

    /// 一个**一定不存在**的启动器路径。
    pub fn missing() -> PathBuf {
        let id = TEMP_ID.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "live2d-ai-mod-cosyvoice3-missing-{}-{id}/start.cmd",
            std::process::id()
        ))
    }
}

impl Drop for TempProgram {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 一个当前空闲的端口（bind 一次拿号后立刻释放）。
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind 空闲端口");
    listener.local_addr().expect("本机地址").port()
}

/// 一个**被占着**的端口（listener 由调用方持有到测试结束）。
pub fn occupied_port() -> (u16, std::net::TcpListener) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind 占用端口");
    let port = listener.local_addr().expect("本机地址").port();
    (port, listener)
}
