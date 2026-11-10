//! 运行时状态机：**先探活、再拉起、按秒轮询**，停用只收自己拉起的进程。
//!
//! # `enable`（[`ModRuntime::start`] → [`CosyVoice3Runtime::ensure_ready`]）
//!
//! 1. `starting`，先探一次活（`GET http://127.0.0.1:<port>/healthz`，单次 2 秒）：
//!    **已经是 2xx 就 ready**——不管它是上一次留下的服务、还是用户自己起的
//!    （`pid = null`、`owned = false`，停用时**不会**去杀它）；
//! 2. 没就绪 → 判 `program_missing`（启动器不存在）/ `port_in_use`（端口有人应答
//!    但不是 `/healthz`，**不硬起第二个**）；
//! 3. spawn 启动器（Windows `.cmd` 经 `cmd.exe /C`，见 [`crate::child`]），
//!    然后**每 1 秒**探一次，直到 `health_timeout_s`：
//!    - 2xx → ready（`pid` = 本 Mod 拉起的那个）；
//!    - 子进程退出 → `spawn_failed`（Err 里带启动器 stderr 的尾巴）；
//!    - 超时 → 先把自己拉起的那个收掉，再报 `health_timeout`（不留给用户一个
//!      占着端口却永远不就绪的半成品）。
//!
//! # `disable` / `shutdown` / `drop`
//!
//! **只停自己拉起的子进程**（`owned`）。外部已就绪的 sidecar 一律不碰——那可能
//! 是用户手动起的服务，甚至是另一条线正在调试的实例。
//!
//! 停止本身按「先整棵树、再单进程」：Windows `taskkill /T /F`、Unix 进程组
//! `kill -s KILL -- -<pgid>`（见 [`crate::child::real_spawn`]）。
//!
//! # `apply_config`（宿主侧叫 apply_settings）
//!
//! **原子**：先全量解析校验（坏配置 → `ModError::InvalidSettings`，可读原因，
//! 运行态**一个字段都不改**），再一次性替换；运行中（有子进程 / 已 ready）则
//! 拒绝并要求先停用——因为改了 `port` 却不重启，`state_json` 就会写着新端口而
//! 进程还在旧端口上服务，那是**界面在说谎**。
//!
//! # 不订阅任何事件
//!
//! 本 Mod 的事件面是**空**的：它不看 turn / 句子 / 动作，只负责「sidecar 活着」。
//! `on_event` 用缺省实现（`Ok`），`start` 里**不调用** `registrar.subscribe`。

use std::time::{Duration, Instant};

use live2d_ai_mod_system::{ModError, ModRegistrar, ModRuntime, ModServices};

use crate::child::{ChildProbe, ChildProcess, SpawnRequest, Spawner, entry_argv, real_spawn};
use crate::config::{CosyVoiceConfig, MOD_ID, PROBE_TIMEOUT};
use crate::health::{Prober, http_health, port_accepts};
use crate::state::{LastError, RuntimeState};

/// 生产轮询间隔：**每 1 秒**探一次 `/healthz`（口径）。
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// CosyVoice3 sidecar 的运行时实例。
pub struct CosyVoice3Runtime {
    services: ModServices,
    config: CosyVoiceConfig,
    /// 当前子进程（`None` = 没有；**至多一个**）。
    child: Option<Box<dyn ChildProcess>>,
    spawner: Spawner,
    prober: Prober,
    state: RuntimeState,
    poll_interval: Duration,
}

impl CosyVoice3Runtime {
    /// 生产构造：真实 `Command` + 真实 HTTP 探活。
    pub fn new(services: ModServices, config: CosyVoiceConfig) -> Self {
        Self::with_hooks(
            services,
            config,
            Box::new(real_spawn),
            Box::new(http_health),
        )
    }

    /// 注入 spawner / prober 的构造（**单测路径**：不 exec 真进程、不真连端口）。
    pub fn with_hooks(
        services: ModServices,
        config: CosyVoiceConfig,
        spawner: Spawner,
        prober: Prober,
    ) -> Self {
        Self {
            services,
            config,
            child: None,
            spawner,
            prober,
            state: RuntimeState::default(),
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// 覆盖轮询间隔（**单测用**：把 1 秒压到 10 毫秒，超时用例才跑得快）。
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// 当前生效的配置（**只读**）。
    pub fn config(&self) -> &CosyVoiceConfig {
        &self.config
    }

    /// 当前运行态（**只读**；`state_json` 的源）。
    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    /// 原子应用一份新配置（宿主 `apply_settings` 那条路径的 Mod 侧实现）。
    ///
    /// 顺序是刻意的：**先解析**（坏配置在这里就回可读错误，`self` 一个字段都不改），
    /// **再看运行态**（运行中拒绝原地改），最后一次性替换。
    pub fn apply_config(&mut self, config: &serde_json::Value) -> Result<(), ModError> {
        let next = CosyVoiceConfig::parse(config).map_err(|message| ModError::InvalidSettings {
            mod_id: MOD_ID.to_string(),
            message,
        })?;
        if self.child.is_some() || self.state.running() {
            return Err(ModError::InvalidSettings {
                mod_id: MOD_ID.to_string(),
                message: "运行中不能原地应用配置：先停用本 Mod（宿主会 shutdown 后用新配置重建实例），                          否则 state_json 会显示新端口而进程还在旧端口上服务"
                    .to_string(),
            });
        }
        self.config = next;
        Ok(())
    }

    /// [`Self::apply_config`] 的别名（宿主/文档用的是 `apply_settings` 这个词）。
    ///
    /// **注意**与 [`ModServices::apply_settings`] 不是一回事：那个写的是主配置
    /// `live2d-ai.toml`（`[tts]` 等），本 Mod **从不调用**它。
    pub fn apply_settings(&mut self, config: &serde_json::Value) -> Result<(), ModError> {
        self.apply_config(config)
    }

    /// 保证 sidecar 就绪：先探活，未就绪则拉起并按秒轮询。
    pub fn ensure_ready(&mut self) -> Result<(), ModError> {
        // 已经握着**自己拉起的**子进程：健康就沿用，不健康就先收掉（不并存两个）。
        if self.child.is_some() {
            if self.probe() {
                let pid = self.state.pid;
                self.state.mark_ready(pid);
                return Ok(());
            }
            self.stop_owned()?;
        }
        self.state.begin_start();
        // 1) 先探活：可能是上一次留下的服务，也可能是用户自己起的。
        if self.probe() {
            self.state.mark_ready(None);
            return Ok(());
        }
        // 2) 前置判据（顺序按「用户下一步该做什么」排：先补文件，再看端口）。
        if !self.config.program_path().is_file() {
            return Err(self.fail(LastError::ProgramMissing));
        }
        if port_accepts(self.config.port, PROBE_TIMEOUT) {
            return Err(self.fail(LastError::PortInUse));
        }
        // 3) 拉起。
        let argv = entry_argv(&self.config.program, cfg!(windows))
            .map_err(|reason| self.fail_because(LastError::SpawnFailed, &reason))?;
        let request = SpawnRequest {
            argv,
            workdir: self.config.workdir(),
        };
        let child = match (self.spawner)(&request) {
            Ok(child) => child,
            Err(error) => {
                return Err(
                    self.fail_because(LastError::SpawnFailed, &format!("拉起进程失败：{error}"))
                );
            }
        };
        let pid = child.pid();
        self.services.logger.info(&format!(
            "{MOD_ID} 已拉起 pid={pid:?}（{}）",
            self.config.health_url()
        ));
        self.child = Some(child);
        self.state.pid = pid;
        self.state.owned = true;
        // 4) 每 poll_interval 轮询，直到 health_timeout_s。
        let deadline = Instant::now() + self.config.health_timeout();
        loop {
            if self.probe() {
                self.state.mark_ready(pid);
                return Ok(());
            }
            if let Some(exit) = self.child_exited() {
                let reason = self.stderr_reason();
                self.child = None;
                self.state.pid = None;
                self.state.owned = false;
                return Err(self.fail_because(
                    LastError::SpawnFailed,
                    &format!("进程在就绪前退出（{exit}）{reason}"),
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let detail = format!(
                    "{} 秒内 {} 未返回 2xx",
                    self.config.health_timeout_s,
                    self.config.health_url()
                );
                // 不留半成品：超时就收掉刚拉起的那个（只收自己拉起的）。
                let stop = self.stop_owned();
                let tail = match stop {
                    Ok(()) => String::new(),
                    Err(error) => format!("；收掉该进程也没成功：{error}"),
                };
                return Err(self.fail_because(LastError::HealthTimeout, &format!("{detail}{tail}")));
            }
            std::thread::sleep(self.poll_interval.min(remaining));
        }
    }

    /// 停掉**自己拉起的**子进程；没有自己的子进程时只把状态落成 `disabled`。
    ///
    /// 返回 `Err` 时**句柄放回**（下一次 `shutdown` 仍会去停同一个句柄）——与
    /// `local-tts` 的 S-000 修正同一条纪律：停不干净不能装作停干净了。
    pub fn stop_owned(&mut self) -> Result<(), ModError> {
        let Some(mut child) = self.child.take() else {
            self.state.mark_disabled();
            return Ok(());
        };
        self.state.mark_disabled();
        let stopped = child.stop();
        let waited = child.wait();
        match (stopped, waited) {
            (Ok(()), Ok(())) => Ok(()),
            // 停的时候进程已经没了（ESRCH / taskkill 找不到）→ 收尸成功就算停干净。
            (Err(_), Ok(())) => Ok(()),
            (Ok(()), Err(error)) => {
                self.child = Some(child);
                Err(ModError::Other(format!(
                    "{MOD_ID} 停止后等待进程退出失败：{error}（句柄已保留，下次停用会重试）"
                )))
            }
            (Err(stop_error), Err(wait_error)) => {
                self.child = Some(child);
                Err(ModError::Other(format!(
                    "{MOD_ID} 停止子进程失败：{stop_error}；等待退出也失败：{wait_error}（句柄已保留）"
                )))
            }
        }
    }

    /// 一次探活（生产 = 真 HTTP，单测 = 假实现）。
    fn probe(&self) -> bool {
        (self.prober)(self.config.port, PROBE_TIMEOUT)
    }

    /// 非阻塞看子进程是否已经退出（`Some(说明)` = 退出了 / 观察不了）。
    fn child_exited(&mut self) -> Option<String> {
        let child = self.child.as_mut()?;
        match child.try_wait() {
            Ok(ChildProbe::Running) => None,
            Ok(ChildProbe::Exited { code }) => Some(match code {
                Some(0) => "退出码 0".to_string(),
                Some(code) => format!("退出码 {code}"),
                None => "被信号终止".to_string(),
            }),
            Err(error) => Some(format!("无法观察（{error}）")),
        }
    }

    /// 子进程 stderr 的尾巴 → 附在 Err 后面的一句（没有就返回空串，**不编**）。
    fn stderr_reason(&self) -> String {
        let Some(child) = self.child.as_ref() else {
            return String::new();
        };
        match child.stderr_tail() {
            Some(text) if !text.trim().is_empty() => format!("；启动器输出：{}", text.trim()),
            _ => String::new(),
        }
    }

    /// 失败收口：状态 + 原因码，返回**可读**的 `Err`（宿主据此标 `Failed`）。
    fn fail(&mut self, error: LastError) -> ModError {
        self.state.fail(error);
        ModError::Other(format!(
            "{MOD_ID} 启动失败（{}）：{}",
            error.as_str(),
            error.advice()
        ))
    }

    /// 同上，多带一句具体原因（spawn 报错 / 启动器 stderr / 收尾失败）。
    fn fail_because(&mut self, error: LastError, cause: &str) -> ModError {
        self.state.fail(error);
        ModError::Other(format!(
            "{MOD_ID} 启动失败（{}）：{}；原因：{cause}",
            error.as_str(),
            error.advice()
        ))
    }
}

impl ModRuntime for CosyVoice3Runtime {
    fn start(&mut self, registrar: &mut dyn ModRegistrar) -> Result<(), ModError> {
        registrar.register_settings(crate::spec::settings_spec())?;
        if !self.config.auto_start {
            // 不拉起：只探一次活，把「外部是不是已经好了」如实报出来。
            if self.probe() {
                let pid = self.child.as_ref().and_then(|child| child.pid());
                self.state.mark_ready(pid);
            } else {
                self.state.mark_disabled();
            }
            return Ok(());
        }
        self.ensure_ready()
    }

    fn state_json(&mut self) -> Option<serde_json::Value> {
        Some(self.state.snapshot(&self.config))
    }

    fn command(
        &mut self,
        command: &str,
        _args: &serde_json::Value,
    ) -> Result<serde_json::Value, ModError> {
        match command {
            "probe" => {
                let healthy = self.probe();
                if healthy {
                    let pid = self
                        .child
                        .as_ref()
                        .and_then(|child| child.pid())
                        .or(self.state.pid);
                    self.state.mark_ready(pid);
                } else if self.state.running() {
                    // 曾经 ready 现在不通：如实标失败，别继续显示 ready。
                    self.state.fail(LastError::HealthTimeout);
                }
                Ok(serde_json::json!({
                    "healthy": healthy,
                    "status": self.state.status.as_str(),
                    "last_error": self.state.last_error.map(|e| e.as_str()),
                }))
            }
            "start" => {
                self.ensure_ready()?;
                Ok(serde_json::json!({ "status": self.state.status.as_str() }))
            }
            "stop" => {
                self.stop_owned()?;
                Ok(serde_json::json!({ "status": self.state.status.as_str() }))
            }
            "restart" => {
                self.stop_owned()?;
                self.ensure_ready()?;
                Ok(serde_json::json!({ "status": self.state.status.as_str() }))
            }
            other => Err(ModError::UnsupportedCommand {
                command: other.to_string(),
            }),
        }
    }

    fn shutdown(&mut self) -> Result<(), ModError> {
        self.stop_owned()
    }
}

impl Drop for CosyVoice3Runtime {
    fn drop(&mut self) {
        // 只收自己拉起的那个；失败不 panic（Drop 里没有地方返回 Result）。
        if self.child.is_some()
            && let Err(error) = self.stop_owned()
        {
            self.services
                .logger
                .warn(&format!("{MOD_ID} drop 时停止子进程失败：{error}"));
        }
    }
}
