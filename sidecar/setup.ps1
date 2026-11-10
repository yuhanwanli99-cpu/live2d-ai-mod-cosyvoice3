#requires -Version 7.0
<#
.SYNOPSIS
  CosyVoice3 TTS sidecar —— Windows 原生一键搭建（不依赖 WSL）。

.DESCRIPTION
  1) uv 安装 Python 3.10；2) 在 <Dir>\.venv 建虚拟环境；
  3) 装 torch / torchaudio cu128（RTX 50 系 sm_120 必需）；
  4) 装 requirements-win.txt（fastapi/uvicorn + CosyVoice3 推理链依赖）；
  5) CosyVoice 源码（git clone --recursive）与模型目录就位；
  6) 跑 tools\doctor.py 自检（CUDA / import 链 / 模型文件 / 参考音频）。

.PARAMETER CopyModel
  默认用目录联接（junction）指向已有模型，省 5GB 磁盘；加 -CopyModel 则真复制。

.EXAMPLE
  pwsh -File setup.ps1
#>
[CmdletBinding()]
param(
    [string]$Dir = 'D:\live2dai\tts-cosyvoice3',
    [string]$RepoPath = 'D:\live2dai\CosyVoice',
    [string]$ModelSource = 'D:\live2dai\cosyvoice-cache\models\Fun-CosyVoice3-0.5B-2512',
    [switch]$CopyModel,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-Location $Dir

function Step($n, $msg) { Write-Host ""; Write-Host "=== [$n] $msg ===" -ForegroundColor Cyan }
function Ok($msg) { Write-Host "  [ok] $msg" -ForegroundColor Green }
function Warn($msg) { Write-Host "  [warn] $msg" -ForegroundColor Yellow }

# --- 0. uv ------------------------------------------------------------------
Step '0/7' '定位 uv'
$uv = (Get-Command uv -ErrorAction SilentlyContinue).Source
if (-not $uv) {
    $cand = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Filter uv.exe -Recurse -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($cand) { $uv = $cand.FullName }
}
if (-not $uv) {
    throw "找不到 uv.exe。装法：winget install --id astral-sh.uv  或 https://docs.astral.sh/uv/"
}
Ok "uv = $uv"

# 磁盘纪律：uv 缓存放 D 盘，别塞 C 盘
if (-not $env:UV_CACHE_DIR) { $env:UV_CACHE_DIR = Join-Path $Dir '.uv-cache' }
New-Item -ItemType Directory -Force $env:UV_CACHE_DIR | Out-Null
Ok "UV_CACHE_DIR = $env:UV_CACHE_DIR"

# --- 1. Python 3.10 ---------------------------------------------------------
Step '1/7' '安装 Python 3.10（uv 托管）'
& $uv python install 3.10
if ($LASTEXITCODE -ne 0) { throw "uv python install 3.10 失败" }
Ok "python 3.10 ready"

# --- 2. venv ----------------------------------------------------------------
Step '2/7' "建虚拟环境 $Dir\.venv"
$py = Join-Path $Dir '.venv\Scripts\python.exe'
if ((Test-Path $py) -and -not $Force) {
    Ok 'venv 已存在（-Force 可重建）'
} else {
    if (Test-Path (Join-Path $Dir '.venv')) { Remove-Item -Recurse -Force (Join-Path $Dir '.venv') }
    & $uv venv --python 3.10 (Join-Path $Dir '.venv')
    if ($LASTEXITCODE -ne 0) { throw 'uv venv 失败' }
    Ok "venv = $py"
}

# --- 3. torch cu128 ---------------------------------------------------------
Step '3/7' '安装 torch / torchaudio（cu128，RTX 50 系必需）'
& $uv pip install --python $py torch torchaudio --index-url https://download.pytorch.org/whl/cu128
if ($LASTEXITCODE -ne 0) { throw 'torch cu128 安装失败' }
& $py -c "import torch; assert torch.cuda.is_available(), 'CUDA not available'; print('torch', torch.__version__, '|', torch.cuda.get_device_name(0))"
if ($LASTEXITCODE -ne 0) { throw 'torch 装好了但 CUDA 不可用：先查 NVIDIA 驱动（>= 572）' }
Ok 'torch cu128 + CUDA ok'

# --- 4. 其它依赖 ------------------------------------------------------------
Step '4/7' '安装 requirements-win.txt'
$pipArgs = @('pip', 'install', '--python', $py, '-r', (Join-Path $Dir 'requirements-win.txt'))
if ($env:COSYVOICE_PIP_INDEX) { $pipArgs += @('-i', $env:COSYVOICE_PIP_INDEX) }
& $uv @pipArgs
if ($LASTEXITCODE -ne 0) { throw 'requirements-win.txt 安装失败' }
Ok 'deps ok'

# --- 5. CosyVoice 源码 ------------------------------------------------------
Step '5/7' "CosyVoice 源码 $RepoPath"
if (Test-Path (Join-Path $RepoPath 'cosyvoice\cli\cosyvoice.py')) {
    Ok '源码已存在'
} else {
    git clone --recursive https://github.com/FunAudioLLM/CosyVoice $RepoPath
    if ($LASTEXITCODE -ne 0) { throw 'git clone cosyvoice 失败' }
    Ok '源码已 clone'
}
if (-not (Test-Path (Join-Path $RepoPath 'third_party\Matcha-TTS\matcha'))) {
    git -C $RepoPath submodule update --init --recursive
    if ($LASTEXITCODE -ne 0) { throw 'git submodule 失败' }
}
Ok 'Matcha-TTS 子模块就位（server.py 用 sys.path 引入，不 pip install）'

# --- 6. 模型目录 ------------------------------------------------------------
Step '6/7' "模型目录 $Dir\model"
$need = @('llm.pt', 'flow.pt', 'hift.pt', 'campplus.onnx', 'speech_tokenizer_v3.onnx', 'cosyvoice3.yaml')
$modelLink = Join-Path $Dir 'model'
$haveAll = { param($p) ($need | Where-Object { -not (Test-Path (Join-Path $p $_)) }).Count -eq 0 }
if ((Test-Path $modelLink) -and (& $haveAll $modelLink)) {
    Ok 'model 已就位'
} else {
    if (-not (Test-Path $ModelSource)) {
        throw "找不到模型源 $ModelSource。先按 spike 的 scripts/download_model.py 下载，或 -ModelSource 指定。"
    }
    if (Test-Path $modelLink) { Remove-Item -Recurse -Force $modelLink }
    if ($CopyModel) {
        Copy-Item -Recurse $ModelSource $modelLink
        Ok '模型已复制（5GB）'
    } else {
        New-Item -ItemType Junction -Path $modelLink -Target $ModelSource | Out-Null
        Ok "已建目录联接：$modelLink -> $ModelSource"
    }
    if (-not (& $haveAll $modelLink)) { throw '模型目录缺少必需文件' }
}

# --- 7. 自检 ----------------------------------------------------------------
Step '7/7' 'doctor 自检（CUDA / import 链 / 模型文件 / 参考音频）'
& $py (Join-Path $Dir 'tools\doctor.py')
if ($LASTEXITCODE -ne 0) { throw 'doctor 自检失败，看上面的报错' }

Write-Host ""
Write-Host '=== 搭建完成 ===' -ForegroundColor Green
Write-Host "  启动:  $Dir\start.cmd"
Write-Host "  状态:  $Dir\status.cmd"
Write-Host "  停止:  $Dir\stop.cmd"
Write-Host "  验收:  .venv\Scripts\python.exe tools\accept.py"
