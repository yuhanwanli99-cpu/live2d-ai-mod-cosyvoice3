//! 运行态与可观测面：`state_json` 的**固定形状**与失败码。
//!
//! # `state_json`（宿主 `GET /api/v1/mods/cosyvoice3-tts/state` 直接透传）
//!
//! ```json
//! {"status":"ready","running":true,"pid":4242,"port":8011,"voice":"dafeiyu","last_error":null}
//! ```
//!
//! **只有这六个键**（多一个都算改契约，单测按 key 集合断言）：
//!
//! | key | 类型 | 含义 |
//! | --- | --- | --- |
//! | `status` | string | `disabled` / `starting` / `ready` / `failed` |
//! | `running` | bool | `status == "ready"`（**已确认**在服务，不是「spawn 过」） |
//! | `pid` | number \| null | **本 Mod 拉起的**子进程 pid；外部已就绪的 sidecar → `null` |
//! | `port` | number | 当前配置的端口（探活与展示用） |
//! | `voice` | string | 当前配置的音色 id（**只展示**，不参与合成） |
//! | `last_error` | string \| null | 最近一次失败的原因码；成功一次即清空 |
//!
//! # 四个失败原因码（文档化字符串，前端/排障按它归类）
//!
//! | 码 | 什么时候 | 用户下一步 |
//! | --- | --- | --- |
//! | [`ERR_PROGRAM_MISSING`] `program_missing` | `program` 指向的启动器不存在 | 把 `start.cmd` 放到 `D:/live2dai/tts-cosyvoice3/`，或改 `program` |
//! | [`ERR_PORT_IN_USE`] `port_in_use` | 端口 TCP 有人应答，但 `/healthz` 不是 2xx | 那个进程不是 CosyVoice3：换端口或关掉它（本 Mod 不会硬起第二个） |
//! | [`ERR_SPAWN_FAILED`] `spawn_failed` | 拉不起来 / 起来后**就绪前**退出 | 看 `stderr`（python 没起、显存不够、端口被自己占…） |
//! | [`ERR_HEALTH_TIMEOUT`] `health_timeout` | `health_timeout_s` 内 `/healthz` 一直不是 2xx | sidecar 载模型慢就调大超时；再不行看 sidecar 日志 |
//!
//! # 与宿主 `ModStatus` 的对应（「状态语义与现有 Mod 一致」）
//!
//! | 本 Mod | 宿主 | 说明 |
//! | --- | --- | --- |
//! | [`Status::Disabled`] | `ModStatus::Disabled` | 未启用 / 已停用（`auto_start=false` 且未探到服务时也落这里） |
//! | [`Status::Starting`] | `ModStatus::Starting` | `start` / `restart` 正在探活 + 拉起 |
//! | [`Status::Ready`] | `ModStatus::Running` | `/healthz` 已确认 2xx |
//! | [`Status::Failed`] | `ModStatus::Failed { message }` | `start` 返回 Err，message 里带原因码 |

use live2d_ai_mod_system::ModStatus;

use crate::config::CosyVoiceConfig;

/// `last_error` = 启动器不存在。
pub const ERR_PROGRAM_MISSING: &str = "program_missing";
/// `last_error` = 端口被别的进程占着（不是我们的 sidecar）。
pub const ERR_PORT_IN_USE: &str = "port_in_use";
/// `last_error` = 拉不起来 / 就绪前退出。
pub const ERR_SPAWN_FAILED: &str = "spawn_failed";
/// `last_error` = 等 `/healthz` 超时。
pub const ERR_HEALTH_TIMEOUT: &str = "health_timeout";

/// 本 Mod 的运行态（`state_json.status` 的取值域）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// 未启用 / 已停用。
    #[default]
    Disabled,
    /// 正在探活 + 拉起。
    Starting,
    /// `/healthz` 已确认 2xx。
    Ready,
    /// 失败（原因见 `last_error`）。
    Failed,
}

impl Status {
    /// 稳定字符串 id（进 `state_json`，前端/日志按它判断）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Status::Disabled => "disabled",
            Status::Starting => "starting",
            Status::Ready => "ready",
            Status::Failed => "failed",
        }
    }
}

/// 失败原因（`state_json.last_error` 的取值域）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastError {
    /// 启动器不存在。
    ProgramMissing,
    /// 端口被别的进程占着。
    PortInUse,
    /// 拉不起来 / 就绪前退出。
    SpawnFailed,
    /// 等 `/healthz` 超时。
    HealthTimeout,
}

impl LastError {
    /// 文档化的稳定字符串（**这四条是本 Mod 的公开契约**，别改字面量）。
    pub const fn as_str(self) -> &'static str {
        match self {
            LastError::ProgramMissing => ERR_PROGRAM_MISSING,
            LastError::PortInUse => ERR_PORT_IN_USE,
            LastError::SpawnFailed => ERR_SPAWN_FAILED,
            LastError::HealthTimeout => ERR_HEALTH_TIMEOUT,
        }
    }

    /// 给人看的一句处置建议（进 Err 消息；不替代 `state_json` 里的码）。
    pub const fn advice(self) -> &'static str {
        match self {
            LastError::ProgramMissing => {
                "sidecar 启动器不存在：把 start.cmd 放到 D:/live2dai/tts-cosyvoice3/，或把 program 改成实际路径"
            }
            LastError::PortInUse => {
                "端口已被别的进程占用（它应答 TCP 但不是 /healthz）：换端口或先关掉那个进程"
            }
            LastError::SpawnFailed => {
                "启动器拉不起来或起来后就绪前退出：看它的 stderr（python 环境、显卡驱动/显存、端口自查）"
            }
            LastError::HealthTimeout => {
                "等 /healthz 超时：sidecar 载模型慢就调大 health_timeout_s，再不行看 sidecar 自己的日志"
            }
        }
    }
}

/// 运行态快照（**只读语义**：不在这里 spawn / 探活 / 写盘）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeState {
    /// 当前状态。
    pub status: Status,
    /// 本 Mod 拉起的子进程 pid（没有 → `None`）。
    pub pid: Option<u32>,
    /// 最近一次失败原因（成功一次即清空）。
    pub last_error: Option<LastError>,
    /// 在场子进程是不是**本 Mod 拉起的**（disable 只停这种）。
    pub owned: bool,
}

impl RuntimeState {
    /// `running`：已经**确认**在服务（不是「plausible」）。
    pub fn running(&self) -> bool {
        self.status == Status::Ready
    }

    /// 映射到宿主的 [`ModStatus`]（语义与既有 Mod 一致；见模块头注的表）。
    pub fn host_status(&self) -> ModStatus {
        match self.status {
            Status::Disabled => ModStatus::Disabled,
            Status::Starting => ModStatus::Starting,
            Status::Ready => ModStatus::Running,
            Status::Failed => ModStatus::Failed {
                message: self
                    .last_error
                    .map_or_else(String::new, |e| e.as_str().to_string()),
            },
        }
    }

    /// `state_json` 的**唯一**构造点（六个键，顺序无关、集合固定）。
    pub fn snapshot(&self, config: &CosyVoiceConfig) -> serde_json::Value {
        // 显式 match 而不是 `map_or(_, Value::from)`：`Value::from` 是重载的，
        // 写成闭包会被 clippy 判 `redundant_closure`。
        let pid = match self.pid {
            Some(pid) => serde_json::Value::from(pid),
            None => serde_json::Value::Null,
        };
        let last_error = match self.last_error {
            Some(error) => serde_json::Value::from(error.as_str()),
            None => serde_json::Value::Null,
        };
        serde_json::json!({
            "status": self.status.as_str(),
            "running": self.running(),
            "pid": pid,
            "port": config.port,
            "voice": config.voice.as_str(),
            "last_error": last_error,
        })
    }

    /// 开始一次尝试：`starting`、清掉上一次的失败与 pid。
    pub fn begin_start(&mut self) {
        self.status = Status::Starting;
        self.pid = None;
        self.owned = false;
        self.last_error = None;
    }

    /// 就绪：`ready` + 记录确实是本 Mod 拉起的那个 pid（外部已就绪 → `None`）。
    pub fn mark_ready(&mut self, owned_pid: Option<u32>) {
        self.status = Status::Ready;
        self.pid = owned_pid;
        self.owned = owned_pid.is_some();
        self.last_error = None;
    }

    /// 失败：`failed` + 原因码（pid 归 null：没有在服务的子进程）。
    pub fn fail(&mut self, error: LastError) {
        self.status = Status::Failed;
        self.pid = None;
        self.owned = false;
        self.last_error = Some(error);
    }

    /// 停用：`disabled`（`last_error` **保留**——它是「最近一次为什么失败」的记录）。
    pub fn mark_disabled(&mut self) {
        self.status = Status::Disabled;
        self.pid = None;
        self.owned = false;
    }
}
