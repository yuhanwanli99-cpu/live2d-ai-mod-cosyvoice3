#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""打印 wav 的关键参数 + 电平（验收用）。用法: wav_info.py <file.wav>"""
import sys
import wave

import numpy as np


def main():
    path = sys.argv[1]
    with wave.open(path, "rb") as w:
        ch, sw, sr, n = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
        raw = w.readframes(n)
    a = np.frombuffer(raw, dtype="<i2" if sw == 2 else "<i4")
    peak = int(np.abs(a.astype("int64")).max()) if a.size else 0
    rms = float(np.sqrt(np.mean(a.astype("float64") ** 2))) if a.size else 0.0
    print(f"{path}: {n} frames {n / sr:.3f}s  {sr}Hz  ch={ch}  sw={sw * 8}bit  "
          f"peak={peak} ({20 * np.log10(max(peak, 1) / 32768.0):.1f} dBFS)  "
          f"rms={rms:.1f} ({20 * np.log10(max(rms, 1e-9) / 32768.0):.1f} dBFS)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
