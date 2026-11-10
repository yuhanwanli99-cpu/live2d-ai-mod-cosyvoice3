#requires -Version 7.0
<#
.SYNOPSIS
  CosyVoice3 sidecar 启动（Windows 原生）。幂等：已在跑就直接打印 /healthz。
#>
[CmdletBinding()]
param(
    [string]$Dir = $PSScriptRoot,
    [int]$Port = 8011,
    [int]$WaitSeconds = 300
)
$ErrorActionPreference = 'Stop'
if (-not $Dir) { $Dir = 'D:\live2dai\tts-cosyvoice3' }
$Dir = (Resolve-Path $Dir).Path
$py = Join-Path $Dir '.venv\Scripts\python.exe'
$server = Join-Path $Dir 'server.py'
$logs = Join-Path $Dir 'logs'
$pidFile = Join-Path $logs 'server.pid'
New-Item -ItemType Directory -Force $logs | Out-Null
New-Item -ItemType Directory -Force (Join-Path $Dir 'cache') | Out-Null
New-Item -ItemType Directory -Force (Join-Path $Dir 'out') | Out-Null

function Get-Health {
    try {
        return Invoke-RestMethod -Uri "http://127.0.0.1:$Port/healthz" -TimeoutSec 2
    } catch {
        return $null
    }
}

# --- 已在跑？ ---
if (Test-Path $pidFile) {
    $oldPid = (Get-Content $pidFile -ErrorAction SilentlyContinue | Select-Object -First 1)
    if ($oldPid -and (Get-Process -Id $oldPid -ErrorAction SilentlyContinue)) {
        $h = Get-Health
        if ($h -and $h.ok) {
            Write-Host "[start] already running (pid $oldPid) on 127.0.0.1:$Port"
            $h | ConvertTo-Json -Compress
            exit 0
        }
        Write-Host "[start] pid $oldPid alive but /healthz not ready; keep waiting"
    }
}
if (Get-Health) {
    Write-Host "[start] port $Port already serving /healthz by another process; refuse to start" -ForegroundColor Yellow
    exit 3
}
if (-not (Test-Path $py)) {
    Write-Host "[start] python not found: $py — 先跑 setup.ps1" -ForegroundColor Red
    exit 2
}

# --- 启动 ---
$outLog = Join-Path $logs 'server.stdout.log'
$errLog = Join-Path $logs 'server.stderr.log'
$stamp = Get-Date -Format 'yyyy-MM-ddTHH:mm:sszzz'
Add-Content -Path (Join-Path $logs 'launch.log') -Value "[$stamp] start $py -u $server (port $Port)" -Encoding utf8

$procArgs = @{ FilePath = $py; ArgumentList = @('-u', $server); WorkingDirectory = $Dir; WindowStyle = 'Hidden'; PassThru = $true; RedirectStandardOutput = $outLog; RedirectStandardError = $errLog }
$p = Start-Process @procArgs
$p.Id | Set-Content -Path $pidFile -Encoding ascii
Write-Host "[start] pid $($p.Id); waiting for /healthz (model load ~40-90s, spk 缓存命中更快)"

$sw = [Diagnostics.Stopwatch]::StartNew()
while ($sw.Elapsed.TotalSeconds -lt $WaitSeconds) {
    $h = Get-Health
    if ($h -and $h.ok) {
        Write-Host "[start] READY after $([int]$sw.Elapsed.TotalSeconds)s" -ForegroundColor Green
        $h | ConvertTo-Json -Compress
        exit 0
    }
    if ($p.HasExited) {
        Write-Host "[start] server died during startup (exit $($p.ExitCode)); last log:" -ForegroundColor Red
        Get-Content $errLog -Tail 40 -ErrorAction SilentlyContinue
        Get-Content $outLog -Tail 20 -ErrorAction SilentlyContinue
        Remove-Item $pidFile -ErrorAction SilentlyContinue
        exit 1
    }
    Start-Sleep -Milliseconds 1000
}
Write-Host "[start] timeout after $WaitSeconds s; last log:" -ForegroundColor Red
Get-Content $errLog -Tail 40 -ErrorAction SilentlyContinue
exit 1
