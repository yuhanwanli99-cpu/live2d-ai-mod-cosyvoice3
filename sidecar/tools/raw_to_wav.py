#!/usr/bin/env python3
"""raw s16le -> RIFF wav（验收工具：证明服务返回的是能直接播的 PCM）。

用法: raw_to_wav.py <in.raw> <out.wav> [--rate 44100] [--channels 1]
"""
import argparse
import wave


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("raw")
    ap.add_argument("wav")
    ap.add_argument("--rate", type=int, default=44100)
    ap.add_argument("--channels", type=int, default=1)
    a = ap.parse_args()
    with open(a.raw, "rb") as f:
        data = f.read()
    assert len(data) % (2 * a.channels) == 0, f"raw size {len(data)} not s16le-aligned"
    with wave.open(a.wav, "wb") as w:
        w.setnchannels(a.channels)
        w.setsampwidth(2)
        w.setframerate(a.rate)
        w.writeframes(data)
    frames = len(data) // (2 * a.channels)
    print(f"ok {a.wav}: {frames} frames {frames / a.rate:.3f}s @{a.rate}Hz ch={a.channels} bytes={len(data)}")


if __name__ == "__main__":
    main()
