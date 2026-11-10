# live2d-ai-mod-cosyvoice3

CosyVoice3 本地 TTS sidecar 的**守护 Mod**。crate 名 `live2d-ai-mod-cosyvoice3`，
稳定 id **`cosyvoice3-tts`**，工厂 **`COSYVOICE3_FACTORY`**，`api_version = 1`。

这个仓库是**独立本地仓库**（仓库根 = crate 根），只**拉起 + 守护** Windows 上的
CosyVoice3 sidecar；语音合成本身仍由核心链路按 `[tts]` 发 OpenAI 兼容请求。

- 依赖方向只有 **Mod → 主仓**：`live2d-ai-mod-system` 走 path 指
  `../Live2D-Ai/crates/live2d-ai-mod-system`；不指旧第二仓、不指其它 Mod 仓库。
- 本仓是独立 workspace（`Cargo.toml` 末尾空的 `[workspace]`），可以单独跑门禁。

## 这是什么 / 不是什么

**是**：一个 `spawn-only + 探活` 的 Mod。启用时探一次 `GET
http://127.0.0.1:<port>/healthz`（2 秒超时）：已经是 2xx 就直接 `ready`；没就绪就
spawn `program`（缺省 `D:/live2dai/tts-cosyvoice3/start.cmd`），此后**每 1 秒**再探，
直到 `health_timeout_s`。停用时**只收自己拉起的那棵进程树**。

**不是**：

- **不做推理、不带权重、不带 sidecar 代码**：`server.py` / `start.cmd` / 参考音频 /
  模型权重都在 `D:/live2dai/tts-cosyvoice3`（另一条线维护），本仓只有 Rust；
- **不写 `[tts]`、不写 `.env`**：合成仍是核心链路按 `[tts].base_url` 发
  `POST {base_url}/audio/speech`；本 Mod 调用 `ModServices.apply_settings` 的次数
  **恒为 0**（单测有计数间谍守着）；
- **不下载权重**、不新增宿主端点、不占 `18080`（Web/API 端口）；
- **不订阅事件**：可观测面是 `state_json`，不是事件流；
- **不自动重试**：`start` 返回 `Err` 就是 `Failed`（不重试、不改端口、不换地址）。

## 为什么替换 MeloTTS（`local-tts-melo`）在 Windows 的角色

`local-tts-melo` 的入口是 `melo/start.sh`（bash）。Windows 上没有 bash 可执行入口，
那个 Mod 在 Windows 上会**主动拒绝** `.sh`（见它的 `windows_entry_blocker`），于是
「本机出声」在 Windows 上等于没有。

CosyVoice3 的定位正好补上：重活（CUDA、模型、24k→44.1k 重采样）由本机
`python.exe` 进程承担，桌面程序这一侧只剩一个 `.cmd` 启动器，而 8011 端口与
`[tts]` 的 OpenAI 兼容契约完全不变——**核心链路一行都不用改**。

## Windows 前置（**不需要 WSL2**）

| 需要 | 说明 |
| --- | --- |
| NVIDIA 显卡 + 驱动 | 驱动侧提供 CUDA 运行时；不要求装 WSL2、不要求 bash |
| 本机 CosyVoice3 模型 | 权重 **Apache-2.0**（FunAudioLLM/Fun-CosyVoice3-0.5B-2512），放在 sidecar 目录里 |
| `D:/live2dai/tts-cosyvoice3/start.cmd` | Windows 原生启动器：直接起 `python.exe`（uvicorn 监听 `127.0.0.1:8011`），**不经过 `wsl.exe`** |
| 端口 8011 空闲 | 不用 18080（那是本项目 Web/API） |

本 Mod **不 spawn `wsl.exe`、不跑 bash**：`program` 只认 Windows 原生入口
（`.cmd` / `.bat` / `.exe` / `.ps1`，以及无扩展名的 exe）；`.sh` 会在 spawn 前**报错**并说明
原因，而不是把脚本扔给 `CreateProcess` 或偷偷调 wsl。

## 它怎么工作（生命周期）

| 阶段 | 行为 |
| --- | --- |
| 注册 | `AVAILABLE_MOD_FACTORIES` 注册工厂；`descriptor` 静态可见，**未启用也能拿到 `settings_spec`** |
| enable / `start` | `starting` → 探活（2 秒）→ 就绪即 `ready`；未就绪按顺序判 `program_missing` / `port_in_use` → spawn → 每 1 秒轮询到 `health_timeout_s` → 2xx 即 `ready` |
| 失败 | 状态 `failed`、`last_error` 记原因码；`start` 返回可读 `Err`，宿主标 `ModStatus::Failed`（主链继续） |
| disable / `shutdown` / `drop` | 只停**自己拉起的**进程树（Windows `taskkill /T /F`；Unix 进程组）；外部已就绪的 sidecar 一概不碰 |
| `apply_config`（宿主叫 apply_settings） | **原子**：先全量校验（坏配置 → 可读错误，运行态一个字段都不改），再一次性替换；运行中拒绝原地改 |

超时窗口内拉起的进程若一直不就绪，会被**收掉**再报 `health_timeout`——不留给用户一个
占着端口却永远不就绪的半成品。

## 设置（`settings_spec`，五个字段）

| key | 类型 | 缺省 | 说明 |
| --- | --- | --- | --- |
| `program` | string | `D:/live2dai/tts-cosyvoice3/start.cmd` | sidecar 启动器；空 = 用缺省 |
| `port` | number | `8011` | sidecar 的 HTTP 端口（探活地址 `http://127.0.0.1:<port>/healthz`） |
| `voice` | string | `dafeiyu` | **只展示**：与 `[tts].voice` 对照；本 Mod 不写 `[tts]` |
| `auto_start` | bool | `true` | 启用本 Mod 时是否拉起 sidecar；关掉 = 只探活，服务由用户自己起 |
| `health_timeout_s` | number | `60` | 拉起后等 `/healthz` 变 2xx 的上限（1..=600） |

`port` / `health_timeout_s` 在契约里是 `Number` 字段（没有 default 槽），缺省值写在
label 里，并由配置解析兜底：键缺失或空 → 按 8011 / 60 跑，与面板写的字一致。

## `state_json`（`GET /api/v1/mods/cosyvoice3-tts/state`）

```json
{"status":"ready","running":true,"pid":4242,"port":8011,"voice":"dafeiyu","last_error":null}
```

| 键 | 类型 | 含义 |
| --- | --- | --- |
| `status` | string | `disabled` / `starting` / `ready` / `failed` |
| `running` | bool | `status == "ready"`（**已确认**在服务，不是「spawn 过」） |
| `pid` | number \| null | **本 Mod 拉起的**子进程 pid；外部已就绪的 sidecar → `null` |
| `port` | number | 当前配置端口 |
| `voice` | string | 当前配置音色（只展示） |
| `last_error` | string \| null | 最近一次失败原因码；成功一次即清空 |

四个失败原因码（**文档化字符串**，排障按它归类）：

| 码 | 什么时候 | 下一步 |
| --- | --- | --- |
| `program_missing` | `program` 指向的启动器不存在 | 把 `start.cmd` 放到 `D:/live2dai/tts-cosyvoice3/`，或改 `program` |
| `port_in_use` | 端口有人应答 TCP，但 `/healthz` 不是 2xx | 那是别的进程：换端口或先关掉它（本 Mod 不会硬起第二个） |
| `spawn_failed` | 拉不起来 / 起来后**就绪前**退出 | 看 Err 里带的启动器 stderr（python 环境、驱动/显存、端口自查） |
| `health_timeout` | `health_timeout_s` 内 `/healthz` 一直不是 2xx | sidecar 载模型慢就调大超时；再不行看 sidecar 日志 |

## 用户的 `[tts]` 配置（**用户自己填，本 Mod 不写**）

`live2d-ai.toml`：

```toml
[tts]
mode = "local"
base_url = "http://127.0.0.1:8011/v1"
voice = "dafeiyu"
response_format = "pcm"
sample_rate = 44100
channels = 1
```

- `base_url` 必须是 sidecar 的 **`/v1`** 基址（核心链路发 `POST {base_url}/audio/speech`）；
- `response_format = "pcm"`：sidecar 回裸 s16le、44.1 kHz、单声道；
- `voice` 是 sidecar 里预缓存的音色 id（缺省 `dafeiyu`）；`model` 可省（省略时请求体不带它）；
- 本地 127.0.0.1 不需要 `api_key_env`。

## 挂载到主仓（two-repo-mods 五步，**仅本地，不推主仓**）

没有动态加载器：这个 crate 不编进二进制就不存在。按顺序做完五步（缺一行就是半接线）：

1. 主仓根 `Cargo.toml` 的 `[workspace.dependencies]` 加：
   `live2d-ai-mod-cosyvoice3 = { path = "../live2d-ai-mod-cosyvoice3" }`；
2. `crates/live2d-ai-desktop/Cargo.toml` 加 `live2d-ai-mod-cosyvoice3 = { workspace = true }`；
3. `crates/live2d-ai-desktop/src/main.rs` 的 `AVAILABLE_MOD_FACTORIES` 注册
   `&live2d_ai_mod_cosyvoice3::COSYVOICE3_FACTORY`，并同步 `mod_count_is_*` /
   `mod_factory_ids_match_expected`（数量断言按实际数改）；
4. 宿主路由：**本 Mod 没有独立宿主路由**（不新增端点、不需要 404 `mod_disabled` 门），
   这一步只需确认没有残留的 TTS 相关路由要恢复；
5. 缺省启用：**不要**加进 `cli_entry::default_mods_manifest`（本 Mod 缺省停用；
   要用就在设置页打开开关）。若维护者确实要缺省启用，才同步那一段。

以上全部是**本地工作树改动**，评审逐条看。**不推主仓**、不把本仓写成主仓的 `git` 依赖。

## 一次性命令（宿主 `POST .../command`）

| 命令 | 行为 |
| --- | --- |
| `probe` | 探一次 `/healthz`，按结果更新 `status`（曾 ready 现在不通 → `failed` + `health_timeout`） |
| `start` | 等价于 enable 的就绪流程（探活 → 必要时 spawn → 轮询） |
| `stop` | 只停自己拉起的进程树 |
| `restart` | 先 `stop` 再 `start` |

未知命令回 `ModError::UnsupportedCommand`（宿主回 409 `unsupported_command`）。

## 门禁

```bash
cargo test --all-targets           # 40 个单测（含真 TCP 假 HTTP sidecar / 假子进程）
cargo test --doc
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
```

Windows 目标交叉检查（WSL 里跑，PATH 加 mingw；target 放 D 盘）：

```bash
export PATH=$HOME/.local/bin:$PATH
export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export CARGO_TARGET_DIR=/mnt/d/live2dai/build/target-win-cosyvoice3
cargo check --target x86_64-pc-windows-gnu --all-targets
```

## 排障

| 现象 | 查什么 |
| --- | --- |
| 启用后立刻 `Failed`，`last_error = program_missing` | `D:/live2dai/tts-cosyvoice3/start.cmd` 在不在；`program` 是不是写成了相对路径 |
| `port_in_use` | 用 `netstat -ano` 查占着 8011 的 pid；那个进程不是 CosyVoice3 就换端口或先关掉它 |
| `spawn_failed`，Err 里带 stderr | 启动器自己印的原因优先看：`python.exe` 在不在、依赖装没装、显存够不够 |
| `health_timeout` | 手动 `curl.exe -s -o NUL -w "%{http_code}" http://127.0.0.1:8011/healthz`；首次载权重慢就把 `health_timeout_s` 调大 |
| `ready` 但没声音 | 那是**核心链路**的问题：看 `[tts]` 的 `base_url` / `voice` / `response_format`，以及 sidecar 的 `POST /v1/audio/speech` |
| 停用后端口还占着 | 只可能是**外部**起的 sidecar（本 Mod 只停自己拉起的）；手动停它 |

## 许可与素材

- 本仓库源码：**AGPL-3.0-only**（见 `LICENSE`）。
- CosyVoice3 权重：**Apache-2.0**（FunAudioLLM/Fun-CosyVoice3-0.5B-2512）——**不随本仓分发**，
  由 sidecar 目录自持。
- 参考音色（`voice-anchor-6yo.wav` 等）：是**合成样本**（由本地 TTS 生成、不是真人声纹采集），
  也只留在 sidecar 目录，不进这个仓库。

公开仓库：<https://github.com/yuhanwanli99-cpu/live2d-ai-mod-cosyvoice3>（分支 `main`）。
