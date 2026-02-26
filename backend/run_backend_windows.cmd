@echo off
setlocal

set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%"

echo [INFO] Starting cm-api-rs backend...
echo [INFO] Directory: %CD%

where cargo >nul 2>&1
if errorlevel 1 (
  echo [ERROR] cargo not found in PATH.
  exit /b 1
)

cargo run
set "EXIT_CODE=%ERRORLEVEL%"

if not "%EXIT_CODE%"=="0" (
  echo [ERROR] backend exited with code %EXIT_CODE%.
  exit /b %EXIT_CODE%
)

endlocal
