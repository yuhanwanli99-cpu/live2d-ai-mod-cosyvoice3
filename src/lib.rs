//! live2d-ai-mod-cosyvoice3：**守护 CosyVoice3 本地 TTS sidecar** 的 Mod。
//!
//! | 项 | 值 |
//! | --- | --- |
//! | crate | `live2d-ai-mod-cosyvoice3` |
//! | 稳定 id | `cosyvoice3-tts` |
//! | 工厂 | [`COSYVOICE3_FACTORY`] |
//! | `api_version` | 1（[`live2d_ai_mod_system::MOD_API_VERSION`]） |
//! | 缺省状态 | **停用**（挂回后也不进缺省 manifest；见 README「挂载」） |
//!
//! # 它是什么
//!
//! Windows 上 CosyVoice3 的 sidecar（FastAPI + uvicorn，OpenAI 兼容
//! `POST /v1/audio/speech`）是**本机 Windows 原生进程**：`start.cmd` 直接起
//! `python.exe`（CUDA 走本机驱动），监听 `127.0.0.1:8011`，**不需要 WSL2**。
//! 它不随桌面程序启动，需要有人**探活 → 拉起 → 收尾**。本 Mod 就是那个人：
//!
//! - `enable`：先探活（2 秒），已就绪即 `ready`；没就绪就 spawn
//!   `D:/live2dai/tts-cosyvoice3/start.cmd`（`cmd.exe /C`），此后**每 1 秒**再探，
//!   直到 `health_timeout_s`；
//! - `disable` / `shutdown` / `drop`：**只停自己拉起的**那棵进程树（Windows
//!   `taskkill /T /F`，Unix 进程组），外部已就绪的服务一概不碰；
//! - `state_json`：`status / running / pid / port / voice / last_error` 六个键，
//!   失败原因用文档化的四个码（`program_missing` / `port_in_use` /
//!   `spawn_failed` / `health_timeout`，见 [`state`]）。
//!
//! # 它不是什么（边界，逐条与 `docs/architecture/tts-is-core.md` 对齐）
//!
//! - **不做推理、不带权重、不带 sidecar 代码**：`server.py` 在
//!   `D:/live2dai/tts-cosyvoice3`（另一条线维护），本仓库只有 Rust。
//! - **不写 `[tts]`、不写 `.env`**：合成仍是核心链路的 HTTP 客户端按
//!   `[tts].base_url` 发 `POST {base_url}/audio/speech`。本 Mod 调用
//!   `ModServices.apply_settings` 的次数**恒为 0**（单测有计数间谍守着）。
//! - **不新增宿主端点**、不占 `18080`（那是 Web/API）。
//! - **不下载权重**（CosyVoice3 权重是 sidecar 的事，Apache-2.0）。
//! - **不依赖 WSL2**：不 spawn `wsl.exe`、不跑 bash；`program` 只认 Windows 原生
//!   入口（`.cmd` / `.bat` / `.exe` / `.ps1`），`.sh` 直接报错（见 [`child::entry_argv`]）。
//! - **不订阅事件**：它的可观测面是 `state_json`，不是事件流。
//! - **不替用户改出声地址**：`openai_base_url()` 只出现在 README 与 `state_json`
//!   的对照说明里，**不**写进任何配置。
//!
//! # 为什么替换 MeloTTS（`local-tts-melo`）在 Windows 的角色
//!
//! MeloTTS 的入口是 `melo/start.sh`（bash），Windows 上没有可执行入口——那个
//! Mod 在 Windows 上会**主动拒绝** `.sh`（见它的 `windows_entry_blocker`），
//! 于是「本机出声」在 Windows 上等于没有。CosyVoice3 的定位正好能补上：重活
//! （CUDA、模型、采样率转换）由 Windows 上的 `python.exe` 进程承担，桌面程序
//! 这一侧只剩一个 `.cmd` 启动器，而 8011 端口与 `[tts]` 的 OpenAI 兼容契约完全不变。
//!
//! # settings_spec（五个字段，见 [`spec`]）
//!
//! `program`（缺省 `D:/live2dai/tts-cosyvoice3/start.cmd`）/ `port`（8011）/
//! `voice`（`dafeiyu`，只展示）/ `auto_start`（true）/ `health_timeout_s`（60）。
//!
//! # 失败不重试
//!
//! `start` 返回 `Err` 就是 `Failed`（宿主标 `ModStatus::Failed`）：不自动重试、
//! 不改端口、不换地址、不把失败写成成功。要再试就再点一次启用（或
//! `command("restart")`）。

pub mod child;
pub mod config;
pub mod health;
pub mod runtime;
pub mod spec;
pub mod state;

pub use child::{ChildProbe, ChildProcess, SpawnRequest, Spawner, entry_argv, real_spawn};
pub use config::{
    CosyVoiceConfig, DEFAULT_AUTO_START, DEFAULT_HEALTH_TIMEOUT_S, DEFAULT_PORT, DEFAULT_PROGRAM,
    DEFAULT_VOICE, HEALTH_PATH, MOD_ID, PROBE_TIMEOUT,
};
pub use health::{Prober, http_health, port_accepts};
pub use runtime::{CosyVoice3Runtime, DEFAULT_POLL_INTERVAL};
pub use state::{
    ERR_HEALTH_TIMEOUT, ERR_PORT_IN_USE, ERR_PROGRAM_MISSING, ERR_SPAWN_FAILED, LastError,
    RuntimeState, Status,
};

use live2d_ai_mod_system::{
    MOD_API_VERSION, ModDescriptor, ModError, ModFactory, ModRuntime, ModServices, ModSettingsSpec,
};

/// Mod 描述符（静态身份）。**id 必须是小写加连字符**（与 `mods/<id>/mod.toml`、
/// `mods.json` 的命名空间同一个词）。
pub const DESCRIPTOR: ModDescriptor = ModDescriptor {
    id: config::MOD_ID,
    name: "本地tts_CosyVoice3（大肥鱼）",
    version: env!("CARGO_PKG_VERSION"),
    api_version: MOD_API_VERSION,
};

/// `api_version` 门禁（**纯函数**）：声明值与宿主 `MOD_API_VERSION` 不一致 → `Err`。
///
/// 宿主自己也会拦（`ModRegistry` 拿 `descriptor.api_version` 比 `MOD_API_VERSION`）；
/// 这里多做一次是因为**本 crate 可以脱离宿主被单测**，而「版本对不上」必须能在
/// 自己这一侧测出来，而不是只在集成时才炸。
pub fn check_api_version(mod_id: &str, declared: u32) -> Result<(), ModError> {
    if declared == MOD_API_VERSION {
        Ok(())
    } else {
        Err(ModError::IncompatibleApi {
            mod_id: mod_id.to_string(),
            got: declared,
            expected: MOD_API_VERSION,
        })
    }
}

/// CosyVoice3 sidecar 的静态工厂。
pub struct CosyVoice3Factory;

impl ModFactory for CosyVoice3Factory {
    fn descriptor(&self) -> &'static ModDescriptor {
        &DESCRIPTOR
    }

    /// 静态 schema：本 Mod **缺省停用**，用户要先能填配置再启用。
    fn settings_spec(&self) -> Option<ModSettingsSpec> {
        Some(spec::settings_spec())
    }

    fn create(
        &self,
        services: ModServices,
        config: serde_json::Value,
    ) -> Result<Box<dyn ModRuntime>, ModError> {
        check_api_version(DESCRIPTOR.id, DESCRIPTOR.api_version)?;
        // 坏配置在 create 就 Err（宿主据此把本 Mod 标 Failed），**不 spawn、
        // 不折成缺省继续跑**。
        let config = CosyVoiceConfig::parse(&config).map_err(|message| ModError::Init {
            mod_id: DESCRIPTOR.id.to_string(),
            message,
        })?;
        Ok(Box::new(CosyVoice3Runtime::new(services, config)))
    }
}

/// 工厂单例（挂载时注册 `&COSYVOICE3_FACTORY`）。
pub const COSYVOICE3_FACTORY: CosyVoice3Factory = CosyVoice3Factory;

#[cfg(test)]
#[path = "tests_config.rs"]
mod tests_config;
#[cfg(test)]
#[path = "tests_health.rs"]
mod tests_health;
#[cfg(test)]
#[path = "tests_lifecycle.rs"]
mod tests_lifecycle;
#[cfg(test)]
#[path = "tests_support.rs"]
mod tests_support;
