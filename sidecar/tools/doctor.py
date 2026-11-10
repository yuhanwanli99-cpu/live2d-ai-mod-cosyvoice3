#!/usr/bin/env python3
# -*- coding: utf-8 -*-
r"""Windows 原生环境自检：CUDA / 依赖 / CosyVoice 源码 / 模型文件 / 参考音频。

用法:
    .venv\Scripts\python.exe tools\doctor.py            # 只查环境 + import 链（不加载模型）
    .venv\Scripts\python.exe tools\doctor.py --load     # 再真加载模型（约 40s）
    .venv\Scripts\python.exe tools\doctor.py --synth    # 加载并合成一句（完整链路）

退出码 0 = 全部通过；非 0 = 有硬阻塞（看 [FAIL] 行）。
"""
import argparse
import json
import os
import platform
import sys
import time

if sys.platform == "win32":  # 控制台/管道统一 UTF-8，避免中文在 cp936 下乱码
    for _s in (sys.stdout, sys.stderr):
        try:
            _s.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO_CAND = [
    os.environ.get("COSYVOICE_REPO", ""),
    os.path.join(os.path.dirname(DIR), "CosyVoice"),
    "D:/live2dai/CosyVoice",
    os.path.expanduser("~/cosyvoice3/CosyVoice"),
]
MODEL_CAND = [
    os.environ.get("COSYVOICE_MODEL_DIR", ""),
    os.path.join(DIR, "model"),
    "D:/live2dai/cosyvoice-cache/models/Fun-CosyVoice3-0.5B-2512",
]


def first_dir(cands):
    for c in cands:
        if c and os.path.isdir(c):
            return os.path.abspath(c)
    return ""


REPO = first_dir(REPO_CAND)
MODEL_DIR = first_dir(MODEL_CAND)
REF_WAV = os.path.join(DIR, "ref", "prompt-16k.wav")
REF_TEXT = os.path.join(DIR, "ref", "refText.txt")

report = {"env": {}, "deps": {}, "paths": {}, "checks": [], "fail": []}


def check(name, ok, detail=""):
    report["checks"].append({"name": name, "ok": bool(ok), "detail": str(detail)})
    tag = "ok  " if ok else "FAIL"
    print(f"[{tag}] {name}{(' — ' + str(detail)) if detail else ''}")
    if not ok:
        report["fail"].append(name)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--load", action="store_true", help="真加载模型")
    ap.add_argument("--synth", action="store_true", help="加载并合成一句")
    ap.add_argument("--json", default=os.path.join(DIR, "out", "doctor.json"))
    args = ap.parse_args()

    print("== 平台 ==")
    report["env"] = {
        "python": sys.version.split()[0],
        "executable": sys.executable,
        "platform": platform.platform(),
        "machine": platform.machine(),
    }
    print(json.dumps(report["env"], ensure_ascii=False, indent=2))

    print("== torch / CUDA ==")
    try:
        import torch
        info = {
            "torch": torch.__version__,
            "cuda_available": bool(torch.cuda.is_available()),
            "torch_cuda": torch.version.cuda,
        }
        check("import torch", True, info["torch"])
        check("torch.cuda.is_available", info["cuda_available"], info["torch_cuda"])
        if info["cuda_available"]:
            props = torch.cuda.get_device_properties(0)
            info.update({
                "device": torch.cuda.get_device_name(0),
                "capability": list(torch.cuda.get_device_capability(0)),
                "total_vram_mb": round(props.total_memory / 1024 ** 2, 1),
            })
            a = torch.randn(1024, 1024, device="cuda")
            b = (a @ a).sum().item()
            check("cuda matmul 1024^2", True, f"sum={b:.1f}")
        report["env"].update(info)
    except Exception as e:
        check("torch / CUDA", False, f"{type(e).__name__}: {e}")

    print("== 依赖 ==")
    deps = ["torchaudio", "soundfile", "numpy", "onnxruntime", "transformers", "diffusers",
            "inflect", "omegaconf", "hydra", "lightning", "whisper", "fastapi", "uvicorn",
            "pydantic", "x_transformers", "regex"]
    for mod in deps:
        try:
            m = __import__(mod)
            ver = getattr(m, "__version__", "?")
            report["deps"][mod] = ver
            check(f"import {mod}", True, str(ver))
        except Exception as e:
            report["deps"][mod] = f"ERROR {type(e).__name__}: {e}"
            check(f"import {mod}", False, f"{type(e).__name__}: {e}")

    try:
        import onnxruntime
        check("onnxruntime CPU provider", "CPUExecutionProvider" in onnxruntime.get_available_providers(),
              str(onnxruntime.get_available_providers()))
    except Exception as e:
        check("onnxruntime providers", False, str(e))

    print("== 路径 ==")
    report["paths"] = {"repo": REPO, "model_dir": MODEL_DIR, "ref_wav": REF_WAV, "ref_text": REF_TEXT}
    check("CosyVoice 源码", os.path.isdir(REPO) and os.path.isfile(os.path.join(REPO, "cosyvoice", "cli", "cosyvoice.py")), REPO)
    check("Matcha-TTS 子模块", os.path.isfile(os.path.join(REPO, "third_party", "Matcha-TTS", "matcha", "models", "components", "flow_matching.py")), "")
    for name in ["llm.pt", "flow.pt", "hift.pt", "campplus.onnx", "speech_tokenizer_v3.onnx", "cosyvoice3.yaml"]:
        p = os.path.join(MODEL_DIR, name)
        check("model/" + name, os.path.isfile(p), f"{(os.path.getsize(p) / 1024 ** 2):.1f} MB" if os.path.isfile(p) else "missing")
    check("ref/prompt-16k.wav", os.path.isfile(REF_WAV), str(os.path.getsize(REF_WAV)) if os.path.isfile(REF_WAV) else "missing")
    check("ref/refText.txt", os.path.isfile(REF_TEXT), open(REF_TEXT, encoding="utf-8").read().strip() if os.path.isfile(REF_TEXT) else "missing")

    if args.load or args.synth:
        print("== 加载模型（fp32）==")
        sys.path.insert(0, REPO)
        sys.path.insert(0, os.path.join(REPO, "third_party", "Matcha-TTS"))
        sys.path.insert(0, DIR)
        try:
            import server  # 复用 server.py 的 torchaudio 垫片与加载逻辑
            t0 = time.time()
            eng = server.Engine()
            os.chdir(REPO)
            eng.load()
            check("Engine.load", True, f"{time.time() - t0:.1f}s, spk_cached={eng.spk_cached}")
            if args.synth:
                t1 = time.time()
                pcm, stats = eng.synthesize_pcm("哥哥你好呀。", speed=1.0)
                check("synthesis", len(pcm) > 0, json.dumps(stats, ensure_ascii=False))
                print(f"  synth wall={time.time() - t1:.2f}s stats={stats}")
        except Exception as e:
            import traceback
            traceback.print_exc()
            check("Engine.load", False, f"{type(e).__name__}: {e}")

    os.makedirs(os.path.join(DIR, "out"), exist_ok=True)
    with open(args.json, "w", encoding="utf-8") as f:
        json.dump(report, f, ensure_ascii=False, indent=2)
    print()
    if report["fail"]:
        print("RESULT: FAIL -> " + ", ".join(report["fail"]))
        return 1
    print("RESULT: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
