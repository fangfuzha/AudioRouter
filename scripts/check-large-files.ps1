<#
.SYNOPSIS
  检查仓库里有没有超过体积上限的文件，超过就让 CI 失败。

.DESCRIPTION
  防止把大文件（构建产物、安装包、录屏、模型权重等）提交进仓库：
  仓库体积会暴涨、clone 变慢、GitHub 的单文件 100MB 硬限制迟早会被撞到。

  只检查 Git 关心的文件（已跟踪 + 未跟踪但未忽略），因此 target/、
  installer/Output/ 这类被 .gitignore 排除的目录天然不参与检查。

.PARAMETER MaxSizeKB
  单文件体积上限（KB），默认 1024（1MB）。

.PARAMETER AllowListPath
  例外清单路径，默认 scripts/large-files-allowlist.txt。
  清单里每行一个路径（相对仓库根），用于豁免确实需要入库的大资源。

.EXAMPLE
  ./scripts/check-large-files.ps1
.EXAMPLE
  ./scripts/check-large-files.ps1 -MaxSizeKB 512
#>
[CmdletBinding()]
param(
    [int]$MaxSizeKB = 1024,
    [string]$AllowListPath = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    if (-not $AllowListPath) {
        $AllowListPath = Join-Path $repoRoot "scripts/large-files-allowlist.txt"
    }

    # 例外清单：# 开头是注释，空行忽略
    $allowed = @()
    if (Test-Path -LiteralPath $AllowListPath) {
        $allowed = Get-Content -LiteralPath $AllowListPath |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ -ne "" -and -not $_.StartsWith("#") }
    }

    # git ls-files：已跟踪文件 + 未跟踪但未忽略的文件
    # （未跟踪的那部分让本地也能在 add 之前就发现大文件）
    $files = @(git ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) {
        Write-Error "git ls-files 执行失败"
        exit 1
    }

    $limit = $MaxSizeKB * 1KB
    $offenders = @()

    foreach ($rel in $files) {
        if ($allowed -contains $rel) { continue }
        $full = Join-Path $repoRoot $rel
        if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { continue }
        $item = Get-Item -LiteralPath $full
        if ($item.Length -gt $limit) {
            $offenders += [PSCustomObject]@{
                Path  = $rel
                SizeK = [math]::Round($item.Length / 1KB, 1)
            }
        }
    }

    if ($offenders.Count -gt 0) {
        Write-Host "检测到 $($offenders.Count) 个超过上限（$MaxSizeKB KB）的文件：" -ForegroundColor Red
        foreach ($o in $offenders) {
            Write-Host ("  {0}  ({1} KB)" -f $o.Path, $o.SizeK) -ForegroundColor Red
            Write-Host "::error file=$($o.Path)::文件超过 $MaxSizeKB KB（实际 $($o.SizeK) KB），请勿提交大文件。确需入库请在 scripts/large-files-allowlist.txt 中登记例外。"
        }
        exit 1
    }

    Write-Host "大文件检查通过：上限 $MaxSizeKB KB，共检查 $($files.Count) 个文件。"
    exit 0
}
finally {
    Pop-Location
}
