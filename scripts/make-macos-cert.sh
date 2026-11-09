#!/bin/sh
# Generate a free self-signed code-signing certificate for Tether's macOS builds (the counterpart of
# scripts/make-windows-cert.ps1 and the existing MACOS_SIGNING_P12 secret, see docs/macos.md).
# Self-signed is free and gives a stable code signature, so Screen Recording / Accessibility survive
# updates. It is not Apple-notarized (first start still needs the curl install or "Open Anyway");
# a fully trusted, free Developer ID needs an organization fee waiver (docs/code-signing.md).
#
#   sh scripts/make-macos-cert.sh ./secret <p12-password>
#
# Output (keep secret, never commit): tether-codesign.p12  -> CI secret MACOS_SIGNING_P12 (base64).
set -eu
out=${1:?usage: make-macos-cert.sh <out-dir> <p12-password>}
pw=${2:?p12 password required}
mkdir -p "$out"
key="$out/key.pem"; crt="$out/cert.pem"; p12="$out/tether-codesign.p12"

openssl req -x509 -newkey rsa:3072 -keyout "$key" -out "$crt" -days 3650 -nodes \
  -subj "/CN=Tether Code Signing" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" \
  -addext "basicConstraints=critical,CA:false"
# -legacy: the p12 RC2 format the macOS `security import` keychain accepts.
openssl pkcs12 -export -out "$p12" -inkey "$key" -in "$crt" -passout "pass:$pw" -legacy 2>/dev/null \
  || openssl pkcs12 -export -out "$p12" -inkey "$key" -in "$crt" -passout "pass:$pw"

# Flawless one-double-click installer for customers: a configuration profile that installs and trusts
# the certificate (System Settings shows it, the user clicks Install). The native macOS way, no Terminal.
der=$(openssl x509 -in "$crt" -outform DER | base64 | tr -d '\n')
uid() { openssl rand -hex 16 | sed -E 's/(........)(....)(....)(....)(............)/\1-\2-\3-\4-\5/'; }
u1=$(uid); u2=$(uid)
cat > "$out/Trust Tether.mobileconfig" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>PayloadContent</key>
  <array><dict>
    <key>PayloadType</key><string>com.apple.security.root</string>
    <key>PayloadContent</key><data>$der</data>
    <key>PayloadIdentifier</key><string>net.tether.cert.$u2</string>
    <key>PayloadUUID</key><string>$u2</string>
    <key>PayloadVersion</key><integer>1</integer>
    <key>PayloadDisplayName</key><string>Tether Code Signing</string>
  </dict></array>
  <key>PayloadDisplayName</key><string>Trust Tether</string>
  <key>PayloadDescription</key><string>Trusts Tether's code-signing certificate on this Mac.</string>
  <key>PayloadIdentifier</key><string>net.tether.trust.$u1</string>
  <key>PayloadType</key><string>Configuration</string>
  <key>PayloadUUID</key><string>$u1</string>
  <key>PayloadVersion</key><integer>1</integer>
</dict></plist>
EOF
rm -f "$key" "$crt"

echo "P12        : $p12   (secret -> MACOS_SIGNING_P12 = base64 -w0 of this; password -> MACOS_SIGNING_PASSWORD)"
echo "Installer  : $out/Trust Tether.mobileconfig   (hand to customers: double-click, Install in System Settings)"
echo "Note: codesign uses the p12 like today's cert (CI 'Import the code-signing certificate' step trusts it)."
echo "      The profile trusts the signature; a browser download still needs 'Open Anyway' once unless notarized."
