@echo off
rem CosyVoice3 TTS sidecar - stop (Windows native). Idempotent; ASCII only (see start.cmd).
setlocal
set "DIR=%~dp0"
if not "%COSYVOICE_DIR%"=="" set "DIR=%COSYVOICE_DIR%\"
pwsh -NoProfile -ExecutionPolicy Bypass -File "%DIR%stop.ps1" %*
endlocal & exit /b %ERRORLEVEL%
