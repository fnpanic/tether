@echo off
rem Run once as administrator: makes C:\vms, its vm-*.ps1 scripts and C:\vms\broker writable for
rem administrators only, so the elevated broker (broker.ps1) agrees to start. The subfolders that
rem exist now stay writable for users, the request queue included. Same commands as in broker.ps1.
net session >nul 2>&1 || (echo Run this as administrator. & pause & exit /b 1)
for /d %%d in (C:\vms\*) do icacls "%%d" /grant "*S-1-5-11:(OI)(CI)M" >nul
icacls C:\vms /setowner "*S-1-5-32-544"
icacls C:\vms /inheritance:r /grant:r "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX"
icacls C:\vms\vm-*.ps1 /setowner "*S-1-5-32-544"
icacls C:\vms\vm-*.ps1 /reset
icacls C:\vms\broker /setowner "*S-1-5-32-544"
icacls C:\vms\broker /inheritance:r /grant:r "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX"
icacls C:\vms\broker /remove:g "*S-1-5-11"
icacls C:\vms\broker\* /setowner "*S-1-5-32-544"
icacls C:\vms\broker\* /reset
icacls C:\vms\broker\queue /grant "*S-1-5-11:(OI)(CI)M"
echo Done. Start the broker with start-broker.cmd.
pause
