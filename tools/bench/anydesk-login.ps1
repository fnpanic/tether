# Bench helper: answer AnyDesk's "Autorisierung" (authorization) prompt in a session window:
# type the password, tick "log in automatically in future", confirm.
param([string]$Title, [string]$PasswordFile = 'C:\vms\payload\bench.json')
$pw = (Get-Content $PasswordFile | ConvertFrom-Json).password
$sh = New-Object -ComObject WScript.Shell
if (-not $sh.AppActivate($Title)) { 'window not found'; exit 1 }
Start-Sleep -Milliseconds 400
# SendKeys treats + ^ % ~ ( ) { } [ ] as special; escape them.
$escaped = ($pw.ToCharArray() | ForEach-Object { if ('+^%~(){}[]'.Contains($_)) { "{$_}" } else { "$_" } }) -join ''
$sh.SendKeys($escaped)
Start-Sleep -Milliseconds 200
$sh.SendKeys('{TAB}')
$sh.SendKeys(' ')
Start-Sleep -Milliseconds 200
$sh.SendKeys('{ENTER}')
'sent'
