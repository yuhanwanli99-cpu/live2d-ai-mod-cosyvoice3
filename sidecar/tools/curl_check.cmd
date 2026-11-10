@echo off
rem Acceptance: curl POST /v1/audio/speech -> raw PCM -> wav -> duration/peak check.
rem ASCII only (cmd.exe reads .cmd as OEM codepage). The Chinese body lives in tools\curl_body.json.
setlocal
set "DIR=%~dp0.."
set "PY=%DIR%\.venv\Scripts\python.exe"
set "BASE=http://127.0.0.1:8011"
if not exist "%DIR%\out" mkdir "%DIR%\out"

echo == 1) healthz ==
curl -sS "%BASE%/healthz"
echo.
echo == 2) models ==
curl -sS -o NUL -w "GET /v1/models -> %%{http_code} (%%{time_total}s)\n" "%BASE%/v1/models"

echo == 3) PCM: POST /v1/audio/speech (body from tools\curl_body.json) ==
curl -sS -X POST "%BASE%/v1/audio/speech" -H "Content-Type: application/json" --data-binary "@%DIR%\tools\curl_body.json" -D "%DIR%\out\curl-pcm.headers" -o "%DIR%\out\curl-pcm.raw" -w "HTTP %%{http_code}  wall %%{time_total}s  bytes %%{size_download}\n"
findstr /I /C:"x-sample-rate" /C:"x-channels" /C:"x-format" /C:"x-audio-seconds" /C:"x-gen-ms" /C:"x-rtf" "%DIR%\out\curl-pcm.headers"

echo == 4) raw -> wav ==
"%PY%" "%DIR%\tools\raw_to_wav.py" "%DIR%\out\curl-pcm.raw" "%DIR%\out\curl-pcm.wav" --rate 44100 --channels 1
"%PY%" "%DIR%\tools\wav_info.py" "%DIR%\out\curl-pcm.wav"

echo == 5) error paths ==
curl -sS -o NUL -w "empty input -> %%{http_code}\n" -X POST "%BASE%/v1/audio/speech" -H "Content-Type: application/json" -d "{\"input\":\"\",\"voice\":\"dafeiyu\"}"
curl -sS -o NUL -w "bad voice   -> %%{http_code}\n" -X POST "%BASE%/v1/audio/speech" -H "Content-Type: application/json" -d "{\"input\":\"hi\",\"voice\":\"nobody\"}"
curl -sS -o NUL -w "bad format  -> %%{http_code}\n" -X POST "%BASE%/v1/audio/speech" -H "Content-Type: application/json" -d "{\"input\":\"hi\",\"voice\":\"dafeiyu\",\"response_format\":\"mp3\"}"
endlocal
