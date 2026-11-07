# iFreeSftp (Rust edition) MSIX packaging script (Microsoft Store submission / local test install)
# Usage:
#   .\tools\make-msix.ps1                 # community build, unsigned msix (Store accepts unsigned, signs on submission)
#   .\tools\make-msix.ps1 -Official       # official build (requires private ifree-core on THIS machine; the exe comes from the main repo)
#   .\tools\make-msix.ps1 -Sign           # + self-signed cert for local Add-AppxPackage testing
#   .\tools\make-msix.ps1 -IdentityName 'XXXXX' -Publisher 'CN=XXXX...'   # real Partner Center identity override
#
# 与 C# 版 make-msix.ps1 的差异:
#   - 构建: dotnet publish -> cargo build --release (Rust 单 exe, loose 布局天然满足 MSIX)
#   - PRI: Rust 无资源系统 -> makepri new 从 payload 现场生成 resources.pri
#           (C# 版复用 app PRI; 少图标=开始菜单灰块的教训同为红线, 见 payload 校验)
#   - 图标资产: 沿用 C# 项目 Assets (品牌统一), 存放 tools\msix\assets\

param(
    # 官方构建: 需本机持有闭源 ifree-core (主仓), 输出含注册验签入口; 默认社区构建
    [switch]$Official,

    # 包版本, 默认解析公开仓 Cargo.toml [workspace.package] version (0.9.0 -> 0.9.0.0)
    [string]$Version,

    # Partner Center "App identity" 页真实值 (C# 版 0.8.3 提交时校验拿到的, 同一应用沿用)
    [string]$IdentityName = 'iFrees.iFreeSftp',
    [string]$Publisher = 'CN=029857DD-BD5B-48BB-BC40-43193DDB2DE9',

    # Self-sign the msix with a throwaway cert for local Add-AppxPackage smoke testing
    [switch]$Sign
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot   # github\ifreeSftp (tools\ 的上一级)
$MainRepo = 'E:\aiProject\iFreeSftpRust'                            # 主仓 (official 构建源)
$WorkDir = Join-Path $RepoRoot 'release\msix'
$PayloadDir = Join-Path $WorkDir 'payload'

# --- Version: default from Cargo.toml ---
if (-not $Version) {
    $line = Select-String -Path (Join-Path $RepoRoot 'Cargo.toml') -Pattern '^version = "(\d+\.\d+\.\d+)"' | Select-Object -First 1
    if (-not $line) { throw "Cannot parse workspace version from Cargo.toml - pass -Version explicitly" }
    $Version = $line.Matches[0].Groups[1].Value
}
if ($Version -notmatch '^\d+\.\d+\.\d+\.\d+$') { $Version = "$Version.0" }

# --- 防漏校验: 各声明点版本号一致 (README/CHANGELOG; 官网项缺失自动 SKIP) ---
& (Join-Path $PSScriptRoot 'check-version.ps1')
if ($LASTEXITCODE -ne 0) { throw 'check-version failed - aborting MSIX packaging' }

# --- Build the exe ---
# PS 5.1: native stderr 行经 2>&1 会变成 ErrorRecord, 在 $ErrorActionPreference='Stop'
# 下误抛 NativeCommandError —— 原生工具调用段统一局部降级为 Continue, 用 $LASTEXITCODE 判定。
$eap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
Get-Process ifreeSftp -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800
if ($Official) {
    $coreDir = Join-Path $MainRepo 'ifree-core\src'
    if (-not (Test-Path $coreDir)) { throw "official build requires the private ifree-core at $coreDir - or drop -Official" }
    Write-Host "==> cargo build --release --features official (main repo, official mode)" -ForegroundColor Cyan
    Push-Location (Join-Path $MainRepo 'ifreeSftp')
    try {
        $out = & cargo build --release --offline --features official 2>&1
        $out | ForEach-Object { "$_" }
        if ($LASTEXITCODE -ne 0) { throw "official build failed, exit code $LASTEXITCODE" }
    } finally { Pop-Location }
    $exeSrc = Join-Path $MainRepo 'ifreeSftp\target\release\ifreeSftp.exe'
}
else {
    Write-Host "==> cargo build --release (community mode)" -ForegroundColor Cyan
    Push-Location $RepoRoot
    try {
        $out = & cargo build --release --offline 2>&1
        $out | ForEach-Object { "$_" }
        if ($LASTEXITCODE -ne 0) { throw "build failed, exit code $LASTEXITCODE" }
    } finally { Pop-Location }
    $exeSrc = Join-Path $RepoRoot 'target\release\ifreeSftp.exe'
}
$ErrorActionPreference = $eap
if (-not (Test-Path $exeSrc)) { throw "exe not found: $exeSrc" }

# --- Payload layout (loose files, single exe + Assets + manifest) ---
if (Test-Path $WorkDir) { Remove-Item $WorkDir -Recurse -Force }
New-Item -ItemType Directory -Force $PayloadDir | Out-Null
Copy-Item $exeSrc (Join-Path $PayloadDir 'ifreeSftp.exe') -Force
Write-Host ("==> exe: {0} ({1:N1} MB, {2})" -f (Split-Path -Leaf $exeSrc), ((Get-Item $exeSrc).Length / 1MB), ($(if ($Official) { 'official' } else { 'community' }))) -ForegroundColor DarkGray

# --- Tile/splash/icon assets: copy the WHOLE store-assets folder, not a subset.
#     0.8.5 曾漏 Square44x44Logo.targetsize-48_altform-unplated.png 导致开始菜单
#     快捷方式灰块 (Store 页图标在 Partner Center 单独上传不受影响) —— 一张不落。
$assetsSrc = Join-Path $PSScriptRoot 'msix\assets'
if (-not (Test-Path $assetsSrc)) { throw "store assets folder missing: $assetsSrc" }
New-Item -ItemType Directory -Force (Join-Path $PayloadDir 'Assets') | Out-Null
Get-ChildItem $assetsSrc -File | ForEach-Object {
    Copy-Item $_.FullName (Join-Path $PayloadDir "Assets\$($_.Name)") -Force
}

# --- assets 完整性门: manifest/壳引用的每个图标文件必须物理存在 ---
$required = @(
    'StoreLogo.png',
    'Square150x150Logo.scale-200.png',
    'Square44x44Logo.scale-200.png',
    'Square44x44Logo.targetsize-24_altform-unplated.png',
    'Square44x44Logo.targetsize-48_altform-lightunplated.png',
    'Square44x44Logo.targetsize-48_altform-unplated.png',
    'Wide310x150Logo.scale-200.png',
    'SplashScreen.scale-200.png',
    'LockScreenLogo.scale-200.png',
    'AppIcon.ico'
)
$missing = @($required | Where-Object { -not (Test-Path (Join-Path $PayloadDir "Assets\$_")) })
if ($missing.Count -gt 0) {
    throw "required icon assets missing from payload (would render as broken icon):`n  $($missing -join "`n  ")"
}
Write-Host "==> icon assets OK ($($required.Count) files present)" -ForegroundColor DarkGray

# --- Write injected AppxManifest ---
$manifestSrc = Join-Path $PSScriptRoot 'msix\AppxManifest.xml'
$manifest = Get-Content $manifestSrc -Raw -Encoding UTF8
$manifest = $manifest.Replace('__IDENTITY_NAME__', $IdentityName).Replace('__IDENTITY_PUBLISHER__', $Publisher).Replace('__VERSION__', $Version)
$manifestDst = Join-Path $PayloadDir 'AppxManifest.xml'
[System.IO.File]::WriteAllText($manifestDst, $manifest, (New-Object System.Text.UTF8Encoding($false)))

# --- Locate SDK tools (makepri / MakeAppx / signtool) ---
function Find-SdkTool([string]$name) {
    # Green setup: D:\dev\winsdk (manually extracted Microsoft.Windows.SDK.BuildTools nupkg)
    $green = 'D:\dev\winsdk'
    if (Test-Path $green) {
        $tool = Get-ChildItem (Join-Path $green "bin\*\x64\$name") -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($tool) { return $tool.FullName }
    }
    # NuGet cache of Microsoft.Windows.SDK.BuildTools (no Windows SDK install needed)
    $nuget = Join-Path $env:USERPROFILE '.nuget\packages\microsoft.windows.sdk.buildtools'
    if (Test-Path $nuget) {
        $tool = Get-ChildItem $nuget -Directory | Sort-Object Name -Descending |
            ForEach-Object { Get-ChildItem (Join-Path $_.FullName "bin\*\x64\$name") -ErrorAction SilentlyContinue } |
            Select-Object -First 1
        if ($tool) { return $tool.FullName }
    }
    # Classic Windows SDK installation
    $kitRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path $kitRoot) {
        $tool = Get-ChildItem $kitRoot -Directory | Where-Object { $_.Name -match '^10\.' } |
            Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName "x64\$name" } |
            Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($tool) { return $tool }
    }
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw "$name not found - install the Windows SDK or restore the Microsoft.Windows.SDK.BuildTools nupkg"
}
$makePri = Find-SdkTool 'makepri.exe'
$makeAppx = Find-SdkTool 'MakeAppx.exe'

# --- Generate resources.pri (Rust 版无资源系统, 从 payload 现场生成; 磁贴/启动画面解析依赖它) ---
Write-Host "==> makepri new -> resources.pri" -ForegroundColor Cyan
$eap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$priconfig = Join-Path $WorkDir 'priconfig.xml'
& $makePri createconfig /cf $priconfig /dq en-US /o 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "makepri createconfig failed, exit code $LASTEXITCODE" }
& $makePri new /pr $PayloadDir /cf $priconfig /of (Join-Path $PayloadDir 'resources.pri') /mn $manifestDst /o 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "makepri new failed, exit code $LASTEXITCODE" }
$ErrorActionPreference = $eap
$pri = Get-Item (Join-Path $PayloadDir 'resources.pri')
if ($pri.Length -lt 1KB) { throw "resources.pri is suspiciously small ($($pri.Length) bytes) - makepri step broken" }
# 中间文件不打进包
Remove-Item $priconfig -Force -ErrorAction SilentlyContinue

# --- Pack ---
$msixPath = Join-Path $RepoRoot "release\iFreeSftp-$Version-x64.msix"
Write-Host "==> MakeAppx pack -> $msixPath" -ForegroundColor Cyan
$ErrorActionPreference = 'Continue'
& $makeAppx pack /o /d $PayloadDir /p $msixPath 2>&1 | Out-Null
$packExit = $LASTEXITCODE
$ErrorActionPreference = $eap
if ($packExit -ne 0) { throw "MakeAppx pack failed, exit code $packExit" }

# --- Optional self-sign for local test install ---
if ($Sign) {
    $signtool = Find-SdkTool 'signtool.exe'
    Write-Host "==> Creating self-signed cert ($Publisher)" -ForegroundColor Cyan
    $cert = New-SelfSignedCertificate -Type Custom -Subject $Publisher `
        -KeyUsage DigitalSignature -FriendlyName "iFreeSftp test signing" `
        -CertStoreLocation 'Cert:\CurrentUser\My' `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
    $pfxPath = Join-Path $WorkDir 'test-signing.pfx'
    $pfxPassword = ConvertTo-SecureString -String 'iFreeSftpTest' -Force -AsPlainText
    Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $pfxPassword | Out-Null
    & $signtool sign /fd SHA256 /f $pfxPath /p 'iFreeSftpTest' $msixPath
    if ($LASTEXITCODE -ne 0) { throw "signtool sign failed, exit code $LASTEXITCODE" }
    $cerPath = Join-Path $WorkDir 'test-signing.cer'
    Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
    Write-Host @"

==> Package signed with a self-signed test cert. To install locally (admin PowerShell):
    Import-Certificate -FilePath "$cerPath" -CertStoreLocation Cert:\LocalMachine\TrustedPeople
    Add-AppxPackage -Path "$msixPath"
"@ -ForegroundColor Yellow
}

$size = '{0:N1} MB' -f ((Get-Item $msixPath).Length / 1MB)
Write-Host "==> MSIX OK (Core=$(if ($Official) { 'official' } else { 'community' }), Version=$Version): $msixPath [$size]" -ForegroundColor Green
