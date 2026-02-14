!macro NSIS_HOOK_POSTUNINSTALL
  ; 卸载最后时机执行删除,避免占用
  ; 默认：卸载时删除用户数据目录（递归删除）
  
  ; 获取当前用户的 Local AppData 路径（从注册表读取更稳健）
  ReadRegStr $0 HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Shell Folders" "Local AppData"

  StrCpy $1 "$0\\com.github.fangfuzha.AudioRouter"
  IfFileExists "$1\\*" 0 +2
    RMDir /r "$1"

  ; 若需要可在此处删除更多临时/缓存路径或注册表项
!macroend