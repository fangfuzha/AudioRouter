param(
    [string]$Version = "",
    [string]$Config = "release",
    [switch]$NoBuild = $false,
    [switch]$SkipSign = $false,
    # 部署模式：
    #   auto                —— 从已构建的 exe 探测（内嵌 app.manifest 即自包含）
    #   self-contained      —— 运行时 DLL 随安装包发布（默认发布形态）
    #   framework-dependent —— 只带 bootstrapper，运行时用系统已装的 WindowsAppRuntime
    [ValidateSet("auto", "self-contained", "framework-dependent")]
    [string]$DeploymentMode = "auto",
    # WindowsAppRuntime 版本；留空则读 installer\runtime-version.txt
    [string]$RuntimeVersion = "",
    # 只用本地缓存，不联网下载运行时
    [switch]$OfflineRuntime = $false,
    # 自定义 cargo target 目录（例如 CI 给框架依赖变体单独一个目录）
    [string]$CargoTargetDir = ""
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$InstallerDir = Join-Path $ProjectRoot "installer"
$OutputDir = Join-Path $InstallerDir "Output"
$StagingDir = Join-Path $ProjectRoot "target\installer-staging"
$RuntimeCacheDir = Join-Path $env:LOCALAPPDATA "AudioRouter\cache"
$RuntimePkgName = "Microsoft.WindowsAppSDK.Runtime"

if ($CargoTargetDir) {
    # 相对路径按项目根解析，避免受调用方当前目录影响
    if (-not [System.IO.Path]::IsPathRooted($CargoTargetDir)) {
        $CargoTargetDir = Join-Path $ProjectRoot $CargoTargetDir
    }
    $env:CARGO_TARGET_DIR = $CargoTargetDir
    $TargetDir = Join-Path $CargoTargetDir $Config
} else {
    $TargetDir = Join-Path $ProjectRoot "target\$Config"
}

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
        "E:\Program Files\Inno Setup 6\ISCC.exe",
        # winget 默认安装到用户目录
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
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

function Get-RuntimeVersion {
    if ($RuntimeVersion) { return $RuntimeVersion.Trim() }
    $file = Join-Path $InstallerDir "runtime-version.txt"
    if (Test-Path $file) {
        $v = (Get-Content $file -Raw).Trim()
        if ($v) { return $v }
    }
    Write-Host "Error: 找不到 WindowsAppRuntime 版本文件 $file" -ForegroundColor Red
    exit 1
}

# 读取运行时清单（installer\runtime-files.txt），返回 @{小写名 = 原始名}。
# 清单里既有文件（Microsoft.UI.Xaml.Controls.dll）也有目录（Microsoft.UI.Xaml、
# af-ZA 这些语言资源目录）。条目名里带不带点不能用来区分文件/目录 ——
# Microsoft.UI.Xaml 本身就带点 —— 所以这里只做"名字集合"，
# 文件还是目录由 MSIX 里的实际形态决定。
function Get-RuntimeManifest {
    $file = Join-Path $InstallerDir "runtime-files.txt"
    if (-not (Test-Path $file)) {
        Write-Host "Error: 找不到运行时清单 $file" -ForegroundColor Red
        exit 1
    }
    $all = @{}
    $required = @{}
    $section = "required"
    foreach ($line in Get-Content $file) {
        $t = $line.Trim()
        if (-not $t -or $t.StartsWith("#")) { continue }
        if ($t -match '^=== (.+) ===$') {
            $section = $matches[1].ToLowerInvariant()
            continue
        }
        $key = $t.ToLowerInvariant()
        $all[$key] = $t
        if ($section -ne "optional") { $required[$key] = $t }
    }
    if ($all.Count -eq 0 -or $required.Count -eq 0) {
        Write-Host "Error: 运行时清单 $file 内容为空" -ForegroundColor Red
        exit 1
    }
    return @{ All = $all; Required = $required }
}

# 找一个已经下载好的 nupkg。
# 上游 windows-reactor-setup 把 nupkg 放在 %LOCALAPPDATA%\windows-reactor-setup\temp，
# 文件名规则一致，直接复用即可，不必重复下载 159MB。
function Find-CachedNupkg {
    $name = "$RuntimePkgName.$RuntimeVersion.nupkg"
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "windows-reactor-setup\temp\$name"),
        (Join-Path $RuntimeCacheDir $name)
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) { return $c }
    }
    return $null
}

function Save-Nupkg {
    $dest = Join-Path $RuntimeCacheDir "$RuntimePkgName.$RuntimeVersion.nupkg"
    New-Item -ItemType Directory -Path $RuntimeCacheDir -Force | Out-Null
    if (Test-Path $dest) { Remove-Item $dest -Force }
    $urls = @(
        "https://api.nuget.org/v3-flatcontainer/microsoft.windowsappsdk.runtime/$RuntimeVersion/microsoft.windowsappsdk.runtime.$RuntimeVersion.nupkg",
        "https://www.nuget.org/api/v2/package/$RuntimePkgName/$RuntimeVersion"
    )
    $curl = Join-Path $env:SystemRoot "System32\curl.exe"
    if (-not (Test-Path $curl)) {
        Write-Host "Error: 找不到 curl.exe，无法下载运行时" -ForegroundColor Red
        return $null
    }
    foreach ($u in $urls) {
        Write-Host "  下载 $u"
        & $curl -s -L --fail --retry 3 --retry-delay 2 -o $dest $u
        if ($LASTEXITCODE -eq 0 -and (Test-Path $dest) -and (Get-Item $dest).Length -gt 10MB) {
            Write-Host ("  已下载 {0:N1} MB" -f ((Get-Item $dest).Length / 1MB)) -ForegroundColor Green
            return $dest
        }
        Write-Host "  下载失败（curl exit=$LASTEXITCODE），换下一个源" -ForegroundColor Yellow
    }
    if (Test-Path $dest) { Remove-Item $dest -Force }
    return $null
}

# 从 nupkg 里取出 x64 的 MSIX（不整体解包，只取需要的那一个条目）。
function Get-MsixFromNupkg {
    param([string]$NupkgPath, [string]$WorkDir)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null
    $zip = [System.IO.Compression.ZipFile]::OpenRead($NupkgPath)
    try {
        $entry = $zip.Entries |
            Where-Object { $_.FullName -match 'MSIX/win10-x64/Microsoft\.WindowsAppRuntime\.2\.msix$' } |
            Select-Object -First 1
        if (-not $entry) { return $null }
        $msix = Join-Path $WorkDir "Microsoft.WindowsAppRuntime.2.msix"
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $msix, $true)
        return $msix
    } finally {
        $zip.Dispose()
    }
}

# 按清单从 MSIX 里抽取条目到 staging。
# MSIX 是 zip，顶层文件名大小写混合（af-ZA / ca-Es-VALENCIA），比对时忽略大小写。
function Expand-MsixToStaging {
    param([string]$MsixPath, [hashtable]$Manifest, [string]$DestDir)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $copied = @{}
    $zip = [System.IO.Compression.ZipFile]::OpenRead($MsixPath)
    try {
        foreach ($entry in $zip.Entries) {
            if ([string]::IsNullOrEmpty($entry.Name)) { continue }
            $parts = $entry.FullName -split '/'
            $top = $parts[0]
            $key = $top.ToLowerInvariant()
            if (-not $Manifest.All.ContainsKey($key)) { continue }
            $outFile = Join-Path $DestDir ($parts -join '\')
            New-Item -ItemType Directory -Path (Split-Path $outFile -Parent) -Force | Out-Null
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $outFile, $true)
            $copied[$key] = $true
        }
    } finally {
        $zip.Dispose()
    }
    return $copied
}

# 读上游 windows-reactor-setup 里固定的运行时版本（RUNTIME_VER 常量）。
# exe 内嵌的 app.manifest 和 Rust 侧 WinRT 绑定都是按这个版本生成的，
# 打包的运行时应当和它保持一致。
function Get-UpstreamRuntimeVersion {
    $setupDir = Get-ReactorSetupDir
    if (-not $setupDir) { return $null }
    $lib = Join-Path $setupDir "src\lib.rs"
    if (-not (Test-Path $lib)) { return $null }
    $m = Select-String -Path $lib -Pattern 'RUNTIME_VER:\s*&str\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($m -and $m.Matches.Count -gt 0) { return $m.Matches[0].Groups[1].Value }
    return $null
}

# 自包含模式：准备 WindowsAppRuntime 运行时。
# 任何一步失败都直接 exit 1 —— 宁可打包失败，也不能产出一个装完缺文件的坏包。
function Install-SelfContainedRuntime {
    param([string]$DestDir)
    $manifest = Get-RuntimeManifest

    # 版本一致性检查：打包的运行时要和上游 pin 一致。
    # 不一致不会立刻坏（2.6.5 实测能跑），但 exe 的 app.manifest、Rust 绑定、
    # 以及 cargo run 用的 target/release 里的运行时都还是上游那版，
    # 开发环境和产品环境会跑两套运行时。
    $upstreamVer = Get-UpstreamRuntimeVersion
    if ($upstreamVer -and $upstreamVer -ne $RuntimeVersion) {
        Write-Host "警告：打包的运行时 $RuntimeVersion 与上游 windows-reactor-setup 固定的 $upstreamVer 不一致" -ForegroundColor Yellow
        Write-Host "      exe 内嵌的 app.manifest 和 Rust 侧 WinRT 绑定仍按 $upstreamVer 生成，" -ForegroundColor Yellow
        Write-Host "      且 cargo run 用的 target\$Config 里仍是 $upstreamVer 的运行时。" -ForegroundColor Yellow
        Write-Host "      若要换版本，请先升级 windows-rs pin（绑定与 manifest 一起重新生成），再改 installer\runtime-version.txt。" -ForegroundColor Yellow
    }

    $nupkg = Find-CachedNupkg
    if ($nupkg) {
        Write-Host "  复用本地缓存: $nupkg" -ForegroundColor Green
    } else {
        if ($OfflineRuntime) {
            Write-Host "Error: 本地没有 WindowsAppRuntime $RuntimeVersion 的 nupkg，且指定了 -OfflineRuntime" -ForegroundColor Red
            exit 1
        }
        $nupkg = Save-Nupkg
        if (-not $nupkg) {
            Write-Host "Error: 下载 WindowsAppRuntime $RuntimeVersion 失败" -ForegroundColor Red
            exit 1
        }
    }

    $work = Join-Path $RuntimeCacheDir "$RuntimePkgName-$RuntimeVersion"
    $msix = Get-MsixFromNupkg -NupkgPath $nupkg -WorkDir $work
    if (-not $msix -or -not (Test-Path $msix)) {
        Write-Host "Error: nupkg 里找不到 x64 的 Microsoft.WindowsAppRuntime.2.msix" -ForegroundColor Red
        exit 1
    }
    Write-Host "  MSIX: $msix"

    $copied = Expand-MsixToStaging -MsixPath $msix -Manifest $manifest -DestDir $DestDir
    Write-Host ("  已抽取 {0} 个条目" -f $copied.Count)

    # 清单里的每个条目都必须拿到，否则说明版本和清单对不上。
    # required 段缺任何一个都要失败；optional 段（语言资源目录）缺失只提示。
    $missing = @()
    foreach ($k in $manifest.Required.Keys) {
        if (-not $copied.ContainsKey($k)) { $missing += $manifest.Required[$k] }
    }
    if ($missing.Count -gt 0) {
        Write-Host "Error: WindowsAppRuntime $RuntimeVersion 的 MSIX 里缺少以下必需条目：" -ForegroundColor Red
        foreach ($m in $missing) { Write-Host "    - $m" -ForegroundColor Red }
        Write-Host "请更新 installer\runtime-files.txt 或 installer\runtime-version.txt 后重试。" -ForegroundColor Yellow
        exit 1
    }
    $missingOptional = @()
    foreach ($k in $manifest.All.Keys) {
        if (-not $copied.ContainsKey($k)) { $missingOptional += $manifest.All[$k] }
    }
    if ($missingOptional.Count -gt 0) {
        Write-Host ("  提示：该版本没有以下可选条目（语言资源，不影响运行）：{0}" -f ($missingOptional -join ", ")) -ForegroundColor Yellow
    }

    # 关键文件再单独点名检查一次，错误信息更直白。
    $critical = @(
        "microsoft.ui.xaml.controls.dll",
        "microsoft.ui.xaml.dll",
        "microsoft.windowsappruntime.dll",
        "microsoft.ui.dll",
        "microsoft.ui.input.dll",
        "microsoft.ui.windowing.dll",
        "microsoft.ui.composition.ossupport.dll",
        "coremessagingxp.dll",
        "wuceffectsi.dll",
        "sessionhandleipcproxystub.dll",
        "microsoft.ui.pri",
        "microsoft.ui.xaml.controls.pri",
        "microsoft.windowsappruntime.pri",
        "resources.pri"
    )
    foreach ($c in $critical) {
        $real = $manifest.All[$c]
        if (-not $real -or -not (Test-Path (Join-Path $DestDir $real))) {
            Write-Host "Error: 运行时关键文件缺失: $c" -ForegroundColor Red
            exit 1
        }
    }
    Write-Host "  运行时关键文件校验通过" -ForegroundColor Green
}

# 定位上游 windows-reactor-setup crate 的源码目录。
# 用 cargo metadata 拿准确的路径，拿不到再退回 cargo git checkout 的目录扫描。
function Get-ReactorSetupDir {
    Push-Location $ProjectRoot
    try {
        $raw = & cargo metadata --format-version 1 2>$null | Out-String
        if ($LASTEXITCODE -eq 0 -and $raw) {
            $meta = $raw | ConvertFrom-Json
            $pkg = $meta.packages | Where-Object { $_.name -eq "windows-reactor-setup" } | Select-Object -First 1
            if ($pkg -and $pkg.manifest_path) {
                $dir = Split-Path $pkg.manifest_path -Parent
                if (Test-Path $dir) { return $dir }
            }
        }
    } catch {
        Write-Host "  cargo metadata 失败，退回目录扫描" -ForegroundColor Yellow
    } finally {
        Pop-Location
    }
    $root = Join-Path $env:USERPROFILE ".cargo\git\checkouts"
    if (Test-Path $root) {
        $hit = Get-ChildItem $root -Directory -Filter "windows-rs-*" -ErrorAction SilentlyContinue |
            ForEach-Object { Get-ChildItem $_.FullName -Directory -ErrorAction SilentlyContinue } |
            ForEach-Object { Join-Path $_.FullName "crates\libs\reactor-setup" } |
            Where-Object { Test-Path (Join-Path $_ "bootstrap\x64\Microsoft.WindowsAppRuntime.Bootstrap.dll") } |
            Select-Object -First 1
        if ($hit) { return $hit }
    }
    return $null
}

# 框架依赖模式：只放 bootstrapper 和应用自己的 resources.pri，
# 运行时由系统上已安装的 WindowsAppRuntime 包提供。
# bootstrapper 从上游 crate 源码里取，不依赖 build script 的副作用
# （sccache 命中时 build script 根本不跑，副作用会丢）。
function Install-FrameworkDependentRuntime {
    param([string]$DestDir)
    $setupDir = Get-ReactorSetupDir
    if (-not $setupDir) {
        Write-Host "Error: 找不到 windows-reactor-setup crate 源码，无法取得 bootstrapper" -ForegroundColor Red
        exit 1
    }
    Write-Host "  reactor-setup: $setupDir"

    $bootstrap = Join-Path $setupDir "bootstrap\x64\Microsoft.WindowsAppRuntime.Bootstrap.dll"
    if (-not (Test-Path $bootstrap)) {
        Write-Host "Error: 找不到 bootstrapper: $bootstrap" -ForegroundColor Red
        exit 1
    }
    Copy-Item $bootstrap -Destination (Join-Path $DestDir "Microsoft.WindowsAppRuntime.Bootstrap.dll") -Force

    $resPri = Join-Path $setupDir "assets\resources.pri"
    if (Test-Path $resPri) {
        Copy-Item $resPri -Destination (Join-Path $DestDir "resources.pri") -Force
    }

    foreach ($required in @("Microsoft.WindowsAppRuntime.Bootstrap.dll", "resources.pri")) {
        if (-not (Test-Path (Join-Path $DestDir $required))) {
            Write-Host "Error: 框架依赖模式缺少 $required" -ForegroundColor Red
            exit 1
        }
    }
    Write-Host "  bootstrapper 就位" -ForegroundColor Green
}

# 探测已构建 exe 的部署模式。
# 自包含模式的 exe 内嵌了 app.manifest（asmv3:file 声明了必须就地加载的 DLL）；
# 框架依赖模式的 exe 没有这个清单。
function Get-ExeDeploymentMode {
    param([string]$ExePath)
    $bytes = [System.IO.File]::ReadAllBytes($ExePath)
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    if ($text.Contains('asmv3:file name="Microsoft.UI.Xaml.Controls.dll"')) {
        return "self-contained"
    }
    if ($text.Contains("asmv3:file")) {
        return "self-contained"
    }
    return "framework-dependent"
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
    Write-Host "=== Building winui3_gui + audiorouter_service ($Config) ===" -ForegroundColor Cyan
    Push-Location $ProjectRoot
    # 控制面板与常驻服务是两个进程，必须一起构建：服务 exe 要和 GUI exe
    # 放在同一个目录里（服务靠同目录找到 GUI，面板靠同目录找到服务）。
    # --locked 与 CI 保持一致：Cargo.lock 必须与各 manifest 一致
    cargo build --locked --$Config --package winui3_gui --package audiorouter_service
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

$serviceExePath = Join-Path $TargetDir "audiorouter_service.exe"
if (-not (Test-Path $serviceExePath)) {
    Write-Host "Error: Service executable not found at $serviceExePath" -ForegroundColor Red
    exit 1
}
$serviceSize = [math]::Round((Get-Item $serviceExePath).Length / 1MB, 2)
Write-Host "Service executable: $serviceExePath ($serviceSize MB)" -ForegroundColor Green

# 决定部署模式
$detectedMode = Get-ExeDeploymentMode -ExePath $exePath
if ($DeploymentMode -eq "auto") {
    $resolvedMode = $detectedMode
    Write-Host "Deployment mode (auto detected): $resolvedMode" -ForegroundColor Green
} else {
    $resolvedMode = $DeploymentMode
    if ($detectedMode -ne $resolvedMode) {
        Write-Host "Error: 指定的部署模式 '$resolvedMode' 与 exe 实际形态 '$detectedMode' 不一致" -ForegroundColor Red
        Write-Host "框架依赖变体必须用 cargo build --features framework-dependent 编译；自包含变体不要加这个特性。" -ForegroundColor Yellow
        exit 1
    }
    Write-Host "Deployment mode: $resolvedMode" -ForegroundColor Green
}

Write-Host ""
Write-Host "=== Staging runtime files ===" -ForegroundColor Cyan

# 创建 staging 目录，只放这次真正要发布的东西。
# 运行时文件一律从 WindowsAppSDK 的 nupkg/MSIX 里按清单抽取，
# 绝不从 target 目录里"顺带"拷 —— 那正是 v0.3.12~v0.3.16 漏装运行时的根因
# （build script 的副作用在 sccache 命中时消失）。
if (Test-Path $StagingDir) {
    Remove-Item $StagingDir -Recurse -Force
}
New-Item -ItemType Directory -Path $StagingDir -Force | Out-Null

Copy-Item $exePath -Destination $StagingDir -Force
Copy-Item $serviceExePath -Destination $StagingDir -Force
Write-Host "  Exes: winui3_gui.exe, audiorouter_service.exe"

if ($resolvedMode -eq "framework-dependent") {
    Install-FrameworkDependentRuntime -DestDir $StagingDir
} else {
    $RuntimeVersion = Get-RuntimeVersion
    Write-Host "  WindowsAppRuntime version: $RuntimeVersion" -ForegroundColor Green
    Install-SelfContainedRuntime -DestDir $StagingDir
}

$stagedFiles = Get-ChildItem $StagingDir -File
$stagedDirs = Get-ChildItem $StagingDir -Directory
Write-Host ("  Staged files: {0}" -f $stagedFiles.Count)
Write-Host ("  Staged dirs : {0}" -f $stagedDirs.Count)
$stagingSize = [math]::Round((Get-ChildItem $StagingDir -Recurse -File | Measure-Object Length -Sum).Sum / 1MB, 2)
Write-Host "  Staging size: $stagingSize MB" -ForegroundColor Green

# 打包前的最后校验：两个可执行文件必须都在 staging 里。
foreach ($required in @("winui3_gui.exe", "audiorouter_service.exe")) {
    $requiredPath = Join-Path $StagingDir $required
    if (-not (Test-Path $requiredPath)) {
        Write-Host "Error: $required missing from staging dir $StagingDir" -ForegroundColor Red
        exit 1
    }
}

if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
}

Write-Host ""
Write-Host "=== Building installer ===" -ForegroundColor Cyan
$issFile = Join-Path $InstallerDir "AudioRouter.iss"
$packageSuffix = if ($resolvedMode -eq "framework-dependent") { "-fd" } else { "" }
Push-Location $InstallerDir
# 部署模式不通过宏传给 ISPP：AudioRouter.iss 直接看 staging 里的文件形态判断，
# 两种形态互斥，比宏比较可靠。
& $iscc "/DMyAppVersion=$appVersion" "/DMyPackageSuffix=$packageSuffix" "/Qp" $issFile
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
    Write-Host "Mode: $resolvedMode"
} else {
    Write-Host "Warning: Could not find generated installer in $OutputDir" -ForegroundColor Yellow
}
