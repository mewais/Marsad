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
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /IM marsad-agent.exe'
  nsExec::Exec 'taskkill /F /IM marsad-winshim.exe'
!macroend
