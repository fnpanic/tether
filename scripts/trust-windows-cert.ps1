# Install Tether's public code-signing certificate so its signed apps are trusted on this PC:
# no SmartScreen "Unknown publisher", no warning. Run once per machine (or deploy the .cer by
# GPO/Intune to a fleet). The .cer is public; it contains no private key.
#
#   # current user only (no admin):
#   powershell -NoProfile -Command "irm https://<relay>/download/trust-windows-cert.ps1 | iex"
#   # all users (run the PowerShell window as Administrator), pass the relay:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\trust-windows-cert.ps1 -Server https://<relay> -Machine
#
# -Cer takes a local .cer instead of downloading; -Server <relay> downloads /download/tether-codesign.cer.
param(
  [string]$Server,
  [string]$Cer,
  [switch]$Machine
)
$ErrorActionPreference = 'Stop'
if (-not $Cer) {
  if (-not $Server) { throw 'Give -Server https://<relay> or -Cer <file.cer>' }
  $Cer = Join-Path $env:TEMP 'tether-codesign.cer'
  Invoke-WebRequest "$($Server.TrimEnd('/'))/download/tether-codesign.cer" -OutFile $Cer -UseBasicParsing
}
$c = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($Cer)
if ($c.Subject -ne 'CN=Tether Code Signing') { throw "unexpected certificate: $($c.Subject)" }
if ($c.HasPrivateKey) { throw 'refusing: this file contains a private key; install only the public .cer' }

$loc = if ($Machine) { 'LocalMachine' } else { 'CurrentUser' }   # LocalMachine needs Administrator
foreach ($store in 'Root', 'TrustedPublisher') {                 # Root = trust the signer; TrustedPublisher = run without prompt
  Import-Certificate -FilePath $Cer -CertStoreLocation "Cert:\$loc\$store" | Out-Null
}
"Installed 'Tether Code Signing' (thumbprint $($c.Thumbprint)) into $loc Root + TrustedPublisher."
"Tether's signed installers are now trusted for $(if ($Machine) {'all users'} else {'your account'}) on this PC."
