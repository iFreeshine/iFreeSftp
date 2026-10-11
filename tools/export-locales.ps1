# export-locales.ps1 — 语言词典导出: Strings\<lang>\Resources.resw → locales\<lang>.json
#
# 发布管线 (iFreeSftp.csproj 的 ExportLocales target) 自动调用, 也可手动执行:
#   powershell -NoProfile -ExecutionPolicy Bypass -File export-locales.ps1 `
#     -ProjectDir <src\iFreeSftp> -OutputDir <发布目录>\locales
#
# 输出为 UTF-8 (无 BOM)、键排序的扁平 JSON。用户可直接编辑译文, 或复制任一 json
# 改名翻译成新语言 (如 fr-FR.json), 放到应用旁 locales\ 目录重启生效。
param(
    [Parameter(Mandatory = $true)][string]$ProjectDir,
    [Parameter(Mandatory = $true)][string]$OutputDir
)

$ErrorActionPreference = 'Stop'

$stringsDir = Join-Path $ProjectDir 'Strings'
if (-not (Test-Path $stringsDir)) {
    Write-Host "export-locales: 未找到 $stringsDir, 跳过导出"
    exit 0
}

# JSON 字符串转义: 手写而非 ConvertTo-Json——后者把非 ASCII 转成 \uXXXX,
# 词典文件需要用户直接编辑, 中文等字符必须以字面量输出。
function Escape-JsonText([string]$s) {
    $sb = New-Object System.Text.StringBuilder
    foreach ($c in $s.ToCharArray()) {
        if ($c -eq '"')           { [void]$sb.Append('\"') }
        elseif ($c -eq '\')       { [void]$sb.Append('\\') }
        elseif ($c -eq "`n")      { [void]$sb.Append('\n') }
        elseif ($c -eq "`r")      { [void]$sb.Append('\r') }
        elseif ($c -eq "`t")      { [void]$sb.Append('\t') }
        elseif ([int]$c -lt 0x20) { [void]$sb.Append(('\u{0:x4}' -f [int]$c)) }
        else                      { [void]$sb.Append($c) }
    }
    $sb.ToString()
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$exported = @()

Get-ChildItem -Path $stringsDir -Directory | ForEach-Object {
    $lang = $_.Name
    $resw = Join-Path $_.FullName 'Resources.resw'
    if (-not (Test-Path $resw)) { return }

    [xml]$doc = Get-Content -Raw -Encoding UTF8 $resw

    # name → value; 重复键后者覆盖 (zh-CN resw 存在历史重复键, 值一致)
    $entries = @{}
    foreach ($data in $doc.root.data) {
        if ([string]::IsNullOrEmpty($data.name)) { continue }
        $entries[[string]$data.name] = [string]$data.value
    }
    if ($entries.Count -eq 0) { return }

    $json = New-Object System.Text.StringBuilder
    [void]$json.AppendLine('{')
    $keys = @($entries.Keys | Sort-Object)
    for ($i = 0; $i -lt $keys.Count; $i++) {
        $comma = if ($i -lt $keys.Count - 1) { ',' } else { '' }
        [void]$json.AppendLine(('  "' + (Escape-JsonText $keys[$i]) + '": "' +
            (Escape-JsonText ([string]$entries[$keys[$i]])) + '"' + $comma))
    }
    [void]$json.AppendLine('}')

    $outFile = Join-Path $OutputDir ($lang + '.json')
    [System.IO.File]::WriteAllText($outFile, $json.ToString(), $utf8NoBom)
    $exported += ("{0}({1} 键)" -f $lang, $entries.Count)
}

Write-Host ("export-locales: 导出语言词典 {0} -> {1}" -f ($exported -join ', '), $OutputDir)
