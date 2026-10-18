#!/usr/bin/env bash
# Install (or reinstall) the latest Dev Inbox into /Applications:
#
#   curl -fsSL https://raw.githubusercontent.com/hrowan1/gitapp/main/install.sh | bash
#
# Downloaded this way the app isn't quarantined, so macOS doesn't block the first open. After this,
# Dev Inbox keeps itself up to date.
set -euo pipefail

URL="https://github.com/hrowan1/gitapp/releases/latest/download/Dev-Inbox-macOS-arm64.app.tar.gz"
DEST="/Applications"
APP="$DEST/Dev Inbox.app"

[[ "$(uname -s)" == "Darwin" ]] || { echo "Dev Inbox is a Mac app." >&2; exit 1; }
[[ "$(uname -m)" == "arm64" ]] || { echo "Dev Inbox needs an Apple Silicon Mac." >&2; exit 1; }
major=$(sw_vers -productVersion | cut -d. -f1)
minor=$(sw_vers -productVersion | cut -d. -f2)
if (( major < 14 || (major == 14 && minor < 2) )); then
  echo "Dev Inbox needs macOS 14.2 or later." >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "Downloading Dev Inbox…"
curl -fL --progress-bar "$URL" -o "$tmp/app.tar.gz"
tar -xzf "$tmp/app.tar.gz" -C "$tmp"
[[ -d "$tmp/Dev Inbox.app" ]] || { echo "The download didn't contain Dev Inbox.app." >&2; exit 1; }

if pgrep -xq "dev-inbox"; then
  echo "Quitting the running Dev Inbox…"
  osascript -e 'quit app "Dev Inbox"' >/dev/null 2>&1 || true
  sleep 2
fi

sudo_if_needed() { if [[ -w "$DEST" ]]; then "$@"; else sudo "$@"; fi; }
sudo_if_needed rm -rf "$APP"
sudo_if_needed mv "$tmp/Dev Inbox.app" "$APP"
xattr -dr com.apple.quarantine "$APP" 2>/dev/null || true

version=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$APP/Contents/Info.plist")
echo "✓ Dev Inbox $version is in Applications. Opening it…"
open "$APP"
