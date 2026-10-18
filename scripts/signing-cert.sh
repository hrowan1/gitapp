#!/usr/bin/env bash
# Once, on the Mac you release from: make the "Dev Inbox Release" code-signing certificate.
#
# Without an Apple Developer ID, releases are signed ad-hoc, and macOS treats every update as a
# different app: microphone / audio-capture permissions and keychain access get asked for again.
# Signing every release with this one self-signed certificate keeps the app's identity stable, so
# they're kept. (It doesn't make Gatekeeper trust the first download — only Apple notarisation does.)
#
# The certificate and its key go into your login keychain, with a backup in ~/.tauri. Keep that
# backup: a release signed with a new certificate counts as a different app once more.
set -euo pipefail

NAME="Dev Inbox Release"
BACKUP="$HOME/.tauri/dev-inbox-signing"

if security find-identity -p codesigning 2>/dev/null | grep -q "\"$NAME\""; then
  echo "✓ \"$NAME\" is already in your keychain."
  exit 0
fi

mkdir -p "$HOME/.tauri" && chmod 700 "$HOME/.tauri"
if [[ -f "$BACKUP.key.pem" && -f "$BACKUP.cert.pem" ]]; then
  echo "Restoring \"$NAME\" from $BACKUP.*"
else
  echo "Making a new \"$NAME\" certificate (valid 20 years)"
  conf=$(mktemp)
  cat > "$conf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
  openssl req -x509 -newkey rsa:2048 -nodes -days 7300 -config "$conf" \
    -keyout "$BACKUP.key.pem" -out "$BACKUP.cert.pem" 2>/dev/null
  rm -f "$conf"
  chmod 600 "$BACKUP.key.pem" "$BACKUP.cert.pem"
fi

p12=$(mktemp -d)/id.p12
pass=$(openssl rand -hex 16)
openssl pkcs12 -export -inkey "$BACKUP.key.pem" -in "$BACKUP.cert.pem" -name "$NAME" -out "$p12" -passout "pass:$pass"
security import "$p12" -k "$HOME/Library/Keychains/login.keychain-db" -P "$pass" -T /usr/bin/codesign >/dev/null
rm -rf "$(dirname "$p12")"

echo "✓ \"$NAME\" added to your login keychain. Releases will be signed with it."
echo "  The first release may ask to let codesign use it — choose Always Allow."
echo "  Back up $BACKUP.key.pem and $BACKUP.cert.pem with ~/.tauri/dev-inbox-updater.key."
