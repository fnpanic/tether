# Generate a free self-signed code-signing certificate for Tether's Windows builds, mirroring the
# macOS "Tether Code Signing" certificate (docs/code-signing.md). Self-signed is free and gives a
# stable publisher identity: on managed fleets IT deploys the public .cer to the Trusted Publishers
# store (GPO/Intune) and the apps are then fully trusted (no SmartScreen, no "Unknown publisher"),
# at no cost. On unmanaged machines it adds no trust by itself; pair it with the Mark-of-the-Web-free
# install on the download page, or move to the SignPath Foundation certificate once approved.
#
#   pwsh scripts/make-windows-cert.ps1 -OutDir .\secret -Password <pfx-password>
#
# Outputs (keep the .pfx and password secret; the .cer is public):
#   tether-codesign.pfx   signing key + cert   -> CI secret WINDOWS_SIGNING_P12 (base64), never committed
#   tether-codesign.cer   public certificate   -> ship to customers' IT for the trust store
#   Install / Remove Tether certificate.cmd    -> double-click installers for customers (public)
param(
  [string]$OutDir = (Join-Path (Get-Location) 'secret'),
  [Parameter(Mandatory)][string]$Password,
  [string]$Subject = 'CN=Tether Code Signing',
  [int]$Years = 10
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $OutDir | Out-Null

# Code Signing EKU (1.3.6.1.5.5.7.3.3), exportable key, SHA-256. basicConstraints CA=false (critical):
# customers install this certificate into Root, so it must not be able to issue other certificates.
# The certificate in use (thumbprint 9BDFE83E...) was made before this and has no basicConstraints; it
# is not regenerated, because a new certificate needs a new trust install on every customer PC.
$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $Subject `
  -KeyUsage DigitalSignature -KeyExportPolicy Exportable -KeyLength 3072 -HashAlgorithm SHA256 `
  -NotAfter (Get-Date).AddYears($Years) -CertStoreLocation Cert:\CurrentUser\My `
  -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={critical}{text}ca=0')
$bc = $cert.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.19' }
if (-not $bc -or -not $bc.Critical -or $bc.CertificateAuthority) {
  Remove-Item ("Cert:\CurrentUser\My\" + $cert.Thumbprint) -Force
  throw 'the new certificate lacks critical basicConstraints CA=false'
}

$pfx = Join-Path $OutDir 'tether-codesign.pfx'
$cer = Join-Path $OutDir 'tether-codesign.cer'
$cmd = Join-Path $OutDir 'Install Tether certificate.cmd'
$sec = ConvertTo-SecureString $Password -AsPlainText -Force
Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $sec | Out-Null
Export-Certificate   -Cert $cert -FilePath $cer | Out-Null

# Flawless one-double-click installer for customers: a self-elevating .cmd with the public certificate
# embedded (PEM), installed machine-wide with the built-in certutil. No separate .cer, no network, no
# store picking. Root = trust the signer; TrustedPublisher = run the signed installer without a prompt.
$b64 = [Convert]::ToBase64String($cert.RawData)
$pem = for ($i = 0; $i -lt $b64.Length; $i += 64) { $b64.Substring($i, [Math]::Min(64, $b64.Length - $i)) }
$w = [Text.StringBuilder]::new()
[void]$w.AppendLine('@echo off')
[void]$w.AppendLine('rem Installs the Tether code-signing certificate so Windows trusts Tether. Double-click it.')
[void]$w.AppendLine('net session >nul 2>&1 || (powershell -NoProfile -Command "Start-Process -Verb RunAs -FilePath ''%~f0''" & exit /b)')
[void]$w.AppendLine('set "C=%TEMP%\tether-codesign.cer"')
[void]$w.AppendLine('>"%C%" echo -----BEGIN CERTIFICATE-----')
foreach ($l in $pem) { [void]$w.AppendLine(">>`"%C%`" echo $l") }
[void]$w.AppendLine('>>"%C%" echo -----END CERTIFICATE-----')
[void]$w.AppendLine('certutil -addstore -f Root "%C%" >nul && certutil -addstore -f TrustedPublisher "%C%" >nul && (echo Tether certificate installed.) || (echo Install FAILED.)')
[void]$w.AppendLine('del "%C%" >nul 2>&1')
[void]$w.AppendLine('echo.')
[void]$w.AppendLine('pause')
# ASCII, CRLF: a .cmd must be plain ANSI with Windows line endings.
[IO.File]::WriteAllText($cmd, ($w.ToString() -replace "`r?`n", "`r`n"), [Text.Encoding]::ASCII)

# Its counterpart: removes exactly this certificate (by thumbprint) from both stores.
$rm = @('@echo off',
  'rem Removes the Tether code-signing certificate that "Install Tether certificate.cmd" installed. Double-click it.',
  'net session >nul 2>&1 || (powershell -NoProfile -Command "Start-Process -Verb RunAs -FilePath ''%~f0''" & exit /b)',
  "certutil -delstore Root $($cert.Thumbprint) >nul",
  "certutil -delstore TrustedPublisher $($cert.Thumbprint) >nul",
  'echo Tether certificate removed.', 'echo.', 'pause', '') -join "`r`n"
[IO.File]::WriteAllText((Join-Path $OutDir 'Remove Tether certificate.cmd'), $rm, [Text.Encoding]::ASCII)

# Keep the key only in the files, not in the user's store.
Remove-Item ("Cert:\CurrentUser\My\" + $cert.Thumbprint) -Force

"Thumbprint : $($cert.Thumbprint)"
"Expires    : $($cert.NotAfter.ToString('yyyy-MM-dd'))"
"PFX        : $pfx   (secret -> CI WINDOWS_SIGNING_P12 = base64 of this)"
"CER        : $cer   (public)"
"Installer  : $cmd   (hand to customers: double-click, confirm UAC, done)"
