# iFreeSftp MSIX packaging script (Microsoft Store submission / local test install)
# Usage:
#   .\tools\make-msix.ps1                              # stub build, unsigned msix (Store accepts unsigned, signs on submission)
#   .\tools\make-msix.ps1 -Core official               # official build (requires core\iFreeSftp.Core.dll)
#   .\tools\make-msix.ps1 -Sign                        # + self-signed cert for local Add-AppxPackage testing
#   .\tools\make-msix.ps1 -IdentityName 'XXXXX' -Publisher 'CN=XXXX...'   # real Partner Center identity
param(
    # Core implementation mode, same meaning as build.ps1 (stub = community, official = closed-source core dll)
    [ValidateSet('stub', 'official')]
    [string]$Core = 'stub',

    # Package version, defaults to <Version> parsed from the main csproj (e.g. 0.8.0 -> 0.8.0.0)
    [string]$Version,

    # Package identity from Partner Center "App identity" page (0.8.3 提交时
    # Partner Center 校验报错拿到的真实值, 两者必须与账户完全一致)
    [string]$IdentityName = 'iFrees.iFreeSftp',
    [string]$Publisher = 'CN=029857DD-BD5B-48BB-BC40-43193DDB2DE9',

    # Self-sign the msix with a throwaway cert for local Add-AppxPackage smoke testing
    [switch]$Sign
)

$ErrorActionPreference = 'Stop'

# Locate dotnet.exe: prefer user-profile install, fallback to PATH (same as build.ps1)
$Dotnet = Join-Path $env:USERPROFILE '.dotnet\dotnet.exe'
if (-not (Test-Path $Dotnet)) { $Dotnet = 'dotnet' }

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Project = Join-Path $RepoRoot 'src\iFreeSftp\iFreeSftp.csproj'
$WorkDir = Join-Path $RepoRoot 'release\msix'
$PayloadDir = Join-Path $WorkDir 'payload'

# --- Version: default from csproj ---
if (-not $Version) {
    $line = Select-String -Path $Project -Pattern '<Version>(\d+\.\d+\.\d+)</Version>' | Select-Object -First 1
    if (-not $line) { throw "Cannot parse <Version> from $Project - pass -Version explicitly" }
    $Version = $line.Matches[0].Groups[1].Value
}
if ($Version -notmatch '^\d+\.\d+\.\d+\.\d+$') { $Version = "$Version.0" }

# --- 防漏校验: 各声明点版本号与 csproj 一致 (app.manifest/README/CHANGELOG/官网) ---
& (Join-Path $PSScriptRoot 'check-version.ps1')
if ($LASTEXITCODE -ne 0) { throw 'check-version failed - aborting MSIX packaging' }

# --- official mode precondition (same as build.ps1 release) ---
if ($Core -eq 'official' -and -not (Test-Path (Join-Path $RepoRoot 'core\iFreeSftp.Core.dll'))) {
    throw "official mode requires core\iFreeSftp.Core.dll - copy it in from the private repo first, or use -Core stub"
}

# --- Publish (loose-file layout, NOT single-file: MSIX wants on-disk files; also
#     skips the first-launch %TEMP% extraction, store version starts faster) ---
Write-Host "==> dotnet publish (Release, self-contained, loose files) -> $PayloadDir" -ForegroundColor Cyan
Get-Process iFreeSftp -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800
if (Test-Path $WorkDir) { Remove-Item $WorkDir -Recurse -Force }
New-Item -ItemType Directory -Force $PayloadDir | Out-Null
& $Dotnet publish $Project -c Release -r win-x64 -p:Platform=x64 -p:PublishDir="$PayloadDir\" `
    -p:PublishSingleFile=false -p:WindowsPackageType=None -p:CoreImplementation=$Core
if ($LASTEXITCODE -ne 0) { throw "publish failed, exit code $LASTEXITCODE" }

# --- Payload sanity checks (loose-file publish: app PRI ships as a loose file
#     named after the app; WindowsAppSDK framework PRIs sit alongside it) ---
$exe = Join-Path $PayloadDir 'iFreeSftp.exe'
$pri = Join-Path $PayloadDir 'iFreeSftp.pri'
foreach ($f in @($exe, $pri)) {
    if (-not (Test-Path $f)) { throw "payload missing required file: $f" }
}
if ((Get-Item $pri).Length -lt 1KB) { throw "iFreeSftp.pri is suspiciously small ($((Get-Item $pri).Length) bytes) - MSIX tooling/publish config broken" }

# --- Supplement payload with files the MSIX shell needs at rest ---
# Package-level PRI (shell tile/icon resolution): reuse the app PRI as a starting point
Copy-Item $pri (Join-Path $PayloadDir 'resources.pri') -Force
# Tile/splash assets: copy the WHOLE project Assets folder, not a hand-picked subset.
# The PRI indexes every asset in the project (including qualifier variants such as
# targetsize-*/altform-*), so any file left out of the payload leaves the shell
# resolving to a dead path. Symptom: Start menu / taskbar shortcut icon renders as a
# grey broken-image placeholder while the app itself works fine (Store page icon is
# uploaded separately to Partner Center, so it stays intact). 0.8.5 shipped without
# Square44x44Logo.targetsize-48_altform-unplated.png and hit exactly this.
$assetsSrc = Join-Path $RepoRoot 'src\iFreeSftp\Assets'
if (-not (Test-Path $assetsSrc)) { throw "project assets folder missing: $assetsSrc" }
New-Item -ItemType Directory -Force (Join-Path $PayloadDir 'Assets') | Out-Null
Get-ChildItem $assetsSrc -File -Recurse | ForEach-Object {
    Copy-Item $_.FullName (Join-Path $PayloadDir "Assets\$($_.Name)") -Force
}

# --- PRI/asset integrity gate ---
# Every icon resource the PRI advertises must physically exist in the payload; a
# registered-but-missing file is what turns into the broken-icon placeholder. Only
# icon-family resources are asserted (NoiseAsset_256X256_PNG.png comes from the WinUI
# template index and is not used by this app's UI).
$priBytes = [IO.File]::ReadAllBytes((Join-Path $PayloadDir 'resources.pri'))
$priText = [Text.Encoding]::UTF8.GetString($priBytes)
$iconRes = [regex]::Matches($priText,
    'Assets\\(?:Square44x44Logo|Square150x150Logo|Wide310x150Logo|SplashScreen|StoreLogo|LockScreenLogo|TitleBarLogo|AppIcon)[A-Za-z0-9_\-\.]*') |
    ForEach-Object { $_.Value } | Sort-Object -Unique
$missing = @($iconRes | Where-Object { -not (Test-Path (Join-Path $PayloadDir $_)) })
if ($missing.Count -gt 0) {
    throw "PRI registers icon assets missing from payload (would render as broken icon):`n  $($missing -join "`n  ")"
}
Write-Host "==> icon assets OK ($($iconRes.Count) PRI-registered files present)" -ForegroundColor DarkGray

# --- Write injected AppxManifest ---
$manifestSrc = Join-Path $PSScriptRoot 'msix\AppxManifest.xml'
$manifest = Get-Content $manifestSrc -Raw -Encoding UTF8
$manifest = $manifest.Replace('__IDENTITY_NAME__', $IdentityName).Replace('__IDENTITY_PUBLISHER__', $Publisher).Replace('__VERSION__', $Version)
$manifestDst = Join-Path $PayloadDir 'AppxManifest.xml'
[System.IO.File]::WriteAllText($manifestDst, $manifest, (New-Object System.Text.UTF8Encoding($false)))

# --- Locate MakeAppx.exe / signtool.exe ---
function Find-SdkTool([string]$name) {
    # Green setup first: D:\dev\winsdk holds a manually extracted
    # Microsoft.Windows.SDK.BuildTools nupkg (survives OS reinstalls; no
    # Windows SDK installer needed). Layout matches the NuGet package.
    $green = 'D:\dev\winsdk'
    if (Test-Path $green) {
        $tool = Get-ChildItem (Join-Path $green "bin\*\x64\$name") -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($tool) { return $tool.FullName }
    }
    # Prefer the Microsoft.Windows.SDK.BuildTools NuGet package already pulled by the
    # project (ships the full SDK toolset; no separate Windows SDK install needed).
    # NOTE: put the tool name in the wildcarded path itself - in PS 5.1 -Filter is
    # silently ignored when Path contains wildcards.
    $nuget = Join-Path $env:USERPROFILE '.nuget\packages\microsoft.windows.sdk.buildtools'
    if (Test-Path $nuget) {
        $tool = Get-ChildItem $nuget -Directory | Sort-Object Name -Descending |
            ForEach-Object { Get-ChildItem (Join-Path $_.FullName "bin\*\x64\$name") -ErrorAction SilentlyContinue } |
            Select-Object -First 1
        if ($tool) { return $tool.FullName }
    }
    # Fall back to a classic Windows SDK installation
    $kitRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path $kitRoot) {
        $tool = Get-ChildItem $kitRoot -Directory | Where-Object { $_.Name -match '^10\.' } |
            Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName "x64\$name" } |
            Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($tool) { return $tool }
    }
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw "$name not found - restore NuGet packages first (dotnet restore), or install the Windows SDK"
}
$makeAppx = Find-SdkTool 'MakeAppx.exe'

# --- Pack ---
$msixPath = Join-Path $RepoRoot "release\iFreeSftp-$Version-x64.msix"
Write-Host "==> MakeAppx pack -> $msixPath" -ForegroundColor Cyan
& $makeAppx pack /o /d $PayloadDir /p $msixPath
if ($LASTEXITCODE -ne 0) { throw "MakeAppx pack failed, exit code $LASTEXITCODE" }

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
Write-Host "==> MSIX OK (Core=$Core, Version=$Version): $msixPath [$size]" -ForegroundColor Green
