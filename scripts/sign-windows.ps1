# Authenticode-sign release exes (Tether setup, TetherQS.exe) with the self-signed "Tether Code
# Signing" key (docs/code-signing.md), SHA-256 + timestamp. Sign before Tether Manage publishes:
# Manage hashes and minisigns the file as it is. Custom TetherQS apps stay signed: the relay puts
# their profile inside the signature's certificate table (tether_trust::custom::attach).
#   powershell -File scripts/sign-windows.ps1 dist/quick/TetherQS-<v>.exe dist/windows/Tether_<v>_x64-setup.exe
[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Pfx,
    [string]$PasswordFile,
    [Parameter(ValueFromRemainingArguments)][string[]]$Files
)
$ErrorActionPreference = 'Stop'
if (-not $Files) { throw 'usage: sign-windows.ps1 [-Pfx f.pfx] [-PasswordFile f.txt] <file.exe>...' }
# Not as param defaults: Windows PowerShell 5.1 leaves $PSScriptRoot empty there.
if (-not $Pfx) { $Pfx = Join-Path $PSScriptRoot '../secret/tether-codesign-windows.pfx' }
if (-not $PasswordFile) { $PasswordFile = Join-Path $PSScriptRoot '../secret/windows-password.txt' }
$cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new((Resolve-Path $Pfx).Path, (Get-Content $PasswordFile -Raw).Trim())
foreach ($f in $Files) {
    $r = Set-AuthenticodeSignature -FilePath (Resolve-Path $f).Path -Certificate $cert -HashAlgorithm SHA256 -TimestampServer 'http://timestamp.digicert.com'
    # Status UnknownError = signed, but this PC does not trust the self-signed root yet.
    if ($r.Status -ne 'Valid' -and $r.Status -ne 'UnknownError') { throw "signing ${f}: $($r.Status) $($r.StatusMessage)" }
    "signed $f ($($r.SignerCertificate.Thumbprint), $($r.Status))"
}
