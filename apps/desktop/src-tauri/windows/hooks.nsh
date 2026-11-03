; Tether NSIS installer hooks (Tauri bundler). Runs elevated (installMode = perMachine).

!macro NSIS_HOOK_POSTINSTALL
  ; Allow incoming P2P sessions for Tether only (program-scoped, private + domain networks).
  ; Public networks stay blocked; sessions there fall back to the relay.
  nsExec::Exec 'netsh advfirewall firewall delete rule name="Tether"'
  nsExec::Exec 'netsh advfirewall firewall add rule name="Tether" dir=in action=allow program="$INSTDIR\tether.exe" enable=yes profile=private,domain protocol=TCP'
  nsExec::Exec 'netsh advfirewall firewall add rule name="Tether" dir=in action=allow program="$INSTDIR\tether.exe" enable=yes profile=private,domain protocol=UDP'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop and remove the background service (no-op if unattended access was never installed).
  nsExec::Exec '"$INSTDIR\tether.exe" --uninstall-service'
  nsExec::Exec 'netsh advfirewall firewall delete rule name="Tether"'
!macroend
