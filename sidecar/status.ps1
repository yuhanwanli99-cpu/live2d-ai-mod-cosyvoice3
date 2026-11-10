#requires -Version 7.0
<#
.SYNOPSIS
  CosyVoice3 sidecar 状态：healthz + pid + 显存 + 最近请求。
#>
[CmdletBinding()]
param(
    [string]$Dir = $PSScriptRoot,
    [int]$Port = 8011
)
if (-not $Dir) { $Dir = 'D:\live2dai\tts-cosyvoice3' }
$Dir = (Resolve-Path $Dir).Path

Write-Host "== sidecar dir: $Dir =="
Write-Host "== port: 127.0.0.1:$Port =="
$h = $null
try { $h = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/healthz" -TimeoutSec 3 } catch {}
if ($h -and $h.ok) {
    Write-Host '== healthz: OK ==' -ForegroundColor Green
    $h | ConvertTo-Json -Depth 4
} else {
    Write-Host '== healthz: DOWN (not running or still loading) ==' -ForegroundColor Yellow
}

$pidFile = Join-Path $Dir 'logs\server.pid'
if (Test-Path $pidFile) {
    $p = Get-Content $pidFile | Select-Object -First 1
    if (Get-Process -Id $p -ErrorAction SilentlyContinue) { Write-Host "== pid: $p (alive) ==" }
    else { Write-Host '== pid: stale pid file ==' }
} else {
    Write-Host '== pid: none =='
}

$smi = Get-Command nvidia-smi -ErrorAction SilentlyContinue
if ($smi) {
    Write-Host '== gpu =='
    & $smi.Source --query-gpu=name,memory.used,memory.total --format=csv,noheader
}

Write-Host '== last requests (logs/requests.jsonl) =='
$reqLog = Join-Path $Dir 'logs\requests.jsonl'
if (Test-Path $reqLog) { Get-Content $reqLog -Tail 3 } else { Write-Host '(none)' }
