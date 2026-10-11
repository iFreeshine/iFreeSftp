@echo off
rem ============================================================
rem  iFreeSftp dev loop: clean cache + stop old instance + build + launch
rem  (release publish: powershell -File build.ps1 release)
rem  NOTE: keep this file ASCII-only. chcp 65001 inside a batch
rem  with multi-byte text corrupts cmd's script read offsets.
rem ============================================================
setlocal
title iFreeSftp Build

set "DOTNET=%USERPROFILE%\.dotnet\dotnet.exe"
if not exist "%DOTNET%" set "DOTNET=dotnet"
set "PROJ=%~dp0src\iFreeSftp\iFreeSftp.csproj"
set "EXE=%~dp0src\iFreeSftp\bin\Debug\net10.0-windows10.0.26100.0\win-x64\iFreeSftp.exe"

echo [1/3] Stopping old instance...
taskkill /IM iFreeSftp.exe /F >nul 2>&1
rem ping-based delay works even when stdin is redirected (timeout does not)
ping -n 3 127.0.0.1 >nul

rem NOTE: bin/obj are NOT deleted here. Incremental build relies on them;
rem deleting causes full restore + full rebuild (minutes, flaky XAML compiler).
rem For a real clean use: powershell -File build.ps1 clean

echo [2/3] Building (Debug, self-contained)...
"%DOTNET%" build "%PROJ%" -c Debug -nologo
if errorlevel 1 (
    echo.
    echo Build FAILED!
    pause
    exit /b 1
)

echo [3/3] Launching app...
if not exist "%EXE%" (
    echo Output not found: %EXE%
    pause
    exit /b 1
)
start "" "%EXE%"
echo Done.
endlocal
