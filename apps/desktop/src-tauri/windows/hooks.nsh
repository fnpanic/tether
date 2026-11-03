; Tether NSIS installer hooks (Tauri bundler). Runs elevated (installMode = perMachine).

!macro NSIS_HOOK_PREINSTALL
  ; In-app updates (/UPDATE) overwrite files in place: stop the background service first so its
  ; processes release tether.exe. POSTINSTALL starts it again. No-op when it is not installed.
  nsExec::Exec 'sc.exe stop Tether'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToStack 'sc.exe query Tether'
  Pop $0
  Pop $1
  StrCmp $0 "0" 0 +2
    nsExec::Exec 'sc.exe start Tether'
  ; Allow incoming P2P sessions for Tether only (program-scoped, private + domain networks).
  ; Public networks stay blocked; sessions there fall back to the relay.
  nsExec::Exec 'netsh advfirewall firewall delete rule name="Tether"'
  nsExec::Exec 'netsh advfirewall firewall add rule name="Tether" dir=in action=allow program="$INSTDIR\tether.exe" enable=yes profile=private,domain protocol=TCP'
  nsExec::Exec 'netsh advfirewall firewall add rule name="Tether" dir=in action=allow program="$INSTDIR\tether.exe" enable=yes profile=private,domain protocol=UDP'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop and remove the background service, only if unattended access was installed.
  nsExec::ExecToStack 'sc.exe query Tether'
  Pop $0
  Pop $1
  StrCmp $0 "0" 0 +2
    nsExec::Exec '"$INSTDIR\tether.exe" --uninstall-service'
  nsExec::Exec 'netsh advfirewall firewall delete rule name="Tether"'
!macroend
