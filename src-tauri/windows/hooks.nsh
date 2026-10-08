; DropBeam NSIS installer hooks.
;
; 1. "Send with DropBeam" in the Windows right-click (context) menu for every file
;    AND folder, so Windows users can start a send without the menu bar macOS
;    has. The command runs DropBeam.exe with the selected path; the app's
;    single-instance plugin forwards it to the already-running app, which opens
;    the "send to whom?" chooser. Written under HKCU so no admin is needed, and
;    removed cleanly on uninstall. (The MSI does the same in wix/context-menu.wxs.)
;
; 2. A Windows Defender Firewall rule letting DropBeam.exe receive on private and
;    domain networks (never public), so LAN discovery + direct transfers work
;    without the "Windows Defender Firewall has blocked some features" prompt.
;    netsh needs elevation: this succeeds for an all-users (admin) install and is
;    skipped silently for the default per-user install, where Windows asks once on
;    first run instead. Updates leave an existing rule alone.

!define DROPBEAM_FW_RULE "DropBeam (LAN transfers)"

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Classes\*\shell\DropBeam" "" "Send with DropBeam"
  WriteRegStr HKCU "Software\Classes\*\shell\DropBeam" "Icon" "$INSTDIR\DropBeam.exe,0"
  WriteRegStr HKCU "Software\Classes\*\shell\DropBeam\command" "" '"$INSTDIR\DropBeam.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\Directory\shell\DropBeam" "" "Send with DropBeam"
  WriteRegStr HKCU "Software\Classes\Directory\shell\DropBeam" "Icon" "$INSTDIR\DropBeam.exe,0"
  WriteRegStr HKCU "Software\Classes\Directory\shell\DropBeam\command" "" '"$INSTDIR\DropBeam.exe" "%1"'

  ; Replace (not duplicate) the rule so it always points at this install's exe.
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${DROPBEAM_FW_RULE}"'
  Pop $0
  nsExec::Exec 'netsh advfirewall firewall add rule name="${DROPBEAM_FW_RULE}" dir=in action=allow program="$INSTDIR\DropBeam.exe" enable=yes profile=private,domain'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegKey HKCU "Software\Classes\*\shell\DropBeam"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\DropBeam"
  ; An update re-runs the uninstaller first — keep the firewall rule then.
  ${If} $UpdateMode <> 1
    nsExec::Exec 'netsh advfirewall firewall delete rule name="${DROPBEAM_FW_RULE}"'
    Pop $0
  ${EndIf}
!macroend
