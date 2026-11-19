$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot

$script:fail = 0
function Assert-Match {
    param([string]$Name, [bool]$Ok, [string]$Detail)
    if ($Ok) { Write-Host "  PASS  $Name" -ForegroundColor Green }
    else { Write-Host "  FAIL  $Name  <- $Detail" -ForegroundColor Red; $script:fail++ }
}

# --- 基准: workspace Cargo.toml [workspace.package] version ---
$Cargo = Join-Path $RepoRoot 'Cargo.toml'
$verLine = Select-String -Path $Cargo -Pattern '^version = "(\d+\.\d+\.\d+)"' | Select-Object -First 1
if (-not $verLine) {
    Write-Host "FAIL: 无法从 $Cargo 解析 [workspace.package] version" -ForegroundColor Red
    exit 1
}
$Version = $verLine.Matches[0].Groups[1].Value
Write-Host "基准版本 (Cargo.toml workspace version): $Version"

# 1. README 双语徽章
foreach ($pair in @(
        @{ Path = 'README.md';        Name = 'README 版本徽章' },
        @{ Path = 'README.zh-CN.md';  Name = 'README.zh-CN 版本徽章' })) {
    $full = Join-Path $RepoRoot $pair.Path
    $r = Select-String -Path $full -Pattern 'badge/version-([\d.]+)-' | Select-Object -First 1
    $rv = if ($r) { $r.Matches[0].Groups[1].Value } else { '' }
    Assert-Match $pair.Name ($rv -eq $Version) "实际 $rv, 期望 $Version"
}

# 2. CHANGELOG 双语发版节
foreach ($ChangelogFile in 'CHANGELOG.md', 'CHANGELOG.zh-CN.md') {
    $hasSection = $null -ne (Select-String -Path (Join-Path $RepoRoot $ChangelogFile) -Pattern ('^## \[' + [regex]::Escape($Version) + '\]'))
    Assert-Match "$ChangelogFile 存在 [$Version] 节" $hasSection '缺少对应发版节'
}

# 3. 官网首页下载链接 + 版本文案 (website\ 缺失时 SKIP, 不阻塞发布)
$Url = "releases/download/v$Version/iFreeSftp-v$Version-win-x64.zip"
$Wi = Join-Path $RepoRoot 'website\index.html'
$Wzh = Join-Path $RepoRoot 'website\zh\index.html'
if ((Test-Path $Wi) -and (Test-Path $Wzh)) {
    Assert-Match '官网 EN 下载链接' ($null -ne (Select-String -Path $Wi -Pattern $Url -SimpleMatch)) "缺少 $Url"
    Assert-Match '官网 EN 版本文案' ($null -ne (Select-String -Path $Wi -Pattern "Version v$Version")) "缺少 Version v$Version"
    Assert-Match '官网 ZH 下载链接' ($null -ne (Select-String -Path $Wzh -Pattern $Url -SimpleMatch)) "缺少 $Url"
    Assert-Match '官网 ZH 版本文案' ($null -ne (Select-String -Path $Wzh -Pattern "版本 v$Version")) "缺少 版本 v$Version"
}
else {
    Write-Host '  SKIP  官网首页 4 项  <- website\ 未入库' -ForegroundColor Yellow
}

# 4. 官网 changelog 条目 (website\ 缺失时 SKIP)
$Wc = Join-Path $RepoRoot 'website\changelog.html'
$Wczh = Join-Path $RepoRoot 'website\zh\changelog.html'
if ((Test-Path $Wc) -and (Test-Path $Wczh)) {
    Assert-Match '官网 EN changelog 条目' ($null -ne (Select-String -Path $Wc -Pattern ">v$Version</h4>")) "缺少 >v$Version</h4>"
    Assert-Match '官网 ZH changelog 条目' ($null -ne (Select-String -Path $Wczh -Pattern ">v$Version</h4>")) "缺少 >v$Version</h4>"
}
else {
    Write-Host '  SKIP  官网 changelog 2 项  <- website\ 未入库' -ForegroundColor Yellow
}

# --- 汇总 ---
Write-Host ''
if ($script:fail -gt 0) {
    Write-Host "check-version: $script:fail 项不一致 —— 发版前必须全部同步" -ForegroundColor Red
    exit 1
}
Write-Host 'check-version: 全部一致' -ForegroundColor Green
exit 0
