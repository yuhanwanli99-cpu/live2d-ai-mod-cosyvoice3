@echo off
rem CosyVoice3 TTS sidecar - start (Windows native, WSL not required).
rem Idempotent: already running -> prints /healthz and exits 0.
rem NOTE: keep this file pure ASCII. cmd.exe reads .cmd as the OEM codepage (GBK on zh-CN),
rem so UTF-8 comments would be mangled into a bogus command. All messages come from start.ps1.
setlocal
set "DIR=%~dp0"
if not "%COSYVOICE_DIR%"=="" set "DIR=%COSYVOICE_DIR%\"
pwsh -NoProfile -ExecutionPolicy Bypass -File "%DIR%start.ps1" %*
set RC=%ERRORLEVEL%
endlocal & exit /b %RC%
