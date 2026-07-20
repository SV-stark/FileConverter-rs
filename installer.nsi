!define APP_NAME "FileConverter"
!define APP_VERSION "0.1.0"
!define APP_PUBLISHER "SV-stark"
!define APP_WEBSITE "https://github.com/SV-stark/FileConverter-rs"

Unicode True
RequestExecutionLevel admin

InstallDir "$PROGRAMFILES64\${APP_NAME}"
InstallDirRegKey HKLM "Software\${APP_NAME}" "InstallDir"

Name "${APP_NAME}"
OutFile "FileConverter_Setup.exe"

!include "MUI2.nsh"

!define MUI_ABORTWARNING

; Pages
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Install"
    SetRegView 64
    SetOutPath "$INSTDIR"
    
    ; Copy build outputs
    File "target\release\file_converter_bin.exe"
    File "target\release\file_converter_shell.dll"
    
    ; Register shell extension DLL
    RegDLL "$INSTDIR\file_converter_shell.dll"
    
    ; Create uninstaller
    WriteUninstaller "$INSTDIR\uninstall.exe"
    
    ; Add uninstall registry keys
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "DisplayName" "${APP_NAME}"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "Publisher" "${APP_PUBLISHER}"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "URLInfoAbout" "${APP_WEBSITE}"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "DisplayVersion" "${APP_VERSION}"
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "NoModify" 1
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "NoRepair" 1
SectionEnd

Section "Uninstall"
    SetRegView 64
    ; Unregister shell DLL
    UnRegDLL "$INSTDIR\file_converter_shell.dll"
    
    ; Delete files
    Delete "$INSTDIR\file_converter_bin.exe"
    Delete "$INSTDIR\file_converter_shell.dll"
    Delete "$INSTDIR\uninstall.exe"
    
    RMDir "$INSTDIR"
    
    ; Remove registry keys
    DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
    DeleteRegKey HKLM "Software\${APP_NAME}"
SectionEnd
