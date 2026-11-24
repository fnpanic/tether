; Tether NSIS installer hooks (Tauri bundler). Runs elevated (installMode = perMachine).

; System tools by full path: a bare name is looked up in the installer's own folder first, so a
; file planted beside the downloaded setup exe would run elevated. The installer is a 32-bit
; process: $SYSDIR is the system folder it sees (SysWOW64 on 64-bit Windows), which has all three.
!define TETHER_SC '"$SYSDIR\sc.exe"'
!define TETHER_NETSH '"$SYSDIR\netsh.exe"'
!define TETHER_POWERSHELL '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"'

; "1" when the installer closed a running Tether main window/tray process (POSTINSTALL restarts it).
Var TetherRelaunch

; The bundler checks right after PREINSTALL / PREUNINSTALL whether tether.exe is in use and then asks
; the user to close Tether. Close everything ourselves first: the service (and its --server/--cm
; children), then every other tether.exe from this folder (main window, tray, session viewers),
; waiting until none is left. Prints 1 if a main window/tray process (no arguments) was running.
!macro TETHER_STOP_ALL
  nsExec::Exec '${TETHER_SC} stop ${PRODUCTNAME}'
  nsExec::ExecToStack `${TETHER_POWERSHELL} -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$exe = '$INSTDIR\${MAINBINARYNAME}.exe'; $$s = Get-Service ${PRODUCTNAME} -ErrorAction SilentlyContinue; if ($$s -and $$s.Status -ne 'Stopped') { try { $$s.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(20)) } catch {} }; $$ui = 0; for ($$i = 0; $$i -lt 50; $$i++) { $$p = @(Get-CimInstance Win32_Process -Filter 'Name=''${MAINBINARYNAME}.exe''' | Where-Object { $$_.ExecutablePath -eq $$exe }); if ($$p.Count -eq 0) { break }; foreach ($$x in $$p) { if ($$x.CommandLine -match '\.exe.?\s*$$') { $$ui = 1 }; Stop-Process -Id $$x.ProcessId -Force -ErrorAction SilentlyContinue }; Start-Sleep -Milliseconds 200 }; Write-Output $$ui"`
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
  nsExec::ExecToStack '${TETHER_SC} query ${PRODUCTNAME}'
  Pop $0
  Pop $1
  StrCmp $0 "0" 0 +2
    nsExec::Exec '${TETHER_SC} start ${PRODUCTNAME}'
  ; Direct LAN connections (policy network.direct): Tether listens on TCP 21118 and answers the
  ; LAN lookup on UDP 21119, from the local subnet only. The app listens only while this rule
  ; exists (netpolicy.rs), so it never raises a firewall prompt. Replaces older versions' rules.
  nsExec::Exec '${TETHER_NETSH} advfirewall firewall delete rule name="${PRODUCTNAME}"'
  nsExec::Exec '${TETHER_NETSH} advfirewall firewall add rule name="${PRODUCTNAME}" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=TCP localport=21118 remoteip=localsubnet profile=any enable=yes'
  nsExec::Exec '${TETHER_NETSH} advfirewall firewall add rule name="${PRODUCTNAME}" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=UDP localport=21119 remoteip=localsubnet profile=any enable=yes'
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
  nsExec::ExecToStack '${TETHER_SC} query ${PRODUCTNAME}'
  Pop $0
  Pop $1
  StrCmp $0 "0" 0 +2
    nsExec::Exec '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-service'
  nsExec::Exec '${TETHER_NETSH} advfirewall firewall delete rule name="${PRODUCTNAME}"'
  !insertmacro TETHER_STOP_ALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; "Delete the application data": Tether keeps its data (device key, password hash, remembered
  ; peer passwords, team key, policy cache, logs) in %APPDATA%\Tether, not under the bundle id the
  ; bundler removes, and the service keeps a copy in the LocalService profile.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RmDir /r "$WINDIR\ServiceProfiles\LocalService\AppData\Roaming\${PRODUCTNAME}"
    ; The elevated uninstaller's profile (the admin who confirmed UAC) ...
    SetShellVarContext current
    RmDir /r "$APPDATA\${PRODUCTNAME}"
    ; ... and the signed-in user's, as that user (RunAsUser: unelevated, %APPDATA% is theirs).
    ; shortcut: other users' profiles on this PC keep their %APPDATA%\Tether; walk
    ; HKLM\...\ProfileList when shared PCs must be wiped by one uninstall.
    nsis_tauri_utils::RunAsUser "$SYSDIR\cmd.exe" '/c rd /s /q "%APPDATA%\${PRODUCTNAME}"'
  ${EndIf}
!macroend
