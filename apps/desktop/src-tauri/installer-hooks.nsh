; NSIS installer hooks for Marsad.
;
; marsad-agent.exe is a DETACHED daemon: it keeps running after the desktop window
; is closed so live sessions outlive the app. That means it holds a file lock on
; marsad-agent.exe, and an installer/updater can't overwrite it ("can't write to
; marsad-agent.exe") even though no "Marsad" window is open. Terminate the agent
; (and the winshim) before installing, and again before uninstalling.

!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /F /IM marsad-agent.exe'
  nsExec::Exec 'taskkill /F /IM marsad-winshim.exe'
  ; Also kill the desktop window — an open app holds a lock on marsad-desktop.exe
  ; that would block the installer from overwriting it.
  nsExec::Exec 'taskkill /F /IM marsad-desktop.exe'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /IM marsad-agent.exe'
  nsExec::Exec 'taskkill /F /IM marsad-winshim.exe'
  ; Also kill the desktop window so an open app can't block uninstall.
  nsExec::Exec 'taskkill /F /IM marsad-desktop.exe'
!macroend
