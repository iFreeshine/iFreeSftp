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

    # Package identity from Partner Center "App identity" page (placeholders fine for local test install)
    [string]$IdentityName = '17739B12-7C1F-45C9-894C-46B43951093D',
    [string]$Publisher = 'CN=AppPublisher',

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
# Manifest-referenced tile/splash assets: copy the icon set from project Assets
$assetsSrc = Join-Path $RepoRoot 'src\iFreeSftp\Assets'
foreach ($asset in @('Square44x44Logo.scale-200.png', 'Square44x44Logo.targetsize-24_altform-unplated.png', 'Square44x44Logo.targetsize-48_altform-lightunplated.png', 'Square150x150Logo.scale-200.png', 'Wide310x150Logo.scale-200.png', 'SplashScreen.scale-200.png', 'StoreLogo.png', 'LockScreenLogo.scale-200.png')) {
    $src = Join-Path $assetsSrc $asset
    if (-not (Test-Path $src)) { throw "project asset missing: $src" }
    Copy-Item $src (Join-Path $PayloadDir "Assets\$asset") -Force
}

# --- Write injected AppxManifest ---
$manifestSrc = Join-Path $PSScriptRoot 'msix\AppxManifest.xml'
$manifest = Get-Content $manifestSrc -Raw -Encoding UTF8
$manifest = $manifest.Replace('__IDENTITY_NAME__', $IdentityName).Replace('__IDENTITY_PUBLISHER__', $Publisher).Replace('__VERSION__', $Version)
$manifestDst = Join-Path $PayloadDir 'AppxManifest.xml'
[System.IO.File]::WriteAllText($manifestDst, $manifest, (New-Object System.Text.UTF8Encoding($false)))

# --- Locate MakeAppx.exe / signtool.exe ---
function Find-SdkTool([string]$name) {
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
