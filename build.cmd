@echo off
rem ============================================================
rem  iFreeSftp (Rust edition) dev loop: stop old instance + build + launch
rem  (release publish: powershell -File build.ps1 release)
rem  NOTE: keep this file ASCII-only. chcp 65001 inside a batch
rem  with multi-byte text corrupts cmd's script read offsets.
rem ============================================================
setlocal
title iFreeSftp Build

rem windows-gnu toolchain: cargo + mingw binutils (dlltool) must be on PATH
set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if not exist "%CARGO%" set "CARGO=cargo"

echo [1/3] Stopping old instance...
taskkill /IM ifreeSftp.exe /F >nul 2>&1
rem ping-based delay works even when stdin is redirected (timeout does not)
ping -n 3 127.0.0.1 >nul

rem NOTE: target/ is NOT deleted here. Incremental build relies on it.
rem For a real clean use: powershell -File build.ps1 clean

echo [2/3] Building (Debug, source mode)...
"%CARGO%" build --offline
if errorlevel 1 (
    echo.
    echo Build FAILED!
    pause
    exit /b 1
)

echo [3/3] Launching app...
start "" "%~dp0target\debug\ifreeSftp.exe"
echo Done.
endlocal
