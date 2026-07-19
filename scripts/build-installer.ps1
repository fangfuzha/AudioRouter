param(
    [string]$Version = "",
    [string]$Config = "release",
    [switch]$NoBuild = $false,
    [switch]$SkipSign = $false
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$InstallerDir = Join-Path $ProjectRoot "installer"
$OutputDir = Join-Path $InstallerDir "Output"
$TargetDir = Join-Path $ProjectRoot "target\$Config"

function Find-InnoSetup {
    if ($env:ISCC_PATH -and (Test-Path $env:ISCC_PATH)) {
        return $env:ISCC_PATH
    }
    if ($env:INNO_PATH) {
        $p = Join-Path $env:INNO_PATH "ISCC.exe"
        if (Test-Path $p) { return $p }
    }
    $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $candidates = @(
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe",
        "C:\Program Files (x86)\Inno Setup 5\ISCC.exe",
        "C:\Program Files\Inno Setup 5\ISCC.exe",
        "D:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "D:\Program Files\Inno Setup 6\ISCC.exe",
        "E:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "E:\Program Files\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) { return $c }
    }
    $regPaths = @(
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
        "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1"
    )
    foreach ($rp in $regPaths) {
        if (Test-Path $rp) {
            $installLoc = (Get-ItemProperty $rp -ErrorAction SilentlyContinue).InstallLocation
            if ($installLoc) {
                $p = Join-Path $installLoc "ISCC.exe"
                if (Test-Path $p) { return $p }
            }
        }
    }
    return $null
}

function Get-Version {
    if ($Version) { return $Version }
    $cargo = Join-Path $ProjectRoot "winui3_gui\Cargo.toml"
    if (Test-Path $cargo) {
        $content = Get-Content $cargo -Raw
        if ($content -match 'version\s*=\s*"([^"]+)"') {
            return $matches[1]
        }
    }
    return "0.1.0"
}

$iscc = Find-InnoSetup
if (-not $iscc) {
    Write-Host "Error: Inno Setup (ISCC.exe) not found." -ForegroundColor Red
    Write-Host "Please install Inno Setup from https://jrsoftware.org/isdl.php" -ForegroundColor Yellow
    Write-Host "Or set the ISCC_PATH environment variable to the full path of ISCC.exe" -ForegroundColor Yellow
    exit 1
}
Write-Host "Found Inno Setup: $iscc" -ForegroundColor Green

$appVersion = Get-Version
Write-Host "Version: $appVersion"

if (-not $NoBuild) {
    Write-Host ""
    Write-Host "=== Building winui3_gui ($Config) ===" -ForegroundColor Cyan
    Push-Location $ProjectRoot
    cargo build --$Config --package winui3_gui
    if ($LASTEXITCODE -ne 0) {
        Pop-Location
        Write-Host "Build failed!" -ForegroundColor Red
        exit 1
    }
    Pop-Location
}

$exePath = Join-Path $TargetDir "winui3_gui.exe"
if (-not (Test-Path $exePath)) {
    Write-Host "Error: Executable not found at $exePath" -ForegroundColor Red
    exit 1
}
$exeSize = [math]::Round((Get-Item $exePath).Length / 1MB, 2)
Write-Host "Executable: $exePath ($exeSize MB)" -ForegroundColor Green

$dllCount = (Get-ChildItem (Join-Path $TargetDir "*.dll")).Count
Write-Host "Runtime DLLs: $dllCount files" -ForegroundColor Green

# 创建 staging 目录，只复制运行时必需的文件
# 排除 cargo 构建产物（.fingerprint/deps/incremental/build/examples）、
# 调试符号（pdb）、链接器产物（exp/lib）、依赖文件（.d）
# 包含 WinUI 3 必需的 .pri 资源索引和 xx-XX 语言资源目录
$StagingDir = Join-Path $ProjectRoot "target\installer-staging"
if (Test-Path $StagingDir) {
    Remove-Item $StagingDir -Recurse -Force
}
New-Item -ItemType Directory -Path $StagingDir -Force | Out-Null

Write-Host ""
Write-Host "=== Staging runtime files ===" -ForegroundColor Cyan

# 复制根目录下的运行时文件（exe/dll/pri）
$runtimeFiles = Get-ChildItem $TargetDir -File | Where-Object {
    $_.Extension.ToLower() -in ".exe", ".dll", ".pri"
}
foreach ($f in $runtimeFiles) {
    Copy-Item $f.FullName -Destination $StagingDir -Force
}
Write-Host "  Root files: $($runtimeFiles.Count)"

# 复制语言资源子目录（xx-XX 格式，含 .mui 文件）
$langDirs = Get-ChildItem $TargetDir -Directory | Where-Object { $_.Name -match '^[a-z]{2}-' }
foreach ($d in $langDirs) {
    Copy-Item $d.FullName -Destination $StagingDir -Recurse -Force
}
Write-Host "  Language dirs: $($langDirs.Count)"

# 复制 Microsoft.UI.Xaml 框架资源子目录
$xamlDir = Join-Path $TargetDir "Microsoft.UI.Xaml"
if (Test-Path $xamlDir) {
    Copy-Item $xamlDir -Destination $StagingDir -Recurse -Force
    Write-Host "  Microsoft.UI.Xaml: copied"
}

$stagingSize = [math]::Round((Get-ChildItem $StagingDir -Recurse -File | Measure-Object Length -Sum).Sum / 1MB, 2)
Write-Host "  Staging size: $stagingSize MB" -ForegroundColor Green

if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
}

Write-Host ""
Write-Host "=== Building installer ===" -ForegroundColor Cyan
$issFile = Join-Path $InstallerDir "AudioRouter.iss"
Push-Location $InstallerDir
& $iscc "/DMyAppVersion=$appVersion" "/Qp" $issFile
$exitCode = $LASTEXITCODE
Pop-Location

if ($exitCode -ne 0) {
    Write-Host "Installer build failed (exit code: $exitCode)!" -ForegroundColor Red
    exit 1
}

$installerFile = Get-ChildItem $OutputDir -Filter "AudioRouter-Setup-*.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($installerFile) {
    $sizeMB = [math]::Round($installerFile.Length / 1MB, 2)
    Write-Host ""
    Write-Host "=== Build Succeeded ===" -ForegroundColor Green
    Write-Host "Installer: $($installerFile.FullName)"
    Write-Host "Size: $sizeMB MB"
} else {
    Write-Host "Warning: Could not find generated installer in $OutputDir" -ForegroundColor Yellow
}
