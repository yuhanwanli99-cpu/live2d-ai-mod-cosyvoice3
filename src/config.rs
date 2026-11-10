//! 纯数据：`cosyvoice3-tts` 的设置解析与校验（**不碰进程、不碰 host**）。
//!
//! 拆出来的理由（除体量外）：这些判据是「删掉实现就必须红」的那一半，它们必须是
//! **纯函数**——可以被单测直接喂非法输入，不需要真的 spawn、也不需要有端口。
//!
//! # 五个字段（与 `settings_spec` 一一对应）
//!
//! | key | 类型 | 缺省 | 含义 |
//! | --- | --- | --- | --- |
//! | `program` | String | [`DEFAULT_PROGRAM`] | sidecar 的 Windows 启动器（`.cmd`） |
//! | `port` | Number | 8011 | sidecar 的 HTTP 端口（探活 + 展示） |
//! | `voice` | String | `dafeiyu` | 只展示：与 `[tts].voice` 对齐，Mod **不写** `[tts]` |
//! | `auto_start` | Bool | true | 启用本 Mod 时要不要把 sidecar 拉起来 |
//! | `health_timeout_s` | Number | 60 | spawn 之后等 `GET /healthz` 变 2xx 的上限 |
//!
//! # 三条硬口径
//!
//! 1. **坏配置在 create / apply 就报错**，不折成缺省：用户以为填了 8081 而实际按
//!    8011 跑，是最难查的一类静默偏差。
//! 2. **空 `program` = 未配置**，回落 [`DEFAULT_PROGRAM`]（与模板 Mod
//!    `local-tts` 的「空 program 回落引擎缺省」同口径）。
//! 3. 未知键**忽略不报错**：namespaced 配置里可能有宿主写入的附加键，
//!    本 Mod 只认自己那五个。

use std::path::PathBuf;
use std::time::Duration;

/// Mod 稳定 id（与 [`crate::DESCRIPTOR`] 同源）。
pub const MOD_ID: &str = "cosyvoice3-tts";

/// sidecar 的缺省启动器：**Windows 上的 `.cmd`**。
///
/// 为什么指向 D 盘而不是仓库内的脚本：sidecar（server.py / 权重 / 参考音频）
/// 由另一条线放在 `D:/live2dai/tts-cosyvoice3`，**不进这个仓库**——本 crate 只做
/// 「探活 + 拉起 + 收尾」，不打包服务端。路径按 `D:/` 正斜杠写，Windows 的
/// CreateProcess 接受正斜杠；Linux 开发目标上它当然不存在（那是 `program_missing`，
/// 门禁里由单测手工指定存在/不存在的路径覆盖）。
pub const DEFAULT_PROGRAM: &str = "D:/live2dai/tts-cosyvoice3/start.cmd";

/// sidecar 的缺省 HTTP 端口（**避开 18080**：那是本项目的 Web/API 端口）。
pub const DEFAULT_PORT: u16 = 8011;

/// 缺省音色 id：与用户 `[tts].voice` 对齐的**同名占位**（sidecar 里预缓存的那份）。
///
/// 它只进 `state_json` 供人对照，**不参与合成**：请求体里的 `voice` 由核心链路
/// 按 `[tts]` 填（见 `docs/architecture/tts-is-core.md`）。
pub const DEFAULT_VOICE: &str = "dafeiyu";

/// 缺省：启用即拉起。
pub const DEFAULT_AUTO_START: bool = true;

/// 缺省：spawn 之后最多等 60 秒让 `/healthz` 变 2xx。
///
/// 首次加载 CosyVoice3 权重是分钟级，但 `start.cmd` 应该先把 HTTP 起起来再载模型
/// 的（sidecar 侧负责）；时间不够就调大这个值，而不是把失败当成功。
pub const DEFAULT_HEALTH_TIMEOUT_S: u64 = 60;

/// `port` 的下界（声明用；实际判据是 1..=65535）。
pub const MIN_PORT: u16 = 1;
/// `port` 的上界。
pub const MAX_PORT: u16 = 65535;
/// `health_timeout_s` 的下界。
pub const MIN_HEALTH_TIMEOUT_S: u64 = 1;
/// `health_timeout_s` 的上界（10 分钟；再长就该修 sidecar，而不是等）。
pub const MAX_HEALTH_TIMEOUT_S: u64 = 600;

/// 探活路径（sidecar 契约：`GET http://127.0.0.1:<port>/healthz` → 2xx）。
pub const HEALTH_PATH: &str = "/healthz";

/// **单次**探活的超时（口径：2 秒）。轮询间隔是 1 秒，与它无关。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// 解析后的有效配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosyVoiceConfig {
    /// 启动器路径（`argv[0]` 的来源，见 [`crate::child::entry_argv`]）。
    pub program: String,
    /// sidecar 的 HTTP 端口。
    pub port: u16,
    /// 音色 id（只展示）。
    pub voice: String,
    /// 启用时是否拉起 sidecar。
    pub auto_start: bool,
    /// spawn 之后等 `/healthz` 的上限（秒）。
    pub health_timeout_s: u64,
}

impl CosyVoiceConfig {
    /// 全缺省（等价于配置 `{}` / `null`）。
    pub fn defaults() -> Self {
        Self {
            program: DEFAULT_PROGRAM.to_string(),
            port: DEFAULT_PORT,
            voice: DEFAULT_VOICE.to_string(),
            auto_start: DEFAULT_AUTO_START,
            health_timeout_s: DEFAULT_HEALTH_TIMEOUT_S,
        }
    }

    /// 从 namespaced JSON 解析。
    ///
    /// **全量校验**：任意一个键坏就返回 `Err`（带原文 + 期望类型 + 范围），
    /// 调用方据此把 Mod 标 `Failed` / 回可读错误，**不得**折成缺省继续跑。
    pub fn parse(config: &serde_json::Value) -> Result<Self, String> {
        let mut out = Self::defaults();
        let map = match config {
            serde_json::Value::Null => return Ok(out),
            serde_json::Value::Object(map) => map,
            other => {
                return Err(format!(
                    "配置必须是 JSON 对象（如 {{\"port\":8011}}），实际是 {other}（{}）",
                    json_kind(other)
                ));
            }
        };
        if let Some(value) = map.get("program") {
            out.program = parse_program(value)?;
        }
        if let Some(value) = map.get("port") {
            out.port = parse_port(value)?;
        }
        if let Some(value) = map.get("voice") {
            out.voice = parse_voice(value)?;
        }
        if let Some(value) = map.get("auto_start") {
            out.auto_start = value.as_bool().ok_or_else(|| {
                format!(
                    "auto_start 必须是 true / false，实际是 {value}（{}）",
                    json_kind(value)
                )
            })?;
        }
        if let Some(value) = map.get("health_timeout_s") {
            out.health_timeout_s = parse_health_timeout(value)?;
        }
        Ok(out)
    }

    /// 探活地址（只进日志 / 错误消息，**不写 [tts]**）。
    pub fn health_url(&self) -> String {
        format!("http://127.0.0.1:{}{HEALTH_PATH}", self.port)
    }

    /// sidecar 的 OpenAI 兼容基址（只展示：与用户 `[tts].base_url` 对齐的最后一段）。
    pub fn openai_base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    /// spawn 之后等就绪的上限。
    pub fn health_timeout(&self) -> Duration {
        Duration::from_secs(self.health_timeout_s)
    }

    /// 启动器路径（`program_missing` 判据直接查它）。
    pub fn program_path(&self) -> PathBuf {
        PathBuf::from(self.program.trim())
    }

    /// spawn 的工作目录 = 启动器所在目录（让 `start.cmd` 里的相对路径可用）。
    ///
    /// **不创建目录**：目录不存在是用户配置问题，由 `program_missing` 先报出来。
    pub fn workdir(&self) -> Option<PathBuf> {
        let program = self.program_path();
        let parent = program.parent()?;
        if parent.as_os_str().is_empty() {
            None
        } else {
            Some(parent.to_path_buf())
        }
    }
}

/// `program`：字符串；空串 = 未配置 → 缺省启动器。
fn parse_program(value: &serde_json::Value) -> Result<String, String> {
    let raw = value.as_str().ok_or_else(|| {
        format!(
            "program 必须是字符串路径，实际是 {value}（{}）",
            json_kind(value)
        )
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(DEFAULT_PROGRAM.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// `port`：整数 1..=65535（浮点 / 字符串 / 越界都报原文）。
fn parse_port(value: &serde_json::Value) -> Result<u16, String> {
    let number = value.as_i64().ok_or_else(|| {
        format!(
            "port 必须是整数（1..={MAX_PORT}），实际是 {value}（{}）",
            json_kind(value)
        )
    })?;
    if !(i64::from(MIN_PORT)..=i64::from(MAX_PORT)).contains(&number) {
        return Err(format!(
            "port 超出范围 {MIN_PORT}..={MAX_PORT}：{number}（18080 是本项目的 Web 端口，别占它）"
        ));
    }
    Ok(number as u16)
}

/// `voice`：非空字符串（只展示）。
fn parse_voice(value: &serde_json::Value) -> Result<String, String> {
    let raw = value.as_str().ok_or_else(|| {
        format!(
            "voice 必须是字符串（如 dafeiyu），实际是 {value}（{}）",
            json_kind(value)
        )
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("voice 为空：请填 sidecar 里预缓存的音色 id（缺省 dafeiyu）".to_string());
    }
    Ok(trimmed.to_string())
}

/// `health_timeout_s`：整数 1..=600。
fn parse_health_timeout(value: &serde_json::Value) -> Result<u64, String> {
    let number = value.as_i64().ok_or_else(|| {
        format!(
            "health_timeout_s 必须是整数秒（{MIN_HEALTH_TIMEOUT_S}..={MAX_HEALTH_TIMEOUT_S}），实际是 {value}（{}）",
            json_kind(value)
        )
    })?;
    if !(MIN_HEALTH_TIMEOUT_S as i64..=MAX_HEALTH_TIMEOUT_S as i64).contains(&number) {
        return Err(format!(
            "health_timeout_s 超出范围 {MIN_HEALTH_TIMEOUT_S}..={MAX_HEALTH_TIMEOUT_S}：{number}"
        ));
    }
    Ok(number as u64)
}

/// JSON 值的类型名（错误消息用，**不**打印整份配置）。
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
