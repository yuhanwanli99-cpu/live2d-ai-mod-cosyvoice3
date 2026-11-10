@echo off
rem CosyVoice3 TTS sidecar - status (Windows native). ASCII only (see start.cmd).
setlocal
set "DIR=%~dp0"
if not "%COSYVOICE_DIR%"=="" set "DIR=%COSYVOICE_DIR%\"
pwsh -NoProfile -ExecutionPolicy Bypass -File "%DIR%status.ps1" %*
endlocal & exit /b %ERRORLEVEL%
