; DropBeam NSIS installer hooks.
;
; Adds a "Send with DropBeam" entry to the Windows right-click (context) menu for
; every file, so Windows users can start a send without the menu bar that macOS
; has. The command runs DropBeam.exe with the selected file's path; the app's
; single-instance plugin forwards that path to the already-running app, which
; opens the "send to whom?" chooser. Written under HKCU so no admin is needed,
; and removed cleanly on uninstall.

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Classes\*\shell\DropBeam" "" "Send with DropBeam"
  WriteRegStr HKCU "Software\Classes\*\shell\DropBeam" "Icon" "$INSTDIR\DropBeam.exe,0"
  WriteRegStr HKCU "Software\Classes\*\shell\DropBeam\command" "" '"$INSTDIR\DropBeam.exe" "%1"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegKey HKCU "Software\Classes\*\shell\DropBeam"
!macroend
