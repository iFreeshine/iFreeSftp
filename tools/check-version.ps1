# iFreeSftp 版本号一致性防漏校验 (发布前必跑; build.ps1 release 与 make-msix.ps1 已自动挂钩)
# 基准版本从主 csproj <Version> 解析, 校验以下声明点全部一致:
#   1. src\iFreeSftp\app.manifest            assemblyIdentity version (四段, 前三段须相等)
#   2. README.md                             版本徽章 img.shields.io/badge/version-x.y.z-
#   3. CHANGELOG.md / CHANGELOG.zh-CN.md     存在 "## [x.y.z]" 发版节
#   4. website\index.html + website\zh\index.html      GitHub 下载链接与版本文案
#   5. website\changelog.html + website\zh\changelog.html  "v x.y.z" 条目标题
#   6. web2\changelog.html + web2\zh-changelog.html    "v x.y.z" 条目标题 (web2 扁平站日志页)
# website\ / web2\ 文件缺失时对应项自动 SKIP (可选站点目录, 不阻塞发布); 1-3 项为仓库内必备文件, 缺失仍 FAIL。
# 任一不一致即 FAIL (退出码 1, 阻塞发布); 全部一致输出 PASS。
# 用法: .\tools\check-version.ps1  (无参数)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot

$script:fail = 0
function Assert-Match {
    param([string]$Name, [bool]$Ok, [string]$Detail)
    if ($Ok) { Write-Host "  PASS  $Name" -ForegroundColor Green }
    else { Write-Host "  FAIL  $Name  <- $Detail" -ForegroundColor Red; $script:fail++ }
}

# --- 基准: 主 csproj <Version> ---
$Csproj = Join-Path $RepoRoot 'src\iFreeSftp\iFreeSftp.csproj'
$verLine = Select-String -Path $Csproj -Pattern '<Version>(\d+\.\d+\.\d+)</Version>' | Select-Object -First 1
if (-not $verLine) {
    Write-Host "FAIL: 无法从 $Csproj 解析 <Version>" -ForegroundColor Red
    exit 1
}
$Version = $verLine.Matches[0].Groups[1].Value
$Version4 = "$Version.0"
Write-Host "基准版本 (csproj <Version>): $Version"

# 1. app.manifest
$Manifest = Join-Path $RepoRoot 'src\iFreeSftp\app.manifest'
$m = Select-String -Path $Manifest -Pattern 'assemblyIdentity[^>]*version="([\d.]+)"' | Select-Object -First 1
$mv = if ($m) { $m.Matches[0].Groups[1].Value } else { '' }
Assert-Match 'app.manifest 清单版本' ($mv -eq $Version4) "实际 $mv, 期望 $Version4"

# 2. README 徽章
$Readme = Join-Path $RepoRoot 'README.md'
$r = Select-String -Path $Readme -Pattern 'badge/version-([\d.]+)-' | Select-Object -First 1
$rv = if ($r) { $r.Matches[0].Groups[1].Value } else { '' }
Assert-Match 'README 版本徽章' ($rv -eq $Version) "实际 $rv, 期望 $Version"

# 3. CHANGELOG 双语发版节
foreach ($ChangelogFile in 'CHANGELOG.md', 'CHANGELOG.zh-CN.md') {
    $hasSection = $null -ne (Select-String -Path (Join-Path $RepoRoot $ChangelogFile) -Pattern ('^## \[' + [regex]::Escape($Version) + '\]'))
    Assert-Match "$ChangelogFile 存在 [$Version] 节" $hasSection '缺少对应发版节'
}

# 4. 官网首页下载链接 + 版本文案 (website\ 缺失时 SKIP, 不阻塞发布)
$Url = "releases/download/v$Version/iFreeSftp-v$Version-win-x64.zip"
$Wi = Join-Path $RepoRoot 'website\index.html'
$Wzh = Join-Path $RepoRoot 'website\zh\index.html'
if ((Test-Path $Wi) -and (Test-Path $Wzh)) {
    # -SimpleMatch 按字面匹配, 直接传原始 URL (勿再 regex::Escape, 否则 \. 反而失配)
    Assert-Match '官网 EN 下载链接' ($null -ne (Select-String -Path $Wi -Pattern $Url -SimpleMatch)) "缺少 $Url"
    Assert-Match '官网 EN 版本文案' ($null -ne (Select-String -Path $Wi -Pattern "Version v$Version")) "缺少 Version v$Version"
    Assert-Match '官网 ZH 下载链接' ($null -ne (Select-String -Path $Wzh -Pattern $Url -SimpleMatch)) "缺少 $Url"
    Assert-Match '官网 ZH 版本文案' ($null -ne (Select-String -Path $Wzh -Pattern "版本 v$Version")) "缺少 版本 v$Version"
}
else {
    Write-Host '  SKIP  官网首页 4 项  <- website\ 未入库' -ForegroundColor Yellow
}

# 5. 官网 changelog 条目 (website\ 缺失时 SKIP)
$Wc = Join-Path $RepoRoot 'website\changelog.html'
$Wczh = Join-Path $RepoRoot 'website\zh\changelog.html'
if ((Test-Path $Wc) -and (Test-Path $Wczh)) {
    Assert-Match '官网 EN changelog 条目' ($null -ne (Select-String -Path $Wc -Pattern ">v$Version</h4>")) "缺少 >v$Version</h4>"
    Assert-Match '官网 ZH changelog 条目' ($null -ne (Select-String -Path $Wczh -Pattern ">v$Version</h4>")) "缺少 >v$Version</h4>"
}
else {
    Write-Host '  SKIP  官网 changelog 2 项  <- website\ 未入库' -ForegroundColor Yellow
}

# 6. web2 扁平站 changelog 条目 (web2\ 已 gitignore, 干净克隆上没有 -> SKIP, 不阻塞发布)
foreach ($pair in @(
        @{ Path = 'web2\changelog.html';    Name = 'web2 EN changelog 条目' },
        @{ Path = 'web2\zh-changelog.html'; Name = 'web2 ZH changelog 条目' })) {
    $full = Join-Path $RepoRoot $pair.Path
    if (-not (Test-Path $full)) {
        Write-Host "  SKIP  $($pair.Name)  <- $($pair.Path) 未入库" -ForegroundColor Yellow
        continue
    }
    Assert-Match $pair.Name ($null -ne (Select-String -Path $full -Pattern ">v$Version</h4>")) "缺少 >v$Version</h4>"
}

# --- 汇总 ---
Write-Host ''
if ($script:fail -gt 0) {
    Write-Host "check-version: $script:fail 项不一致 —— 发版前必须全部同步" -ForegroundColor Red
    exit 1
}
Write-Host 'check-version: 全部一致' -ForegroundColor Green
exit 0
