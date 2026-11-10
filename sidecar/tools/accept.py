#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""CosyVoice3 sidecar 验收（Windows 原生，仅用标准库 + numpy）。

覆盖：/healthz、/v1/models、短句/长句 warm 延迟与 RTF、连发 5 次、PCM/WAV 契约校验。
跑法（服务已起）：
    .venv\Scripts\python.exe tools\accept.py
输出：out\accept.json + out\*.raw / out\*.wav
"""
import json
import os
import statistics
import sys
import time
import urllib.error
import urllib.request

if sys.platform == "win32":  # 控制台/管道统一 UTF-8，避免中文在 cp936 下乱码
    for _s in (sys.stdout, sys.stderr):
        try:
            _s.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(DIR, "out")
os.makedirs(OUT, exist_ok=True)

BASE = os.environ.get("COSYVOICE_BASE", "http://127.0.0.1:8011")
SHORT = "哥哥你好呀。"
LONG = "哥哥你听我说嘛，今天老师布置的作业我全都写完了，还自己检查了一遍，然后就一直在等你回来陪我玩呢！"
LOOP = "这个题我会！你先听我说嘛～咯咯咯"


def http_json(path, timeout=10):
    with urllib.request.urlopen(BASE + path, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8"))


def synth(text, fmt="pcm", name="x"):
    body = json.dumps({"input": text, "voice": "dafeiyu", "response_format": fmt,
                       "model": "cosyvoice3-0.5b"}, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(BASE + "/v1/audio/speech", data=body,
                                 headers={"Content-Type": "application/json"}, method="POST")
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=180) as r:
        payload = r.read()
        hdr = {k.lower(): v for k, v in r.getheaders()}
        status = r.status
    wall = time.time() - t0
    ext = "wav" if fmt == "wav" else "raw"
    path = os.path.join(OUT, name + "." + ext)
    with open(path, "wb") as f:
        f.write(payload)
    audio_s = float(hdr.get("x-audio-seconds", 0)) or (len(payload) / 2 / 44100)
    return {
        "name": name, "chars": len(text), "format": fmt, "http": status,
        "wall_s": round(wall, 3), "audio_s": round(audio_s, 3),
        "rtf": round(wall / audio_s, 3) if audio_s else None,
        "gen_ms": float(hdr.get("x-gen-ms", 0)),
        "sample_rate": int(hdr.get("x-sample-rate", 0)),
        "channels": int(hdr.get("x-channels", 0)),
        "bytes": len(payload), "file": path,
        "pcm": pcm_stats(payload) if fmt == "pcm" else None,
    }


def pcm_stats(raw):
    import numpy as np
    a = np.frombuffer(raw, dtype="<i2")
    if a.size == 0:
        return {"frames": 0}
    peak = int(np.abs(a.astype("int32")).max())
    rms = float(np.sqrt(np.mean(a.astype("float64") ** 2)))
    return {
        "frames": int(a.size), "audio_s": round(a.size / 44100, 3), "peak": peak,
        "peak_dbfs": round(20 * np.log10(max(peak, 1) / 32768.0), 2),
        "rms_dbfs": round(20 * np.log10(max(rms, 1e-9) / 32768.0), 2),
        "clipped_pct": round(100.0 * float((np.abs(a.astype("int32")) >= 32767).sum()) / a.size, 4),
    }


def main():
    import wave
    res = {"base": BASE, "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "runs": {}}
    t = time.time()
    res["healthz"] = http_json("/healthz")
    res["healthz_ms"] = round((time.time() - t) * 1000, 1)
    res["models"] = http_json("/v1/models")
    res["voices"] = http_json("/v1/voices")

    res["runs"]["short"] = synth(SHORT, "pcm", "short")
    res["runs"]["long"] = synth(LONG, "pcm", "long")
    res["runs"]["wav"] = synth(SHORT, "wav", "short-fmt-wav")

    loop = [synth(LOOP, "pcm", "loop%d" % i) for i in range(5)]
    res["runs"]["loop"] = loop
    walls = [x["wall_s"] for x in loop]
    rtfs = [x["rtf"] for x in loop if x["rtf"]]
    res["loop_summary"] = {
        "n": len(loop), "wall_s": walls,
        "wall_median_s": round(statistics.median(walls), 3),
        "rtf": rtfs, "rtf_median": round(statistics.median(rtfs), 3) if rtfs else None,
    }
    res["healthz_after"] = http_json("/healthz")

    wav_path = os.path.join(OUT, "short-fmt-wav.wav")
    with open(wav_path, "rb") as f:
        head = f.read(12)
    with wave.open(wav_path, "rb") as w:
        res["wav_check"] = {
            "riff": head[:4] == b"RIFF", "wave": head[8:12] == b"WAVE",
            "channels": w.getnchannels(), "rate": w.getframerate(),
            "sampwidth": w.getsampwidth(), "duration_s": round(w.getnframes() / w.getframerate(), 3),
        }

    with open(os.path.join(OUT, "accept.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=2)

    print(json.dumps({
        "healthz": {k: res["healthz"].get(k) for k in
                    ("ok", "warm", "model", "voice", "pid", "load_s", "spk_cached", "sample_rate", "platform")},
        "healthz_ms": res["healthz_ms"],
        "models_count": len(res["models"].get("data", [])),
        "short": {k: res["runs"]["short"][k] for k in ("wall_s", "audio_s", "rtf", "gen_ms")},
        "long": {k: res["runs"]["long"][k] for k in ("wall_s", "audio_s", "rtf", "gen_ms")},
        "loop": res["loop_summary"],
        "wav_check": res["wav_check"],
        "pcm_short": res["runs"]["short"]["pcm"],
        "after": {k: res["healthz_after"].get(k) for k in
                  ("requests", "uptime_s", "torch_peak_alloc_mb", "gpu_alloc_mb")},
    }, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
