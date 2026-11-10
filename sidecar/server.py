#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""CosyVoice3 TTS sidecar — OpenAI 兼容 /v1/audio/speech（Windows 原生 / WSL 均可）。

角色：把 spike 验证过的 Fun-CosyVoice3-0.5B-2512 zero-shot 复刻（音色 dafeiyu）
包成常驻 HTTP 服务，给 Live2D-Ai 桌面端当 TTS 端点用。**不依赖 WSL**：默认走
Windows 原生 venv（.venv\\Scripts\\python.exe），路径全部可用环境变量覆盖。

契约（与主仓 crates/live2d-ai-runtime/src/tts.rs 对齐）：
  POST /v1/audio/speech {input, model?, voice?, response_format?, speed?}
    response_format = "pcm"（缺省）：raw s16le / 44100 Hz / 单声道
    response_format = "wav"        ：RIFF 容器，同样 44100 Hz / 单声道
  模型原生 24 kHz，服务端重采样到 44100 Hz（保持 24k，画面侧 44.1k 不用二次重采样）。
  GET  /healthz   -> {ok, model, voice, pid, warm, ...}
  GET  /v1/models -> OpenAI 列表（主仓「设置 → 测试 TTS」就是 GET {base}/models 探针）
  GET  /v1/voices -> 可用音色

启动纪律：
  - 进程内只加载一次模型（fp32，禁 fp16：spike 实测更慢更占显存）；
  - 启动时把参考音频的 spk 向量 add_zero_shot_spk 缓存成 spk2info.pt（模型目录），
    之后重启直接复用，省掉每请求 1.2-2.4s 的 onnx CPU prompt 提取；
  - 参考音频 / 文本改了（sha256 变化）就重算缓存。
"""

from __future__ import annotations

import asyncio
import hashlib
import io
import json
import logging
import os
import sys
import threading
import time
import wave
from contextlib import asynccontextmanager
from typing import Any

# --------------------------------------------------------------------------- #
# 路径解析（Windows 原生优先；WSL 旧路径作兜底，便于同一份代码两处跑）
# --------------------------------------------------------------------------- #
DIR = os.path.dirname(os.path.abspath(__file__))              # ...\tts-cosyvoice3
LOG_DIR = os.environ.get("COSYVOICE_LOG_DIR", os.path.join(DIR, "logs"))
CACHE_DIR = os.environ.get("COSYVOICE_CACHE_DIR", os.path.join(DIR, "cache"))

REPO_CANDIDATES = [
    os.environ.get("COSYVOICE_REPO", ""),
    os.path.join(os.path.dirname(DIR), "CosyVoice"),          # D:\live2dai\CosyVoice
    "D:/live2dai/CosyVoice",
    os.path.expanduser("~/cosyvoice3/CosyVoice"),
]
MODEL_CANDIDATES = [
    os.environ.get("COSYVOICE_MODEL_DIR", ""),
    os.path.join(DIR, "model"),                               # D:\live2dai\tts-cosyvoice3\model
    "D:/live2dai/cosyvoice-cache/models/Fun-CosyVoice3-0.5B-2512",
    os.path.expanduser("~/cosyvoice3/cosyvoice-cache/models/Fun-CosyVoice3-0.5B-2512"),
]


def _first_dir(cands: list[str]) -> str:
    for c in cands:
        if c and os.path.isdir(c):
            return os.path.abspath(c)
    return cands[1] if len(cands) > 1 else ""


REPO = _first_dir(REPO_CANDIDATES)
MODEL_DIR = _first_dir(MODEL_CANDIDATES)
REF_WAV = os.environ.get("COSYVOICE_REF_WAV", os.path.join(DIR, "ref", "prompt-16k.wav"))
REF_TEXT_FILE = os.environ.get("COSYVOICE_REF_TEXT", os.path.join(DIR, "ref", "refText.txt"))

HOST = os.environ.get("COSYVOICE_HOST", "127.0.0.1")
PORT = int(os.environ.get("COSYVOICE_PORT", "8011"))
MODEL_ID = os.environ.get("COSYVOICE_MODEL_ID", "cosyvoice3-0.5b")
VOICE_ID = os.environ.get("COSYVOICE_VOICE", "dafeiyu")
# CosyVoice3 约定：prompt_text 必须带这个前缀（少了复刻质量会崩）。
PROMPT_PREFIX = os.environ.get("COSYVOICE_PROMPT_PREFIX", "You are a helpful assistant.<|endofprompt|>")
OUT_SAMPLE_RATE = 44100                 # 对外契约（主仓 [tts] sample_rate = 44100）
OUT_CHANNELS = 1
MAX_INPUT_CHARS = int(os.environ.get("COSYVOICE_MAX_CHARS", "1000"))
WARMUP = os.environ.get("COSYVOICE_WARMUP", "1") not in ("0", "false", "False", "")
WARMUP_TEXT = os.environ.get("COSYVOICE_WARMUP_TEXT", "你好呀，我是大肥鱼。")

SUPPORTED_FORMATS = {"pcm": "audio/pcm", "s16le": "audio/pcm", "wav": "audio/wav"}

os.makedirs(LOG_DIR, exist_ok=True)
os.makedirs(CACHE_DIR, exist_ok=True)

_fmt = "%(asctime)s %(levelname)s %(name)s: %(message)s"
logging.basicConfig(level=logging.INFO, format=_fmt, stream=sys.stderr)
try:  # 日志落盘（start.cmd 也重定向 stdio，但文件 handler 保证 uvicorn 的日志同样落盘）
    _fh = logging.FileHandler(os.path.join(LOG_DIR, "server.log"), encoding="utf-8")
    _fh.setFormatter(logging.Formatter(_fmt))
    logging.getLogger().addHandler(_fh)
except Exception:
    pass
log = logging.getLogger("cosyvoice3-sidecar")

_io_lock = threading.Lock()             # 模型推理不是线程安全的：串行化
_req_log_lock = threading.Lock()


# --------------------------------------------------------------------------- #
# torchaudio 后端垫片：torchaudio 2.9+ 的 load/save 走 torchcodec（要 FFmpeg），
# 这里直接用 soundfile（Windows 轮子自带 libsndfile），零外部依赖。
# --------------------------------------------------------------------------- #
def _patch_torchaudio() -> None:
    import soundfile as sf
    import torch
    import torchaudio

    def _load(path, *args, **kwargs):
        data, sr = sf.read(str(path), dtype="float32", always_2d=True)
        return torch.from_numpy(data.T.copy()), sr

    def _save(path, src, sample_rate, *args, **kwargs):
        arr = src.detach().cpu().numpy()
        if arr.ndim == 2:
            arr = arr.T
        sf.write(str(path), arr, int(sample_rate), subtype="PCM_16")

    torchaudio.load = _load          # type: ignore[assignment]
    torchaudio.save = _save          # type: ignore[assignment]


def _sha256_of(*paths: str) -> str:
    h = hashlib.sha256()
    for p in paths:
        h.update(os.path.basename(p).encode("utf-8"))
        with open(p, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
    return h.hexdigest()


def _read_ref_text() -> str:
    with open(REF_TEXT_FILE, "r", encoding="utf-8") as f:
        return f.read().strip()


def _wav_bytes(pcm_i16, sample_rate: int) -> bytes:
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(OUT_CHANNELS)
        w.setsampwidth(2)
        w.setframerate(sample_rate)
        w.writeframes(pcm_i16.tobytes())
    return buf.getvalue()


# --------------------------------------------------------------------------- #
# 引擎（模型 + spk 缓存）
# --------------------------------------------------------------------------- #
class Engine:
    def __init__(self) -> None:
        self.cv: Any = None
        self.ready = False
        self.load_s: float | None = None
        self.spk_cached: bool | None = None
        self.spk_load_s: float | None = None
        self.ref_sha: str | None = None
        self.device = "unknown"
        self.torch_peak_alloc_mb: float | None = None
        self.started_at = time.time()
        self.requests = 0
        self.last_error: str | None = None
        self.model_sample_rate = 24000

    # ---- 加载 -------------------------------------------------------------
    def load(self) -> None:
        t0 = time.time()
        os.makedirs(CACHE_DIR, exist_ok=True)
        os.makedirs(LOG_DIR, exist_ok=True)

        if not os.path.isdir(REPO):
            raise RuntimeError(f"CosyVoice source not found (COSYVOICE_REPO={REPO}); run setup.ps1")
        if not os.path.isdir(MODEL_DIR):
            raise RuntimeError(f"model dir not found (COSYVOICE_MODEL_DIR={MODEL_DIR}); run setup.ps1")
        sys.path.insert(0, REPO)
        sys.path.insert(0, os.path.join(REPO, "third_party", "Matcha-TTS"))
        log.info("repo=%s", REPO)
        log.info("model_dir=%s", MODEL_DIR)

        import torch
        import torchaudio  # noqa: F401  (先装垫片再 import cosyvoice)
        _patch_torchaudio()
        torch.backends.cudnn.benchmark = False

        log.info("loading model (fp32)...")
        from cosyvoice.cli.cosyvoice import AutoModel

        os.chdir(REPO)                       # CosyVoice 内部有相对资源路径
        self.cv = AutoModel(model_dir=MODEL_DIR, fp16=False)
        self.model_sample_rate = int(self.cv.sample_rate)
        self.device = "cuda" if torch.cuda.is_available() else "cpu"
        if torch.cuda.is_available():
            log.info("cuda: %s", torch.cuda.get_device_name(0))
        self.load_s = round(time.time() - t0, 2)
        log.info("model loaded in %.2fs (native sr=%d)", self.load_s, self.model_sample_rate)

        # ---- spk 预缓存（ref 变了就重算）----
        t1 = time.time()
        self.ref_sha = _sha256_of(REF_WAV, REF_TEXT_FILE)
        meta_path = os.path.join(CACHE_DIR, "spk-meta.json")
        cached_ok = False
        try:
            with open(meta_path, "r", encoding="utf-8") as f:
                meta = json.load(f)
            cached_ok = (
                meta.get("sha256") == self.ref_sha
                and meta.get("voice") == VOICE_ID
                and VOICE_ID in self.cv.list_available_spks()
            )
        except Exception:
            cached_ok = False

        if cached_ok:
            log.info("spk %s reused from cached spk2info.pt", VOICE_ID)
        else:
            prompt_text = PROMPT_PREFIX + _read_ref_text()
            log.info("building spk embedding for %s from %s (onnx CPU, 1-3s)", VOICE_ID, REF_WAV)
            assert self.cv.add_zero_shot_spk(prompt_text, REF_WAV, VOICE_ID) is True
            self.cv.save_spkinfo()
            with open(meta_path, "w", encoding="utf-8") as f:
                json.dump(
                    {
                        "voice": VOICE_ID,
                        "sha256": self.ref_sha,
                        "ref_wav": os.path.basename(REF_WAV),
                        "saved_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                    },
                    f,
                    ensure_ascii=False,
                    indent=2,
                )
        self.spk_cached = bool(cached_ok)
        self.spk_load_s = round(time.time() - t1, 2)
        if not self.spk_cached:
            log.info("spk cached in %.2fs -> %s/spk2info.pt", self.spk_load_s, MODEL_DIR)

        if torch.cuda.is_available():
            self.torch_peak_alloc_mb = round(torch.cuda.max_memory_allocated() / 1024 ** 2, 1)

        if WARMUP:
            try:
                tw = time.time()
                self.synthesize_pcm(WARMUP_TEXT, speed=1.0)
                log.info("warmup synthesis in %.2fs", time.time() - tw)
            except Exception as e:  # 预热失败不致命
                log.warning("warmup failed: %s", e)

        self.ready = True
        log.info("READY in %.2fs total", time.time() - self.started_at)

    # ---- 合成 -------------------------------------------------------------
    def synthesize_pcm(self, text: str, speed: float = 1.0) -> tuple[Any, dict]:
        """返回 (int16 numpy [n], 统计)。模型原生 24k 生成后重采样到 44.1k。"""
        import numpy as np
        import torch
        import torchaudio.functional as AF

        with _io_lock:
            t0 = time.time()
            chunks = []
            for out in self.cv.inference_zero_shot(
                text, "", "", VOICE_ID, stream=False, speed=float(speed)
            ):
                chunks.append(out["tts_speech"])
            gen_s = time.time() - t0
            if not chunks:
                raise RuntimeError("model produced no audio")
            speech = torch.cat([c.reshape(1, -1) for c in chunks], dim=1).detach().float().cpu()
            audio_in_s = speech.shape[1] / self.model_sample_rate

            t1 = time.time()
            if self.model_sample_rate != OUT_SAMPLE_RATE:
                speech = AF.resample(speech, self.model_sample_rate, OUT_SAMPLE_RATE)
            pcm = torch.round(speech.clamp(-1.0, 1.0) * 32767.0).to(torch.int16).numpy()
            if pcm.ndim == 2:
                pcm = pcm[0]
            pcm = np.ascontiguousarray(pcm)
            resample_s = time.time() - t1

        stats = {
            "gen_ms": round(gen_s * 1000, 1),
            "resample_ms": round(resample_s * 1000, 1),
            "audio_in_s": round(audio_in_s, 3),
            "audio_s": round(len(pcm) / OUT_SAMPLE_RATE, 3),
            "rtf": round(gen_s / audio_in_s, 3) if audio_in_s > 0 else None,
            "peak": int(np.abs(pcm).max()) if pcm.size else 0,
        }
        return pcm, stats


engine = Engine()


# --------------------------------------------------------------------------- #
# FastAPI 应用
# --------------------------------------------------------------------------- #
try:
    from fastapi import FastAPI
    from fastapi.responses import JSONResponse, Response
    from pydantic import BaseModel, Field
except ImportError as e:  # pragma: no cover
    print(f"missing dependency: {e}. 先跑 setup.ps1（或 pip install -r requirements-win.txt）", file=sys.stderr)
    raise


class SpeechRequest(BaseModel):
    input: str = Field(default="", description="要合成的文本")
    model: str | None = None
    voice: str = VOICE_ID
    response_format: str = "pcm"
    speed: float = 1.0
    stream: bool = False


def _err(status: int, message: str, code: str = "invalid_request_error") -> JSONResponse:
    return JSONResponse(
        status_code=status,
        content={"error": {"message": message, "type": code, "code": code}},
    )


@asynccontextmanager
async def lifespan(app: FastAPI):
    await asyncio.to_thread(engine.load)
    yield
    log.info("shutting down")


app = FastAPI(title="CosyVoice3 TTS sidecar", version="1.0.0", lifespan=lifespan)


@app.get("/healthz")
async def healthz() -> dict:
    import torch

    gpu_used = None
    try:
        if torch.cuda.is_available():
            gpu_used = round(torch.cuda.memory_allocated() / 1024 ** 2, 1)
    except Exception:
        pass
    return {
        "ok": engine.ready,
        "status": "ready" if engine.ready else "loading",
        "model": MODEL_ID,
        "model_dir": MODEL_DIR,
        "voice": VOICE_ID,
        "pid": os.getpid(),
        "warm": bool(engine.ready and engine.spk_cached),
        "ready": engine.ready,
        "device": engine.device,
        "platform": sys.platform,
        "sample_rate": OUT_SAMPLE_RATE,
        "model_sample_rate": engine.model_sample_rate,
        "channels": OUT_CHANNELS,
        "load_s": engine.load_s,
        "spk_cached": engine.spk_cached,
        "spk_load_s": engine.spk_load_s,
        "uptime_s": round(time.time() - engine.started_at, 1),
        "requests": engine.requests,
        "torch_peak_alloc_mb": engine.torch_peak_alloc_mb,
        "gpu_alloc_mb": gpu_used,
        "last_error": engine.last_error,
    }


@app.get("/v1/models")
async def models() -> dict:
    now = int(time.time())
    return {
        "object": "list",
        "data": [
            {"id": MODEL_ID, "object": "model", "created": now, "owned_by": "local"},
            {"id": VOICE_ID, "object": "model", "created": now, "owned_by": "local"},
        ],
    }


@app.get("/v1/voices")
async def voices() -> dict:
    return {"object": "list", "data": [{"id": VOICE_ID, "object": "voice", "engine": MODEL_ID}]}


@app.post("/v1/audio/speech")
def speech(req: SpeechRequest) -> Response:
    """同步合成（FastAPI 放到线程池跑，/healthz 与 /v1/models 不被阻塞）。"""
    if not engine.ready:
        return _err(503, "engine not ready", "service_unavailable")
    text = (req.input or "").strip()
    if not text:
        return _err(400, "input is empty")
    if len(text) > MAX_INPUT_CHARS:
        return _err(400, f"input too long ({len(text)} > {MAX_INPUT_CHARS} chars)")
    fmt = (req.response_format or "pcm").lower()
    if fmt not in SUPPORTED_FORMATS:
        return _err(400, f"unsupported response_format {req.response_format} (use pcm|wav)")
    if req.voice != VOICE_ID:
        return _err(400, f"unknown voice {req.voice} (available: {VOICE_ID})", "voice_not_found")
    speed = min(max(float(req.speed or 1.0), 0.5), 2.0)

    t0 = time.time()
    try:
        pcm, stats = engine.synthesize_pcm(text, speed=speed)
    except Exception as e:  # 合成失败：记录并回 500（主仓会按 Error::Status 报出来）
        engine.last_error = f"{type(e).__name__}: {e}"
        log.exception("synthesis failed")
        return _err(500, engine.last_error, "synthesis_failed")

    total_s = time.time() - t0
    body = _wav_bytes(pcm, OUT_SAMPLE_RATE) if fmt == "wav" else pcm.tobytes()
    engine.requests += 1
    rec = {
        "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "chars": len(text),
        "voice": VOICE_ID,
        "format": fmt,
        "speed": speed,
        "stream": bool(req.stream),
        "gen_ms": stats["gen_ms"],
        "total_ms": round(total_s * 1000, 1),
        "audio_s": stats["audio_s"],
        "rtf_gen": stats["rtf"],
        "rtf_total": round(total_s / stats["audio_s"], 3) if stats["audio_s"] else None,
        "peak": stats["peak"],
        "bytes": len(body),
    }
    if req.stream:
        rec["note"] = "stream=true 未实现分块流，按整段返回"
    log.info("tts %s", json.dumps(rec, ensure_ascii=False))
    try:
        with _req_log_lock:
            with open(os.path.join(LOG_DIR, "requests.jsonl"), "a", encoding="utf-8") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    except Exception:
        pass

    headers = {
        "X-Sample-Rate": str(OUT_SAMPLE_RATE),
        "X-Channels": str(OUT_CHANNELS),
        "X-Format": "pcm_s16le" if fmt != "wav" else "wav",
        "X-Audio-Seconds": str(stats["audio_s"]),
        "X-Gen-Ms": str(round(stats["gen_ms"], 1)),
        "X-Rtf": str(stats["rtf"]),
        "X-Model": MODEL_ID,
        "X-Voice": VOICE_ID,
    }
    return Response(content=body, media_type=SUPPORTED_FORMATS[fmt], headers=headers)


@app.get("/")
async def root() -> dict:
    return {
        "service": "cosyvoice3-tts-sidecar",
        "endpoints": ["POST /v1/audio/speech", "GET /healthz", "GET /v1/models", "GET /v1/voices"],
        "sample_rate": OUT_SAMPLE_RATE,
        "voice": VOICE_ID,
    }


# --------------------------------------------------------------------------- #
# main
# --------------------------------------------------------------------------- #
def main() -> None:
    import uvicorn

    logging.getLogger("uvicorn.access").setLevel(logging.WARNING)
    uvicorn.run(app, host=HOST, port=PORT, log_level="info", access_log=False, log_config=None)


if __name__ == "__main__":
    main()
