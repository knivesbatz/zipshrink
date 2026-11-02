; ZipShrink setup for Windows 95 through Windows 11.
;
; Built as an ANSI installer (Unicode false): NSIS's Unicode installers need
; Windows 2000. On Windows 2000 and later it installs for the current user
; (no administrator rights needed), into the same folder earlier versions
; used, so it upgrades them in place. On Windows 95, 98, Me and NT 4, which
; have no per-user program folders, it installs into Program Files.
;
; ZipShrink's updater runs it as `ZipShrink-setup.exe /S [/RESTART]`: silent,
; it waits for ZipShrink to close, keeps the choices of the earlier
; installation, and with /RESTART starts ZipShrink again afterwards.
;
; Build: makensis -DVERSION=0.4.0 -DBINDIR=path\to\exes -DOUTFILE=ZipShrink-setup.exe zipshrink.nsi

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
; The Windows Installer package of versions 0.1 to 0.3.
!define MSI_UPGRADE_CODE "{6B0E2F9A-3C41-4E7B-9F0D-2A5C8E1B7D34}"

!include "MUI2.nsh"
!include "WinVer.nsh"
!include "LogicLib.nsh"
!include "Sections.nsh"
!include "StrFunc.nsh"
!include "FileFunc.nsh"
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

; Waits up to 30 seconds for a running copy of FILE to close.
!macro WaitUnlocked FILE
  StrCpy $R1 0
  ${Do}
    ${IfNot} ${FileExists} "${FILE}"
      ${Break}
    ${EndIf}
    ClearErrors
    FileOpen $R0 "${FILE}" a
    ${IfNot} ${Errors}
      FileClose $R0
      ${Break}
    ${EndIf}
    IntOp $R1 $R1 + 1
    ${If} $R1 >= 60
      ${Break}
    ${EndIf}
    Sleep 500
  ${Loop}
!macroend

Section "ZipShrink (required)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; The updater starts setup and then closes ZipShrink.
  !insertmacro WaitUnlocked "$INSTDIR\zipshrink-gui.exe"
  !insertmacro WaitUnlocked "$INSTDIR\zipshrink.exe"
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
  ; Versions 0.1 to 0.3 also came as an MSI; remove such a copy so only one
  ; ZipShrink remains (Windows asks for permission: it was per machine).
  System::Call 'msi::MsiEnumRelatedProductsA(t "${MSI_UPGRADE_CODE}", i 0, i 0, t .r1) i .r0'
  ${If} $0 == 0
    DetailPrint "Removing the earlier ZipShrink installed from ZipShrink.msi..."
    ExecWait '"$SYSDIR\msiexec.exe" /x $1 /qb-' $0
  ${EndIf}
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
  ; An earlier installation decides the folder (upgrade in place) and which
  ; optional parts stay: an upgrade adds nothing that was left out.
  ReadRegStr $0 SHCTX "Software\ZipShrink" "InstallDir"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
    ${IfNot} ${FileExists} "$DESKTOP\ZipShrink.lnk"
      !insertmacro UnselectSection ${SecDesktop}
    ${EndIf}
    ${If} $PerUser == 1
      ReadRegStr $1 HKCU "Software\Classes\Directory\shell\ZipShrink\command" ""
    ${Else}
      ReadRegStr $1 HKCR "Directory\shell\ZipShrink\command" ""
    ${EndIf}
    ${If} $1 == ""
      !insertmacro UnselectSection ${SecMenu}
    ${EndIf}
    ReadRegStr $1 HKCU "Environment" "Path"
    ${StrStr} $2 "$1" "$INSTDIR"
    ${If} $2 == ""
      !insertmacro UnselectSection ${SecPath}
    ${EndIf}
  ${ElseIf} $PerUser == 1
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\ZipShrink"
  ${EndIf}
  ; Windows 9x keeps PATH in AUTOEXEC.BAT, which setup does not edit.
  ${IfNot} ${IsNT}
    !insertmacro UnselectSection ${SecPath}
    !insertmacro SetSectionFlag ${SecPath} ${SF_RO}
  ${EndIf}
FunctionEnd

Function .onInstSuccess
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/RESTART" $1
  ${IfNot} ${Errors}
    Exec '"$INSTDIR\zipshrink-gui.exe"'
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
