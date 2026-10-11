# iFreeSftp build script
# Usage:
#   .\build.ps1              # Debug build
#   .\build.ps1 build        # Debug build
#   .\build.ps1 release      # Publish single-file exe to release\win-x64\ (all deps bundled, extracted to temp at first launch)
#   .\build.ps1 run          # Build and run (Debug)
#   .\build.ps1 clean        # Clean all build outputs
param(
    [ValidateSet('build', 'release', 'run', 'clean')]
    [string]$Action = 'build',

    # 核心实现模式 (开源闭源分层, 见技术文档):
    #   stub     = 社区构建, 引用 Core.Stub 降级实现 (默认, 仓库可完整编译)
    #   official = 官方构建, 引用 core\iFreeSftp.Core.dll (私有仓库产物, 构建前须已就位)
    [ValidateSet('stub', 'official')]
    [string]$Core = 'stub'
)

$ErrorActionPreference = 'Stop'

# Locate dotnet.exe: prefer user-profile install, fallback to PATH
$Dotnet = Join-Path $env:USERPROFILE '.dotnet\dotnet.exe'
if (-not (Test-Path $Dotnet)) {
    $Dotnet = 'dotnet'
}

$RepoRoot = $PSScriptRoot
$Project = Join-Path $RepoRoot 'src\iFreeSftp\iFreeSftp.csproj'

function Invoke-Build {
    # 构建前清理残留（实测两类锁）: 正在运行的实例锁住 bin 输出; 崩溃残留的
    # 编译服务进程 (VBCSCompiler / XAML 编译器) 锁住 obj 中间 DLL, 均导致
    # CS2012/MSB3027。build-server shutdown 先温和结束 dotnet 复用的编译节点。
    foreach ($name in @('iFreeSftp', 'Microsoft.UI.Xaml.Markup.Compiler', 'VBCSCompiler')) {
        Stop-Process -Name $name -Force -ErrorAction SilentlyContinue
    }
    & $Dotnet build-server shutdown *> $null
    Start-Sleep -Milliseconds 500
    Write-Host '==> dotnet build (Debug)' -ForegroundColor Cyan
    & $Dotnet build $Project -c Debug -p:CoreImplementation=$Core
    if ($LASTEXITCODE -ne 0) { throw "build failed, exit code $LASTEXITCODE" }
    Write-Host "==> Build OK (Core=$Core)" -ForegroundColor Green
}

function Invoke-Release {
    $outDir = Join-Path $RepoRoot 'release\win-x64'
    Write-Host "==> dotnet publish (Release, self-contained, single-file) -> $outDir" -ForegroundColor Cyan
    # Official single-file config for unpackaged self-contained WinUI 3 (WinAppSDK >= 1.5):
    # WindowsPackageType=None + PublishSingleFile + IncludeAllContentForSelfExtract.
    # All dependencies (native runtime, pri, assets, winmd) are bundled into one exe and
    # extracted to %TEMP%\.net\ at first launch.
    # Stop instances running from the publish dir, then wipe it: mixing a single-file exe
    # with leftovers of the old folder-mode payload is not allowed.
    # Stop ALL running instances: a deleted-but-running exe reports Path=null and
    # would survive a path-filtered match, causing stale-UI confusion after publish.
    Get-Process iFreeSftp -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    if (Test-Path $outDir) { Remove-Item $outDir -Recurse -Force }
    if ($Core -eq 'official' -and -not (Test-Path (Join-Path $RepoRoot 'core\iFreeSftp.Core.dll'))) {
        throw "official 模式缺少 core\iFreeSftp.Core.dll - 请先从私有仓库 CI 拷入, 或改用 -Core stub"
    }
    & $Dotnet publish $Project -c Release -r win-x64 -p:Platform=x64 -p:PublishDir="$outDir\" `
        -p:PublishSingleFile=true -p:IncludeAllContentForSelfExtract=true -p:WindowsPackageType=None `
        -p:CoreImplementation=$Core
    if ($LASTEXITCODE -ne 0) { throw "publish failed, exit code $LASTEXITCODE" }
    Write-Host "==> Single-file publish OK (Core=$Core): $outDir\iFreeSftp.exe" -ForegroundColor Green
}

function Invoke-Run {
    Write-Host '==> Build and run (Debug)' -ForegroundColor Cyan
    & $Dotnet build $Project -c Debug
    if ($LASTEXITCODE -ne 0) { throw "build failed, exit code $LASTEXITCODE" }
    $exe = Join-Path $RepoRoot 'src\iFreeSftp\bin\Debug\net10.0-windows10.0.26100.0\win-x64\iFreeSftp.exe'
    if (-not (Test-Path $exe)) { throw "output not found: $exe" }
    Start-Process -FilePath $exe
    Write-Host "==> Launched: $exe" -ForegroundColor Green
}

function Invoke-Clean {
    Write-Host '==> dotnet clean + remove bin/obj/release' -ForegroundColor Cyan
    & $Dotnet clean $Project -c Debug
    & $Dotnet clean $Project -c Release
    foreach ($dir in @(
        (Join-Path $RepoRoot 'src\iFreeSftp\bin'),
        (Join-Path $RepoRoot 'src\iFreeSftp\obj'),
        (Join-Path $RepoRoot 'release'))) {
        if (Test-Path $dir) {
            Remove-Item $dir -Recurse -Force
            Write-Host "    removed $dir"
        }
    }
    Write-Host '==> Clean OK' -ForegroundColor Green
}

switch ($Action) {
    'build'   { Invoke-Build }
    'release' { Invoke-Release }
    'run'     { Invoke-Run }
    'clean'   { Invoke-Clean }
}
