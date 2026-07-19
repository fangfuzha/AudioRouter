<#
.SYNOPSIS
    本地打包并直接发布到 GitHub Release（绕过 GitHub Actions）。

.DESCRIPTION
    此脚本完成端到端的发布流程：
      1. 校验版本号与 winui3_gui/Cargo.toml 一致
      2. 检查 gh CLI 是否已认证
      3. 调用 build-installer.ps1 本地构建 + Inno Setup 打包
      4. 创建 git tag（可选）并推送
      5. 使用 gh release create 创建 Release 并上传安装包

.PARAMETER Version
    要发布的版本号。支持 "0.3.3" 或 "v0.3.3" 两种格式。
    留空则从 winui3_gui/Cargo.toml 自动读取。

.PARAMETER SkipTag
    跳过创建/推送 git tag。默认为 true，因为推送 tag 会触发 release-windows.yml
    工作流，可能与本地发布的产物冲突。
    如需保留 git tag 历史，可手动执行：
      git tag -a v0.3.3 -m "Release v0.3.3"
      git push origin v0.3.3
    （注意：这会触发 CI 工作流）

.PARAMETER ForceTag
    如果 tag 已存在，强制删除并重新创建（覆盖远程 tag）。

.PARAMETER Draft
    创建为 Draft Release（不会公开，可在 GitHub 页面手动发布）。

.PARAMETER Prerelease
    标记为预发布版本。

.PARAMETER Notes
    Release notes 文本。留空则让 GitHub 根据 commits 自动生成
    （generate_release_notes）。

.PARAMETER NotesFile
    Release notes 文件路径。若提供则优先使用文件内容。

.PARAMETER NoBuild
    跳过 cargo build + Inno Setup 打包，直接发布已有的安装包。
    用于发布已构建好的产物。

.EXAMPLE
    .\scripts\publish.ps1
    从 Cargo.toml 读取版本号，构建并发布。

.EXAMPLE
    .\scripts\publish.ps1 -Version 0.3.3 -Draft
    指定版本号，创建为 Draft Release。

.EXAMPLE
    .\scripts\publish.ps1 -SkipTag
    仅发布到 GitHub Release，不创建/推送 git tag。

.EXAMPLE
    .\scripts\publish.ps1 -NotesFile .\CHANGELOG.md
    使用 CHANGELOG.md 作为 release notes。
#>
param(
    [string]$Version = "",
    [switch]$SkipTag = $true,
    [switch]$ForceTag = $false,
    [switch]$Draft = $false,
    [switch]$Prerelease = $false,
    [string]$Notes = "",
    [string]$NotesFile = "",
    [switch]$NoBuild = $false
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$CargoToml = Join-Path $ProjectRoot "winui3_gui\Cargo.toml"

function Get-CargoVersion {
    if (-not (Test-Path $CargoToml)) {
        Write-Host "Error: Cargo.toml not found at $CargoToml" -ForegroundColor Red
        exit 1
    }
    $content = Get-Content $CargoToml -Raw
    if ($content -match 'version\s*=\s*"([^"]+)"') {
        return $matches[1]
    }
    Write-Host "Error: Could not parse version from Cargo.toml" -ForegroundColor Red
    exit 1
}

function Normalize-Version {
    param([string]$v)
    # 去掉前缀 v/V
    return $v -replace '^[vV]', ''
}

function Test-GhAuth {
    $null = gh auth status 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Error: gh CLI not authenticated." -ForegroundColor Red
        Write-Host "Run: gh auth login" -ForegroundColor Yellow
        exit 1
    }
}

function Test-WorktreeClean {
    Push-Location $ProjectRoot
    try {
        $status = git status --porcelain 2>&1
        if ($status) {
            Write-Host "Warning: Working tree has uncommitted changes:" -ForegroundColor Yellow
            Write-Host $status
            $confirm = Read-Host "Continue anyway? (y/N)"
            if ($confirm -ne 'y' -and $confirm -ne 'Y') {
                Write-Host "Aborted." -ForegroundColor Red
                exit 1
            }
        }
    }
    finally {
        Pop-Location
    }
}

# === 1. 解析版本号 ===
$cargoVersion = Get-CargoVersion
if ($Version) {
    $Version = Normalize-Version $Version
    if ($Version -ne $cargoVersion) {
        Write-Host "Error: Requested version ($Version) does not match Cargo.toml ($cargoVersion)." -ForegroundColor Red
        Write-Host "Update winui3_gui/Cargo.toml first, or omit -Version to use Cargo.toml's version." -ForegroundColor Yellow
        exit 1
    }
} else {
    $Version = $cargoVersion
}
$TagName = "v$Version"
Write-Host "Publishing version: $TagName" -ForegroundColor Cyan

# === 2. 检查 gh CLI 认证 ===
Test-GhAuth

# === 3. 检查工作区（仅警告，不强制阻止） ===
Test-WorktreeClean

# === 4. 构建 + 打包 ===
if (-not $NoBuild) {
    Write-Host ""
    Write-Host "=== Building & packaging ===" -ForegroundColor Cyan
    $buildScript = Join-Path $PSScriptRoot "build-installer.ps1"
    & $buildScript -Version $Version
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Build failed!" -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "Skipping build (-NoBuild)" -ForegroundColor Yellow
}

# === 5. 定位安装包 ===
$OutputDir = Join-Path $ProjectRoot "installer\Output"
$installer = Get-ChildItem $OutputDir -Filter "AudioRouter-Setup-$Version-*.exe" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) {
    # 兜底：匹配任意版本
    $installer = Get-ChildItem $OutputDir -Filter "AudioRouter-Setup-*.exe" |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
}
if (-not $installer) {
    Write-Host "Error: Installer not found in $OutputDir" -ForegroundColor Red
    exit 1
}
$sizeMB = [math]::Round($installer.Length / 1MB, 2)
Write-Host "Installer: $($installer.Name) ($sizeMB MB)" -ForegroundColor Green

# === 6. git tag 处理 ===
if (-not $SkipTag) {
    Push-Location $ProjectRoot
    try {
        # 检查 tag 是否已存在
        $existingTag = git rev-parse -q --verify "refs/tags/$TagName" 2>&1
        if ($LASTEXITCODE -eq 0) {
            if ($ForceTag) {
                Write-Host "Tag $TagName already exists, deleting (-ForceTag)..." -ForegroundColor Yellow
                git tag -d $TagName
                git push origin ":refs/tags/$TagName" 2>&1 | Out-Host
            } else {
                Write-Host "Error: Tag $TagName already exists." -ForegroundColor Red
                Write-Host "Use -ForceTag to overwrite, or -SkipTag to skip tag creation." -ForegroundColor Yellow
                exit 1
            }
        }

        Write-Host "Creating tag $TagName..." -ForegroundColor Cyan
        git tag -a $TagName -m "Release $TagName"
        if ($LASTEXITCODE -ne 0) {
            Write-Host "Failed to create tag." -ForegroundColor Red
            exit 1
        }

        Write-Host "Pushing tag $TagName to origin..." -ForegroundColor Cyan
        git push origin $TagName
        if ($LASTEXITCODE -ne 0) {
            Write-Host "Warning: Failed to push tag. You can push manually: git push origin $TagName" -ForegroundColor Yellow
        }
    }
    finally {
        Pop-Location
    }
} else {
    Write-Host "Skipping git tag (-SkipTag)" -ForegroundColor Yellow
}

# === 7. 创建 GitHub Release ===
Write-Host ""
Write-Host "=== Creating GitHub Release ===" -ForegroundColor Cyan

$releaseArgs = @("release", "create", $TagName, $installer.FullName, "--title", "AudioRouter $TagName")

if ($Draft) {
    $releaseArgs += "--draft"
}
if ($Prerelease) {
    $releaseArgs += "--prerelease"
}

# Release notes 优先级：-NotesFile > -Notes > 自动生成
if ($NotesFile -and (Test-Path $NotesFile)) {
    $releaseArgs += @("--notes-file", $NotesFile)
} elseif ($Notes) {
    $releaseArgs += @("--notes", $Notes)
} else {
    $releaseArgs += "--generate-release-notes"
}

# 检查 release 是否已存在
$existingRelease = gh release view $TagName --json name 2>$null
if ($existingRelease) {
    Write-Host "Release $TagName already exists. Uploading asset (--clobber)..." -ForegroundColor Yellow
    gh release upload $TagName $installer.FullName --clobber
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Failed to upload asset." -ForegroundColor Red
        exit 1
    }
} else {
    & gh @releaseArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Failed to create release." -ForegroundColor Red
        exit 1
    }
}

# === 8. 完成 ===
$repoUrl = gh repo view --json url -q .url 2>$null
Write-Host ""
Write-Host "=== Published ===" -ForegroundColor Green
Write-Host "Tag: $TagName"
Write-Host "Installer: $($installer.Name) ($sizeMB MB)"
if ($repoUrl) {
    Write-Host "Release URL: $repoUrl/releases/tag/$TagName"
}
if ($SkipTag) {
    Write-Host ""
    Write-Host "Note: git tag was skipped (default). To create tag history (will trigger CI):" -ForegroundColor Yellow
    Write-Host "  git tag -a $TagName -m 'Release $TagName'"
    Write-Host "  git push origin $TagName"
}
