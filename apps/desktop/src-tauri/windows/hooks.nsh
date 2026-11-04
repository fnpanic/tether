; Tether NSIS installer hooks (Tauri bundler). Runs elevated (installMode = perMachine).

; "1" when the installer closed a running Tether main window/tray process (POSTINSTALL restarts it).
Var TetherRelaunch

; The bundler checks right after PREINSTALL / PREUNINSTALL whether tether.exe is in use and then asks
; the user to close Tether. Close everything ourselves first: the service (and its --server/--cm
; children), then every other tether.exe from this folder (main window, tray, session viewers),
; waiting until none is left. Prints 1 if a main window/tray process (no arguments) was running.
!macro TETHER_STOP_ALL
  nsExec::Exec 'sc.exe stop Tether'
  nsExec::ExecToStack `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$exe = '$INSTDIR\tether.exe'; $$s = Get-Service Tether -ErrorAction SilentlyContinue; if ($$s -and $$s.Status -ne 'Stopped') { try { $$s.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(20)) } catch {} }; $$ui = 0; for ($$i = 0; $$i -lt 50; $$i++) { $$p = @(Get-CimInstance Win32_Process -Filter 'Name=''tether.exe''' | Where-Object { $$_.ExecutablePath -eq $$exe }); if ($$p.Count -eq 0) { break }; foreach ($$x in $$p) { if ($$x.CommandLine -match '\.exe.?\s*$$') { $$ui = 1 }; Stop-Process -Id $$x.ProcessId -Force -ErrorAction SilentlyContinue }; Start-Sleep -Milliseconds 200 }; Write-Output $$ui"`
  Pop $0
  Pop $1
  ; Runs before the old uninstaller and again in PREINSTALL: only ever raise the relaunch flag.
  StrCpy $1 $1 1
  StrCmp $1 "1" 0 +2
    StrCpy $TetherRelaunch "1"
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; In-app updates (/UPDATE) and manual installs overwrite files in place. POSTINSTALL starts the
  ; service again. No-op for whatever is not installed or not running.
  !insertmacro TETHER_STOP_ALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToStack 'sc.exe query Tether'
  Pop $0
  Pop $1
  StrCmp $0 "0" 0 +2
    nsExec::Exec 'sc.exe start Tether'
  ; Direct LAN connections (policy network.direct): Tether listens on TCP 21118 and answers the
  ; LAN lookup on UDP 21119, from the local subnet only. The app listens only while this rule
  ; exists (netpolicy.rs), so it never raises a firewall prompt. Replaces older versions' rules.
  nsExec::Exec 'netsh advfirewall firewall delete rule name="Tether"'
  nsExec::Exec 'netsh advfirewall firewall add rule name="Tether" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=TCP localport=21118 remoteip=localsubnet profile=any enable=yes'
  nsExec::Exec 'netsh advfirewall firewall add rule name="Tether" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=UDP localport=21119 remoteip=localsubnet profile=any enable=yes'
  ; Reopen Tether for the signed-in user (unelevated, like "Run Tether" on the finish page) if we
  ; closed it. With /R (in-app update) the bundler restarts it itself.
  ${If} $TetherRelaunch == "1"
    ClearErrors
    ${GetOptions} $CMDLINE "/R" $0
    ${If} ${Errors}
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop and remove the background service, only if unattended access was installed.
  nsExec::ExecToStack 'sc.exe query Tether'
  Pop $0
  Pop $1
  StrCmp $0 "0" 0 +2
    nsExec::Exec '"$INSTDIR\tether.exe" --uninstall-service'
  nsExec::Exec 'netsh advfirewall firewall delete rule name="Tether"'
  !insertmacro TETHER_STOP_ALL
!macroend
