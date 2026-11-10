# Install Tether's public code-signing certificate so its signed apps are trusted on this PC:
# no SmartScreen "Unknown publisher", no warning. Run once per machine (or deploy the .cer by
# GPO/Intune to a fleet). The .cer is public; it contains no private key.
#
#   # current user only (no admin):
#   powershell -NoProfile -Command "& ([scriptblock]::Create((irm https://<relay>/download/trust-windows-cert.ps1))) -Server https://<relay>"
#   # all users (run the PowerShell window as Administrator), pass the relay:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\trust-windows-cert.ps1 -Server https://<relay> -Machine
#   # remove it again (add -Machine if it was installed for all users, e.g. by "Install Tether certificate.cmd"):
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\trust-windows-cert.ps1 -Remove
#
# -Cer takes a local .cer instead of downloading; -Server <relay> downloads /download/tether-codesign.cer.
# Only the certificate with the thumbprint below is installed, whatever the server sends. -WhatIf changes nothing.
[CmdletBinding(SupportsShouldProcess)]
param(
  [string]$Server,
  [string]$Cer,
  [switch]$Machine,
  [switch]$Remove
)
$ErrorActionPreference = 'Stop'
$Pin = '9BDFE83E548CEF50DF2E96B8737ABCCD39BDC346'   # SHA-1 of certs/tether-codesign.cer
if (-not $Remove) {
  if (-not $Cer) {
    if ($Server -notmatch '^https://') { throw 'Give -Server https://<relay> (https only) or -Cer <file.cer>' }
    $Cer = Join-Path $env:TEMP 'tether-codesign.cer'
    Invoke-WebRequest "$($Server.TrimEnd('/'))/download/tether-codesign.cer" -OutFile $Cer -UseBasicParsing
  }
  $c = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($Cer)
  # A name proves nothing (anyone can make a "Tether Code Signing" certificate): this goes into Root.
  if ($c.Thumbprint -ne $Pin) { throw "unexpected certificate: $($c.Subject), thumbprint $($c.Thumbprint), expected $Pin" }
  if ($c.HasPrivateKey) { throw 'refusing: this file contains a private key; install only the public .cer' }
}

$loc = if ($Machine) { 'LocalMachine' } else { 'CurrentUser' }   # LocalMachine needs Administrator
foreach ($store in 'Root', 'TrustedPublisher') {                 # Root = trust the signer; TrustedPublisher = run without prompt
  if (-not $PSCmdlet.ShouldProcess("Cert:\$loc\$store", "$(if ($Remove) {'remove'} else {'add'}) certificate $Pin")) { continue }
  $s = [System.Security.Cryptography.X509Certificates.X509Store]::new($store, $loc)
  $s.Open('ReadWrite')
  # Add the checked object, not the file again (it could be swapped after the check).
  if ($Remove) { foreach ($x in $s.Certificates.Find('FindByThumbprint', $Pin, $false)) { $s.Remove($x) } } else { $s.Add($c) }
  $s.Close()
}
if ($WhatIfPreference) { return }
if ($Remove) { "Removed 'Tether Code Signing' (thumbprint $Pin) from $loc Root + TrustedPublisher."; return }
"Installed 'Tether Code Signing' (thumbprint $Pin) into $loc Root + TrustedPublisher."
"Tether's signed installers are now trusted for $(if ($Machine) {'all users'} else {'your account'}) on this PC."
