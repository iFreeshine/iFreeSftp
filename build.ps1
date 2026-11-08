# iFreeSftp (Rust edition) build script
# Usage:
#   .\build.ps1              # Debug build
#   .\build.ps1 build        # Debug build
#   .\build.ps1 test         # cargo test (source mode)
#   .\build.ps1 run          # Build and run (Debug)
#   .\build.ps1 release      # Version checks, then Release build -> release\ifreeSftp-<ver>-win64.exe
#                            #   (source-verification build only, NOT a release artifact)
#   .\build.ps1 clean        # Clean all build outputs
#
# Note: the `official` cargo feature (offline registration verification) requires the
# private closed-source ifree-core module and is intentionally NOT exposed here.
# Release artifacts (Win64 zip / Microsoft Store MSIX / Linux tar.gz) are release
# builds (with the official feature) produced on the maintainer machine (private repo)
# and uploaded manually - CI never produces release builds (same rule as the C# edition).

param(
    [ValidateSet('build', 'test', 'run', 'release', 'clean')]
    [string]$Action = 'build'
)

$ErrorActionPreference = 'Stop'

$RepoRoot = $PSScriptRoot

function Get-AppVersion {
    $cargo = Join-Path $RepoRoot 'Cargo.toml'
    $line = Select-String -Path $cargo -Pattern '^version = "(\d+\.\d+\.\d+)"' -SimpleMatch:$false | Select-Object -First 1
    if (-not $line) { throw "cannot parse workspace version from $cargo" }
    return $line.Matches[0].Groups[1].Value
}

function Invoke-Build {
    Write-Host '==> cargo build (Debug, source mode)' -ForegroundColor Cyan
    & cargo build --offline
    if ($LASTEXITCODE -ne 0) { throw "build failed, exit code $LASTEXITCODE" }
    Write-Host '==> Build OK' -ForegroundColor Green
}

function Invoke-Test {
    Write-Host '==> cargo test (source mode)' -ForegroundColor Cyan
    & cargo test --offline
    if ($LASTEXITCODE -ne 0) { throw "test failed, exit code $LASTEXITCODE" }
    Write-Host '==> Test OK' -ForegroundColor Green
}

function Invoke-Run {
    Write-Host '==> Build and run (Debug)' -ForegroundColor Cyan
    & cargo run --offline
    if ($LASTEXITCODE -ne 0) { throw "run failed, exit code $LASTEXITCODE" }
}

function Invoke-Release {
    # 发布前防漏校验: Cargo.toml / README 徽章 / CHANGELOG 版本号必须一致
    & (Join-Path $PSScriptRoot 'tools\check-version.ps1')
    if ($LASTEXITCODE -ne 0) { throw 'check-version 失败, 已阻止发布 - 先把所有版本声明点同步后再发' }

    $ver = Get-AppVersion
    $outDir = Join-Path $RepoRoot 'release'
    if (Test-Path $outDir) { Remove-Item $outDir -Recurse -Force }
    Write-Host "==> cargo build (Release, lto=thin, strip) -> $outDir" -ForegroundColor Cyan
    & cargo build --release --offline
    if ($LASTEXITCODE -ne 0) { throw "release build failed, exit code $LASTEXITCODE" }
    $exe = Join-Path $RepoRoot 'target\release\ifreeSftp.exe'
    if (-not (Test-Path $exe)) { throw "output not found: $exe" }
    Copy-Item $exe (Join-Path $outDir "ifreeSftp-$ver-win64.exe") -Force
    Write-Host "==> Release OK: release\ifreeSftp-$ver-win64.exe" -ForegroundColor Green
    Write-Host '    (GitHub Release 附件请连同 zip 打包; 上传为维护者手工步骤, CI 不出正式发布构建)'
}

function Invoke-Clean {
    Write-Host '==> cargo clean + remove release' -ForegroundColor Cyan
    & cargo clean
    foreach ($dir in @((Join-Path $RepoRoot 'release'))) {
        if (Test-Path $dir) {
            Remove-Item $dir -Recurse -Force
            Write-Host "    removed $dir"
        }
    }
    Write-Host '==> Clean OK' -ForegroundColor Green
}

switch ($Action) {
    'build'   { Invoke-Build }
    'test'    { Invoke-Test }
    'run'     { Invoke-Run }
    'release' { Invoke-Release }
    'clean'   { Invoke-Clean }
}
