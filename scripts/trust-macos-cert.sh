#!/bin/sh
# Install Tether's public code-signing certificate so macOS trusts its signed apps' signature:
# codesign validation passes and Screen Recording / Accessibility are stable. The .cer is public.
#
#   # this user only (no sudo):
#   curl -fsSL https://<relay>/download/trust-macos-cert.sh | sh -s -- https://<relay>
#   # all users (asks for your password once):
#   curl -fsSL https://<relay>/download/trust-macos-cert.sh | sh -s -- https://<relay> --machine
#
# Note: a self-signed certificate is not an Apple Developer ID, so Gatekeeper still shows "Open Anyway"
# once for an app downloaded in a browser. The curl install (scripts/install-macos.sh) avoids that
# entirely (curl sets no quarantine), so for a no-prompt install use that; this trust step keeps the
# signature recognised across updates. For zero prompts on any Mac you need notarization (docs/code-signing.md).
set -eu
src=${1:?usage: trust-macos-cert.sh https://<relay> [--machine] | <file.cer> [--machine]}
machine=${2:-}
case $src in
  https://*) cer=$(mktemp); curl -fsSL "${src%/}/download/tether-codesign-macos.cer" -o "$cer" ;;
  *) cer=$src ;;
esac
# Confirm it is the right certificate and public (no private key).
subj=$(openssl x509 -in "$cer" -inform DER -noout -subject 2>/dev/null || openssl x509 -in "$cer" -noout -subject)
case $subj in *"Tether Code Signing"*) ;; *) echo "unexpected certificate: $subj" >&2; exit 1 ;; esac

if [ "$machine" = "--machine" ]; then
  sudo security add-trusted-cert -d -r trustAsRoot -p codeSign -k /Library/Keychains/System.keychain "$cer"
  echo "Installed 'Tether Code Signing' for all users (System keychain, code signing)."
else
  security add-trusted-cert -r trustAsRoot -p codeSign -k "$HOME/Library/Keychains/login.keychain-db" "$cer"
  echo "Installed 'Tether Code Signing' for your account (login keychain, code signing)."
fi
