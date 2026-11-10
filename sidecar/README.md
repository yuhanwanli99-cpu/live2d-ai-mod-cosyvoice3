# CosyVoice3 TTS sidecar（Windows 原生）

把 spike 验证过的 **Fun-CosyVoice3-0.5B-2512** zero-shot 复刻（音色 `dafeiyu`，6 岁小奶音）
包成常驻 HTTP 服务，给 Live2D-Ai 桌面端当 TTS 端点。**不依赖 WSL**：Python 3.10 venv +
torch cu128 + onnxruntime(CPU)，只听 127.0.0.1:8011。

- 接口：OpenAI 兼容 `POST /v1/audio/speech` → **PCM s16le / 44100 Hz / 单声道**（也支持 `wav`）
- 模型原生 24 kHz，服务端重采样到 44.1 kHz（画面侧 44.1k 不用二次重采样，音色不被降采样拖闷）
- 启动加载一次模型（fp32，禁 fp16）+ 预缓存 `dafeiyu` spk 向量（重启 0s 复用）
- 常驻显存：torch 分配 ~3.4 GB；nvidia-smi 总量 ~5.2 GB（含 Windows 桌面基线 ~1.7 GB）

实测（RTX 5060 Laptop 8GB，driver 592.01，Windows 10.0.26200）：冷启动 READY 20-22 s，
warm RTF 0.65（5 s 语音 ≈ 3.5 s 生成），健康探针 5-8 ms。详见 D:\live2dai\plan\COSYVOICE3-SIDECAR.md。

## 快速开始

前置：**只要 NVIDIA 驱动（>= 572，支持 CUDA 12.8）**，不需要 WSL / MSVC / Docker。

~~~powershell
# 1) 一键环境（uv 装 Python3.10 -> .venv -> torch cu128 -> 依赖 -> 模型/源码就位 -> doctor 自检）
pwsh -ExecutionPolicy Bypass -File D:\live2dai\tts-cosyvoice3\setup.ps1

# 2) 启动 / 状态 / 停止
D:\live2dai\tts-cosyvoice3\start.cmd     # 幂等；等 /healthz 就绪后返回（冷启动 ~20-25 s）
D:\live2dai\tts-cosyvoice3\status.cmd
D:\live2dai\tts-cosyvoice3\stop.cmd

# 3) 验收（服务已起）
cd D:\live2dai\tts-cosyvoice3
.venv\Scripts\python.exe tools\accept.py      # 短/长句 RTF、连发 5 次、PCM/WAV 契约 -> out\accept.json
tools\curl_check.cmd                          # curl PCM -> wav -> 时长/峰值（out\curl-pcm.wav）
~~~

E2E（复制 dist-051 -> dist-cv3、改 [tts]、启动 app、POST /api/v1/settings/test/tts）：

~~~powershell
pwsh -File D:\live2dai\tts-cosyvoice3\tools\e2e_dist051.ps1
# 已有 dist（如 dist-cv3mod）只改 toml 不复制：
pwsh -File D:\live2dai\tts-cosyvoice3\tools\e2e_dist051.ps1 -NoCopy -DistDir D:\live2dai\artifacts\dist-cv3mod
~~~

## 目录结构

| 路径 | 说明 |
|---|---|
| `server.py` | 唯一服务实现（FastAPI + uvicorn；文件头有完整契约注释） |
| `setup.ps1` | 一键搭建：uv Python3.10 / .venv / torch cu128 / 依赖 / 源码 / 模型 / doctor |
| `start.cmd` `stop.cmd` `status.cmd` | 入口（**纯 ASCII**，调同名 `.ps1`；cmd 按 GBK 读注释，中文会炸） |
| `start.ps1` `stop.ps1` `status.ps1` | 幂等启停、pid 文件、/healthz 轮询、端口释放等待 |
| `requirements-win.txt` | 除 torch/torchaudio 外的全部依赖（torch 必须走 cu128 index） |
| `ref/` | `voice-anchor.wav`（原 24k 参考音）、`prompt-16k.wav`（模型 prompt 16k 单声道）、`refText.txt` |
| `model/` | 目录联接 -> D:\live2dai\cosyvoice-cache\models\Fun-CosyVoice3-0.5B-2512（不占额外 5GB） |
| `cache/` | `spk-meta.json`（参考音频 sha256；变了自动重算 spk） |
| `logs/` | `server.log`、`requests.jsonl`（每请求一行）、`launch.log`、pid |
| `out/` | 验收证据：`accept.json`、`doctor.json`、`e2e-dist-cv3.json`、curl 的 raw/wav |
| `tools/` | doctor / accept / curl_check / raw_to_wav / wav_info / patch_toml / e2e_dist051 |
| `wsl-reference/` | 最早 WSL 实现（仅参考；同一份 server.py 在 WSL 也能跑） |

CosyVoice 源码在 D:\live2dai\CosyVoice（commit `074ca6d`，Matcha-TTS 子模块 `dd9105b`）。
server.py 用 `sys.path` 引入它和 `third_party/Matcha-TTS`，**不 pip install matcha**（少一层打包风险）。

## HTTP 接口

### POST /v1/audio/speech

~~~json
{"input": "哥哥你好呀。", "voice": "dafeiyu", "model": "cosyvoice3-0.5b",
 "response_format": "pcm", "speed": 1.0}
~~~

| 字段 | 缺省 | 说明 |
|---|---|---|
| `input` | 必填 | 文本；空 -> 400；上限 `COSYVOICE_MAX_CHARS`（缺省 1000 字） |
| `voice` | `dafeiyu` | 其它值 -> 400（多音色待扩展：再加 `add_zero_shot_spk` 即可） |
| `response_format` | `pcm` | `pcm`/`s16le` = 裸 s16le；`wav` = RIFF 容器；其它 -> 400 |
| `speed` | 1.0 | 钳在 [0.5, 2.0]，透传模型 speed |
| `model` | 忽略 | 仅回显用途（OpenAI 兼容字段） |
| `stream` | false | **未实现**分块流；请求了也按整段返回（日志记 note） |

响应头：`X-Sample-Rate: 44100`、`X-Channels: 1`、`X-Format: pcm_s16le|wav`、
`X-Audio-Seconds`、`X-Gen-Ms`、`X-Rtf`、`X-Model`、`X-Voice`。

### GET /healthz

`ok, status, model, model_dir, voice, pid, warm, ready, device, platform, sample_rate,
model_sample_rate, channels, load_s, spk_cached, spk_load_s, uptime_s, requests,
torch_peak_alloc_mb, gpu_alloc_mb, last_error`

`warm = ready && spk_cached`（spk 向量已从 `spk2info.pt` 复用）。

### GET /v1/models 与 GET /v1/voices

主仓「设置 → 测试 TTS」是 **`GET {base_url}/models` 探针**（不合成，见
`crates/live2d-ai-desktop/src/web_api/settings_routes/test_endpoints.rs`）；
本服务的 `/v1/models` 返回 200 + OpenAI 列表，探针因此 2 ms 内通过。

## 主仓接入

任意 dist 的 `live2d-ai.toml`：

~~~toml
[tts]
mode = "local"
base_url = "http://127.0.0.1:8011/v1"
voice = "dafeiyu"
response_format = "pcm"
sample_rate = 44100
channels = 1
model = "cosyvoice3-0.5b"
# 本地 sidecar 不鉴权：不写 api_key_env
~~~

一键改：`.venv\Scripts\python.exe tools\patch_toml.py <live2d-ai.toml 路径>`（只动 `[tts]` 段，注释保留）。

## 环境变量

| 变量 | 缺省 | 用途 |
|---|---|---|
| `COSYVOICE_PORT` / `COSYVOICE_HOST` | 8011 / 127.0.0.1 | 监听（**不要占 18080**） |
| `COSYVOICE_REPO` | 自动探测 `<dir>\..\CosyVoice` / D:\live2dai\CosyVoice | CosyVoice 源码 |
| `COSYVOICE_MODEL_DIR` | `<dir>\model` | 模型目录（可指任意含 6 个必需文件的目录） |
| `COSYVOICE_REF_WAV` / `COSYVOICE_REF_TEXT` | `ref\prompt-16k.wav` / `ref\refText.txt` | 参考音色 |
| `COSYVOICE_VOICE` | dafeiyu | 音色 id |
| `COSYVOICE_WARMUP` | 1 | 启动后先合成一句预热（首请求省 cudnn autotune） |
| `COSYVOICE_MAX_CHARS` | 1000 | 单请求文本上限 |
| `COSYVOICE_WAIT_S` / `COSYVOICE_PY` | 300 / `.venv\Scripts\python.exe` | start.ps1 等待上限 / 解释器 |

## Windows 原生踩坑与处理（都已在本目录代码里解决）

1. **torchaudio 2.11 的 load/save 要 torchcodec（依赖 FFmpeg）**：`server.py` 启动时把
   `torchaudio.load/save` 换成 `soundfile` 实现（Windows 轮子自带 libsndfile），
   加载 prompt wav、写参考 wav 都不碰 FFmpeg/torchcodec。
2. **wetext / pynini 无 Windows 轮子**：不装。CosyVoice 前端日志会打
   `no frontend is avaliable`，中文标点/分句仍能走（非 wetext 分支），但**数字/多音字
   不做文本规范化**——app 侧送 TTS 前自己把数字写成汉字更稳，后续可接 ttsfrd 资源或 coco-tn。
3. **pyworld 其实有 Windows cp310 轮子**（0.3.5，`--only-binary :all:` 可直接装），
   plan 里「需 MSVC 源码编译」的假设不成立；本目录已装（训练/数据集路径 import 需要它）。
4. **matcha 依赖链**要 `conformer==0.3.2`、`gdown`、`matplotlib`、`wget`、`lightning`、
   `hydra-core`、`diffusers`：requirements-win.txt 已含；matcha 用 `sys.path` 引入，不打包安装。
5. **onnxruntime 只有 CPU**（1.18.0）：campplus / speech_tokenizer 走 CPU；
   参考音频 prompt 提取 0.84 s **只做一次**并写 `spk2info.pt`，重启 `spk_load_s=0.0`。
6. **cmd.exe 按 OEM 代码页（zh-CN=GBK）读 .cmd**：所有 `.cmd` 都是纯 ASCII（注释英文），
   中文与错误信息只出现在 `.ps1` / `server.py` 里（PowerShell 7 按 UTF-8 读脚本）。
7. **venv 的 python.exe 是 launcher**，会再拉起 uv 托管的 base python：`logs\server.pid`
   记的是 launcher，`stop.ps1` 另按命令行匹配 `server.py` 兜底杀子进程（stop.cmd 已验证 0 残留）。
8. **8GB 显存和 Live2D 渲染共存**：sidecar 常驻 nvidia-smi ~5.2GB（含桌面 1.7GB），
   余量 ~2.9GB 给浏览器/WebGL；E2E 里 app+浏览器+sidecar 同时跑通过，但建议关掉其它吃显存的程序。

## 故障排查

~~~powershell
# 起不来：先看这两份
Get-Content D:\live2dai\tts-cosyvoice3\logs\server.log -Tail 60
Get-Content D:\live2dai\tts-cosyvoice3\logs\server.stderr.log -Tail 60
# 环境体检（CUDA / import 链 / 模型文件 / 参考音频）；--load 会真加载模型
.venv\Scripts\python.exe tools\doctor.py
~~~

常见原因：GPU 被占导致 CUDA OOM（关掉别的进程再 start）、8011 被旧进程占
（`status.cmd` 看 pid，`stop.cmd` 兜底）、模型目录联接断了（重跑 `setup.ps1`）。

## 发布包用户：源码与模型从哪来（本目录不含权重）

Windows 包（`tools` 目录）里带的是**脚本**，不带 5.1GB 权重、不带参考音色样本：

```powershell
# 1) CosyVoice 源码（Apache-2.0，子模块含 Matcha-TTS）
git clone --recursive https://github.com/FunAudioLLM/CosyVoice D:\live2dai\CosyVoice

# 2) 模型权重（Apache-2.0，非 gated；任选其一）
pip install modelscope
modelscope download --model FunAudioLLM/Fun-CosyVoice3-0.5B-2512 --local_dir D:\live2dai\cosyvoice-cache\models\Fun-CosyVoice3-0.5B-2512

# 3) 一键环境（-Dir 指向你放这些脚本的目录）
pwsh -ExecutionPolicy Bypass -File .\setup.ps1 -Dir D:\live2dai\tts-cosyvoice3
```

- 本目录可放任意位置：改 Mod「sidecar 启动器」字段（`program`，缺省
  `D:/live2dai/tts-cosyvoice3/start.cmd`）指向你的 `start.cmd` 即可。
- 参考音色 `ref/` 是**本地私有样本**（来自大肥鱼语音包），发布包不附带；换音色改
  `ref/` 与 `COSYVOICE_REF_TEXT` 后让缓存自动重算 spk。
- 显存：5060 8GB 上 sidecar 常驻 nvidia-smi ~5.2GB（含桌面 1.7GB），与 Live2D 渲染可同跑，
  但余量只有 ~2.9GB，建议关掉其它吃显存的程序。

## 许可

CosyVoice 代码与权重均 Apache-2.0（非 gated）；参考音色来自大肥鱼语音包的合成样本
（语音包 README 声明不含他人声音）。保留 Apache-2.0 声明即可商用。
