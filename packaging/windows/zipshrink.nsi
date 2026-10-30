; ZipShrink setup for Windows 10 and 11 (x64). Installs for the current
; user only, so no administrator rights are needed.
;
; Build: makensis -DVERSION=0.1.0 -DBINDIR=path\to\exes -DOUTFILE=ZipShrink-x64-setup.exe zipshrink.nsi

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef BINDIR
  !define BINDIR "..\..\target\x86_64-pc-windows-gnu\release"
!endif
!ifndef OUTFILE
  !define OUTFILE "ZipShrink-x64-setup.exe"
!endif

!define APP "ZipShrink"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\ZipShrink"

!include "MUI2.nsh"
!include "WinVer.nsh"
!include "x64.nsh"
!include "StrFunc.nsh"
${StrStr}
${UnStrRep}

Name "${APP} ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\ZipShrink"
InstallDirRegKey HKCU "Software\ZipShrink" "InstallDir"
BrandingText "ZipShrink ${VERSION}"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "ZipShrink"
VIAddVersionKey "FileDescription" "ZipShrink Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "MIT License, ZipShrink contributors"

!define MUI_ICON "..\..\assets\zipshrink.ico"
!define MUI_UNICON "..\..\assets\zipshrink.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\zipshrink-gui.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start ZipShrink"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "ZipShrink needs 64-bit Windows 10 or 11."
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP "ZipShrink needs Windows 10 or Windows 11."
    Abort
  ${EndIf}
FunctionEnd

Section "ZipShrink (required)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; A running copy would lock the files; the updater closes the app first.
  File "${BINDIR}\zipshrink-gui.exe"
  File "${BINDIR}\zipshrink.exe"
  File "..\..\LICENSE"
  File "..\..\THIRD_PARTY_LICENSES.md"
  File /oname=README.txt "..\..\README.md"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\ZipShrink.lnk" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr HKCU "Software\ZipShrink" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "ZipShrink"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "ZipShrink contributors"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Explorer right-click menu" SecMenu
  WriteRegStr HKCU "Software\Classes\Directory\shell\ZipShrink" "" "Compress with ZipShrink..."
  WriteRegStr HKCU "Software\Classes\Directory\shell\ZipShrink" "Icon" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr HKCU "Software\Classes\Directory\shell\ZipShrink\command" "" '"$INSTDIR\zipshrink-gui.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\*\shell\ZipShrink" "" "Compress with ZipShrink..."
  WriteRegStr HKCU "Software\Classes\*\shell\ZipShrink" "Icon" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr HKCU "Software\Classes\*\shell\ZipShrink\command" "" '"$INSTDIR\zipshrink-gui.exe" "%1"'
SectionEnd

Section "Add the zipshrink command to PATH" SecPath
  ReadRegStr $0 HKCU "Environment" "Path"
  ${StrStr} $1 "$0" "$INSTDIR"
  ${If} $1 == ""
    ${If} $0 == ""
      WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR"
    ${Else}
      WriteRegExpandStr HKCU "Environment" "Path" "$0;$INSTDIR"
    ${EndIf}
    SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  ${EndIf}
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\ZipShrink.lnk" "$INSTDIR\zipshrink-gui.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The ZipShrink program and the zipshrink command line tool."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMenu} "Adds 'Compress with ZipShrink...' when you right-click a file or folder."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecPath} "Lets you run 'zipshrink' from any command prompt."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Puts a ZipShrink icon on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\zipshrink-gui.exe"
  Delete "$INSTDIR\zipshrink.exe"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.md"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\ZipShrink.lnk"
  Delete "$DESKTOP\ZipShrink.lnk"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\ZipShrink"
  DeleteRegKey HKCU "Software\Classes\*\shell\ZipShrink"
  ReadRegStr $0 HKCU "Environment" "Path"
  ${UnStrRep} $0 "$0" ";$INSTDIR" ""
  ${UnStrRep} $0 "$0" "$INSTDIR;" ""
  ${UnStrRep} $0 "$0" "$INSTDIR" ""
  WriteRegExpandStr HKCU "Environment" "Path" "$0"
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\ZipShrink"
  ; Settings in %APPDATA%\ZipShrink and your archives are left alone.
SectionEnd
