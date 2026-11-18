; NSIS installer hooks for Marsad.
;
; marsad-agent.exe runs DETACHED (the hub daemon and the per-session shims keep sessions alive
; after the window closes), so it holds a lock on marsad-agent.exe and an installer/updater
; could not overwrite it. Stop the daemon gracefully first (sessions keep running in their
; shims and are re-adopted by the new daemon), then make sure no window holds the app binary.

!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec '"$INSTDIR\marsad-agent.exe" stop'
  nsExec::Exec 'taskkill /F /IM marsad-desktop.exe'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec '"$INSTDIR\marsad-agent.exe" stop'
  nsExec::Exec 'taskkill /F /IM marsad-desktop.exe'
!macroend
