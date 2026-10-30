; ZipShrink setup for Windows 95 through Windows 11.
;
; Built as an ANSI installer (Unicode false): NSIS's Unicode installers need
; Windows 2000. On Windows 2000 and later it installs for the current user
; (no administrator rights needed), into the same folder earlier versions
; used, so it upgrades them in place. On Windows 95, 98, Me and NT 4, which
; have no per-user program folders, it installs into Program Files.
;
; Build: makensis -DVERSION=0.2.0 -DBINDIR=path\to\exes -DOUTFILE=ZipShrink-setup.exe zipshrink.nsi

Unicode false
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestSupportedOS all

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef BINDIR
  !define BINDIR "..\..\build"
!endif
!ifndef OUTFILE
  !define OUTFILE "ZipShrink-setup.exe"
!endif

!define APP "ZipShrink"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\ZipShrink"

!include "MUI2.nsh"
!include "WinVer.nsh"
!include "LogicLib.nsh"
!include "Sections.nsh"
!include "StrFunc.nsh"
${StrStr}
${UnStrRep}

Name "${APP} ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES\ZipShrink"
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

Var PerUser ; 1 on Windows 2000 and later

!macro DetectScope
  StrCpy $PerUser 0
  ${If} ${IsNT}
  ${AndIf} ${AtLeastWin2000}
    StrCpy $PerUser 1
  ${EndIf}
  ${If} $PerUser == 1
    SetShellVarContext current
  ${Else}
    SetShellVarContext all
  ${EndIf}
!macroend

Function un.onInit
  !insertmacro DetectScope
FunctionEnd

Section "ZipShrink (required)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; A running copy would lock the files; the updater closes the app first.
  File "${BINDIR}\zipshrink-gui.exe"
  File "${BINDIR}\zipshrink.exe"
  File /oname=LICENSE.txt "..\..\LICENSE"
  File /oname=THIRD_PARTY_LICENSES.txt "..\..\THIRD_PARTY_LICENSES.md"
  File /oname=README.txt "..\..\README.md"
  ; Left over from the 0.1 series.
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.md"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\ZipShrink.lnk" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr SHCTX "Software\ZipShrink" "InstallDir" "$INSTDIR"
  WriteRegStr SHCTX "${UNINST_KEY}" "DisplayName" "ZipShrink"
  WriteRegStr SHCTX "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINST_KEY}" "Publisher" "ZipShrink contributors"
  WriteRegStr SHCTX "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr SHCTX "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr SHCTX "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr SHCTX "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD SHCTX "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD SHCTX "${UNINST_KEY}" "NoRepair" 1
SectionEnd

!macro MenuEntry ROOT KEY
  WriteRegStr ${ROOT} "${KEY}\ZipShrink" "" "Compress with ZipShrink..."
  WriteRegStr ${ROOT} "${KEY}\ZipShrink" "Icon" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr ${ROOT} "${KEY}\ZipShrink\command" "" '"$INSTDIR\zipshrink-gui.exe" "%1"'
!macroend

Section "Explorer right-click menu" SecMenu
  ; Per-user file classes exist from Windows 2000 on.
  ${If} $PerUser == 1
    !insertmacro MenuEntry HKCU "Software\Classes\Directory\shell"
    !insertmacro MenuEntry HKCU "Software\Classes\*\shell"
  ${Else}
    !insertmacro MenuEntry HKCR "Directory\shell"
    !insertmacro MenuEntry HKCR "*\shell"
  ${EndIf}
SectionEnd

Section "Add the zipshrink command to PATH" SecPath
  ${If} $PerUser == 1
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
  ${Else}
    ; Windows NT 4: the system path needs an administrator; say so instead.
    DetailPrint "Add $INSTDIR to PATH in Control Panel > System > Environment to use zipshrink from any prompt."
  ${EndIf}
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\ZipShrink.lnk" "$INSTDIR\zipshrink-gui.exe"
SectionEnd

; After the sections, so their indexes are defined.
Function .onInit
  !insertmacro DetectScope
  ; An earlier installation decides the folder (upgrade in place).
  ReadRegStr $0 SHCTX "Software\ZipShrink" "InstallDir"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${ElseIf} $PerUser == 1
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\ZipShrink"
  ${EndIf}
  ; Windows 9x keeps PATH in AUTOEXEC.BAT, which setup does not edit.
  ${IfNot} ${IsNT}
    !insertmacro UnselectSection ${SecPath}
    !insertmacro SetSectionFlag ${SecPath} ${SF_RO}
  ${EndIf}
FunctionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The ZipShrink program and the zipshrink command line tool."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMenu} "Adds 'Compress with ZipShrink...' when you right-click a file or folder."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecPath} "Lets you run 'zipshrink' from any command prompt (Windows 2000 and later)."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Puts a ZipShrink icon on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\zipshrink-gui.exe"
  Delete "$INSTDIR\zipshrink.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.txt"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\ZipShrink.lnk"
  Delete "$DESKTOP\ZipShrink.lnk"
  ${If} $PerUser == 1
    DeleteRegKey HKCU "Software\Classes\Directory\shell\ZipShrink"
    DeleteRegKey HKCU "Software\Classes\*\shell\ZipShrink"
    ReadRegStr $0 HKCU "Environment" "Path"
    ${UnStrRep} $0 "$0" ";$INSTDIR" ""
    ${UnStrRep} $0 "$0" "$INSTDIR;" ""
    ${UnStrRep} $0 "$0" "$INSTDIR" ""
    WriteRegExpandStr HKCU "Environment" "Path" "$0"
    SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  ${Else}
    DeleteRegKey HKCR "Directory\shell\ZipShrink"
    DeleteRegKey HKCR "*\shell\ZipShrink"
  ${EndIf}
  DeleteRegKey SHCTX "${UNINST_KEY}"
  DeleteRegKey SHCTX "Software\ZipShrink"
  ; Settings in HKEY_CURRENT_USER\Software\ZipShrink go with the key above
  ; on Windows 2000 and later; your archives are never touched.
SectionEnd
