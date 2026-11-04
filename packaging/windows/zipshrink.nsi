; ZipShrink setup for 64-bit Windows 10 and 11.
;
; Installs for the current user (no administrator rights needed) into
; %LOCALAPPDATA%\Programs\ZipShrink, the folder every earlier version used
; on Windows 10, so it upgrades them in place. On anything older than
; Windows 10, or on 32-bit Windows, it refuses and changes nothing: older
; ZipShrink versions offer this update there too.
;
; ZipShrink's updater runs it as `ZipShrink-setup.exe /S [/RESTART]`: silent,
; it waits for ZipShrink to close, keeps the choices of the earlier
; installation, and with /RESTART starts ZipShrink again afterwards.
;
; Build: makensis -DVERSION=1.0.0 -DBINDIR=path\to\exes -DOUTFILE=ZipShrink-setup.exe zipshrink.nsi

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestSupportedOS all

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef BINDIR
  !define BINDIR "..\..\target\x86_64-pc-windows-gnu\release"
!endif
!ifndef OUTFILE
  !define OUTFILE "ZipShrink-setup.exe"
!endif

!define APP "ZipShrink"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\ZipShrink"
; The Windows Installer packages of versions 0.1 to 0.3.
!define MSI_UPGRADE_CODE "{6B0E2F9A-3C41-4E7B-9F0D-2A5C8E1B7D34}"

!include "MUI2.nsh"
!include "WinVer.nsh"
!include "x64.nsh"
!include "LogicLib.nsh"
!include "Sections.nsh"
!include "StrFunc.nsh"
!include "FileFunc.nsh"
${StrStr}
${UnStrRep}

Name "${APP} ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\ZipShrink"
BrandingText "ZipShrink ${VERSION}"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "ZipShrink"
VIAddVersionKey "FileDescription" "ZipShrink Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
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

Function un.onInit
  SetShellVarContext current
FunctionEnd

; Waits up to 60 seconds for a running copy of FILE to close.
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
    ${If} $R1 >= 120
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
  ; Left over from version 0.1.
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.md"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\ZipShrink" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "ZipShrink"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "ZipShrink contributors"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/knivesbatz/zipshrink"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0
  ; Versions 0.1 to 0.3 also came as an MSI, installed for all users; remove
  ; such a copy so only one ZipShrink remains (Windows asks for permission).
  System::Call 'msi::MsiEnumRelatedProductsW(w "${MSI_UPGRADE_CODE}", i 0, i 0, w .r1) i .r0'
  ${If} $0 == 0
    DetailPrint "Removing the earlier ZipShrink installed from an .msi package..."
    ExecWait '"$SYSDIR\msiexec.exe" /x $1 /qb-' $0
  ${EndIf}
SectionEnd

Section "Start menu shortcut" SecStart
  CreateShortcut "$SMPROGRAMS\ZipShrink.lnk" "$INSTDIR\zipshrink-gui.exe"
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\ZipShrink.lnk" "$INSTDIR\zipshrink-gui.exe"
SectionEnd

!macro MenuEntry KEY
  WriteRegStr HKCU "${KEY}\ZipShrink" "" "Compress with ZipShrink..."
  WriteRegStr HKCU "${KEY}\ZipShrink" "Icon" "$INSTDIR\zipshrink-gui.exe"
  WriteRegStr HKCU "${KEY}\ZipShrink\command" "" '"$INSTDIR\zipshrink-gui.exe" "%1"'
!macroend

Section "Explorer right-click menu" SecMenu
  !insertmacro MenuEntry "Software\Classes\Directory\shell"
  !insertmacro MenuEntry "Software\Classes\*\shell"
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

; After the sections, so their indexes are defined.
Function .onInit
  SetShellVarContext current
  ; Earlier versions ran on Windows 95 and later, and offer this update
  ; there too: refuse before anything is changed.
  ${IfNot} ${AtLeastWin10}
  ${OrIfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "ZipShrink ${VERSION} needs 64-bit Windows 10 or Windows 11. Nothing was changed: the ZipShrink you have keeps working." /SD IDOK
    SetErrorLevel 3
    Quit
  ${EndIf}
  ; An earlier installation decides the folder (upgrade in place) and which
  ; optional parts stay: an upgrade adds nothing that was left out.
  ReadRegStr $0 HKCU "Software\ZipShrink" "InstallDir"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
    ${IfNot} ${FileExists} "$SMPROGRAMS\ZipShrink.lnk"
      !insertmacro UnselectSection ${SecStart}
    ${EndIf}
    ${IfNot} ${FileExists} "$DESKTOP\ZipShrink.lnk"
      !insertmacro UnselectSection ${SecDesktop}
    ${EndIf}
    ReadRegStr $1 HKCU "Software\Classes\Directory\shell\ZipShrink\command" ""
    ${If} $1 == ""
      !insertmacro UnselectSection ${SecMenu}
    ${EndIf}
    ReadRegStr $1 HKCU "Environment" "Path"
    ${StrStr} $2 "$1" "$INSTDIR"
    ${If} $2 == ""
      !insertmacro UnselectSection ${SecPath}
    ${EndIf}
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
  !insertmacro MUI_DESCRIPTION_TEXT ${SecStart} "Puts ZipShrink in the Start menu."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Puts a ZipShrink icon on the desktop."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMenu} "Adds 'Compress with ZipShrink...' when you right-click a file or folder."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecPath} "Lets you run 'zipshrink' from any command prompt."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\zipshrink-gui.exe"
  Delete "$INSTDIR\zipshrink.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.txt"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD_PARTY_LICENSES.md"
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
