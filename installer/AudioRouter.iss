; AudioRouter Inno Setup 安装脚本
; 使用方法: ISCC.exe AudioRouter.iss /DMyAppVersion=0.1.0

#define MyAppName "AudioRouter"
#define MyAppPublisher "AudioRouter"
#define MyAppExeName "winui3_gui.exe"
#define MyAppId "{{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}"

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
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
OutputBaseFilename=AudioRouter-Setup-{#MyAppVersion}-x64
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
; WinUI 3 自包含部署：从 staging 目录复制所有运行时文件
; staging 目录由 build-installer.ps1 预先准备，只包含运行时必需的文件：
; - exe/dll/pri（XAML 资源索引，WinUI 3 必需）
; - xx-XX\*.mui（语言资源）
; - Microsoft.UI.Xaml\Assets\*（框架资源）
;
; 注意：.pri 文件是 WinUI 3 必需的 XAML 资源索引，缺少会导致窗口创建
; 失败后静默退出。.mui 文件是框架多语言资源，按系统语言加载。
Source: "..\target\installer-staging\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

; 应用资源文件（图标等）
Source: "..\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
