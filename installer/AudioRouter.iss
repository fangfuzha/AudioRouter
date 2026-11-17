; AudioRouter Inno Setup 安装脚本
; 使用方法: ISCC.exe AudioRouter.iss /DMyAppVersion=0.1.0

#define MyAppName "AudioRouter"
#define MyAppPublisher "AudioRouter"
#define MyAppExeName "AudioRouter.exe"
#define MyAppId "{{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}"

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

; 单 exe 架构：托盘、路由、控制面板全在 {#MyAppExeName} 里。

; 安装包文件名后缀：框架依赖变体用 -fd 区分，避免和自包含包重名互相覆盖。
#ifndef MyPackageSuffix
  #define MyPackageSuffix ""
#endif

; 编译期检查：staging 目录里必须有常驻服务 exe。
; 缺了就直接报错，避免装出一个「面板找不到服务」的残缺安装包。
; staging 目录由 scripts\build-installer.ps1 生成。
; ISPP 的 FileExists 用相对路径时以脚本所在目录为基准。
; 注意：这里写死文件名而不是用 {#MyAppExeName} —— ISPP 在 #if 条件表达式里
; 不一定会展开字符串宏，写死最稳。改 exe 名字时这里要同步改。
#if FileExists("..\target\installer-staging\AudioRouter.exe")
  ; 主 exe 已就位
#else
  #error 缺少 AudioRouter.exe：请先运行 scripts\build-installer.ps1 生成 staging 目录
#endif

; 编译期检查：部署模式对应的运行时文件必须齐。
;
; 部署模式不靠宏传递，而是看 staging 里的文件形态（两种形态互斥）：
; - 自包含：带整套 WindowsAppRuntime 运行时（Microsoft.WindowsAppRuntime.dll 等）
; - 框架依赖：只带 Microsoft.WindowsAppRuntime.Bootstrap.dll，运行时用系统上的包
; ISPP 的 == 只能比较字符串，宏比较不可靠，用文件存在性判断更稳。
;
; 自包含模式缺任何一个运行时文件都会导致「窗口创建失败后静默退出」——
; 这正是 v0.3.12~v0.3.16 坏包的成因，所以宁可编译失败也不能出坏包。
#if FileExists("..\target\installer-staging\Microsoft.WindowsAppRuntime.Bootstrap.dll")
  ; 框架依赖模式
  #if FileExists("..\target\installer-staging\Microsoft.WindowsAppRuntime.dll")
    #error 部署模式冲突：staging 里同时有 bootstrapper 和运行时 DLL
  #endif
#else
  ; 自包含模式
  #if FileExists("..\target\installer-staging\Microsoft.UI.Xaml.Controls.dll")
  #else
    #error 缺少 Microsoft.UI.Xaml.Controls.dll：请先运行 scripts\build-installer.ps1 生成 staging 目录
  #endif
  #if FileExists("..\target\installer-staging\Microsoft.ui.xaml.dll")
  #else
    #error 缺少 Microsoft.ui.xaml.dll：请先运行 scripts\build-installer.ps1 生成 staging 目录
  #endif
  #if FileExists("..\target\installer-staging\Microsoft.WindowsAppRuntime.dll")
  #else
    #error 缺少 Microsoft.WindowsAppRuntime.dll：请先运行 scripts\build-installer.ps1 生成 staging 目录
  #endif
  #if FileExists("..\target\installer-staging\resources.pri")
  #else
    #error 缺少 resources.pri：请先运行 scripts\build-installer.ps1 生成 staging 目录
  #endif
#endif

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
OutputDir=Output
OutputBaseFilename=AudioRouter-Setup-{#MyAppVersion}-x64{#MyPackageSuffix}
Compression=lzma2/ultra
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#MyAppExeName}
AppCopyright=Copyright (C) 2026
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
; 中文是 Inno Setup 官方语言包,但 Chocolatey 安装的 Inno Setup 6
; 不包含 Languages\ 子目录下的语言文件,因此把 ChineseSimplified.isl
; 随项目一起打包,用相对路径引用。英文用编译器自带的 Default.isl。
Name: "chinesesimplified"; MessagesFile: "Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; 从 staging 目录复制所有运行时文件。
; staging 目录由 build-installer.ps1 预先准备，内容随部署模式变化：
; - 自包含：WindowsAppRuntime 的 dll/pri（XAML 资源索引，WinUI 3 必需）
;   + xx-XX\*.mui（语言资源）+ Microsoft.UI.Xaml\Assets\*（框架资源）
; - 框架依赖：只有 Microsoft.WindowsAppRuntime.Bootstrap.dll + resources.pri，
;   运行时由系统上已安装的 WindowsAppRuntime 包提供
;
; 注意：.pri 文件是 WinUI 3 必需的 XAML 资源索引，缺少会导致窗口创建
; 失败后静默退出。.mui 文件是框架多语言资源，按系统语言加载。
;
; 整个 staging 目录一次性装进去：主 exe + WindowsAppRuntime 运行时文件。
Source: "..\target\installer-staging\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

; 应用资源文件（图标等）
Source: "..\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; 快捷方式显式指定 IconFilename，避免某些情况下 Windows 缓存 exe 默认图标。
; IconFilename 指向安装目录里随包发布的 assets/icon.ico。
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\assets\icon.ico"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\assets\icon.ico"; Tasks: desktopicon
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"

[UninstallRun]
; 卸载前先结束进程：正在运行的 exe 无法被删除。
; 旧版本（拆分进程时代）的两个 exe 也要结束，否则它们会留在后台，
; 继续占着托盘图标和音频设备。
Filename: "taskkill"; Parameters: "/F /IM {#MyAppExeName}"; Flags: runhidden; RunOnceId: "StopAudioRouter"
Filename: "taskkill"; Parameters: "/F /IM audiorouter_service.exe"; Flags: runhidden; RunOnceId: "StopLegacyService"
Filename: "taskkill"; Parameters: "/F /IM winui3_gui.exe"; Flags: runhidden; RunOnceId: "StopLegacyPanel"

[InstallDelete]
; 升级时清掉旧版本遗留的两个 exe（拆分进程时代的产物）。
Type: files; Name: "{app}\audiorouter_service.exe"
Type: files; Name: "{app}\winui3_gui.exe"

[Run]
; 注意：不能使用 skipifsilent，否则 /VERYSILENT 静默安装（自动更新场景）
; 不会在安装完成后启动应用，用户会以为"安装重启"只是关闭了软件。
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall
