#requires -Version 7.0
<#
.SYNOPSIS
  CosyVoice3 sidecar 停止（幂等；只杀本目录的 server.py）。
#>
[CmdletBinding()]
param(
    [string]$Dir = $PSScriptRoot,
    [int]$Port = 8011
)
$ErrorActionPreference = 'Stop'
if (-not $Dir) { $Dir = 'D:\live2dai\tts-cosyvoice3' }
$Dir = (Resolve-Path $Dir).Path
$pidFile = Join-Path $Dir 'logs\server.pid'
$server = Join-Path $Dir 'server.py'
$stopped = $false

function Stop-Pid2($procId) {
    if (-not $procId) { return $false }
    $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
    if (-not $p) { return $false }
    Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
    for ($i = 0; $i -lt 40; $i++) {
        if (-not (Get-Process -Id $procId -ErrorAction SilentlyContinue)) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $true
}

if (Test-Path $pidFile) {
    $oldPid = (Get-Content $pidFile -ErrorAction SilentlyContinue | Select-Object -First 1)
    if (Stop-Pid2 $oldPid) { $stopped = $true }
    Remove-Item $pidFile -ErrorAction SilentlyContinue
}

# 兜底：pid 文件丢了也按命令行匹配本目录的 server.py
$procs = Get-CimInstance Win32_Process -Filter "Name = 'python.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine -like "*$server*" }
foreach ($pr in $procs) {
    if (Stop-Pid2 $pr.ProcessId) { $stopped = $true }
}

# 等端口真正释放
for ($i = 0; $i -lt 40; $i++) {
    $conn = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
    if (-not $conn) { break }
    Start-Sleep -Milliseconds 250
}

if ($stopped) {
    Write-Host "[stop] stopped (port $Port)"
} else {
    Write-Host "[stop] not running"
}
