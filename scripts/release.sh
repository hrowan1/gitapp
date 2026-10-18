#!/usr/bin/env bash
# Build, sign and publish a release of Dev Inbox to GitHub Releases, where installed apps pick it up
# as an update.
#
#   npm run release -- 0.2.0        # or: patch | minor | major
#   npm run release -- 0.2.0 --dry  # build and stage everything, publish nothing
#
# Needs: the updater key (~/.tauri/dev-inbox-updater.key, made once and backed up), the GitHub CLI
# signed in, and a clean checkout of main. Signs with the "Dev Inbox Release" certificate if you've
# made it (scripts/signing-cert.sh) so macOS keeps permissions across updates; ad-hoc otherwise.
set -euo pipefail

cd "$(dirname "$0")/.."
REPO="hrowan1/gitapp"
KEY="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/dev-inbox-updater.key}"
CERT="${DEV_INBOX_SIGNING_IDENTITY:-Dev Inbox Release}"
ARCH="aarch64"
STAGE="src-tauri/target/release/publish"

die() { echo "✗ $*" >&2; exit 1; }
step() { echo; echo "▸ $*"; }

BUMP="${1:-}"
DRY=0
[[ "${2:-}" == "--dry" || "${1:-}" == "--dry" ]] && DRY=1
[[ "$BUMP" == "--dry" ]] && BUMP=""
[[ -n "$BUMP" ]] || die "Which version? e.g. npm run release -- 0.2.0 (or patch, minor, major)"

# ---- Checks -------------------------------------------------------------------------------------
[[ "$(uname -m)" == "arm64" ]] || die "Releases are built on an Apple Silicon Mac"
[[ -f "$KEY" ]] || die "No updater key at $KEY — restore your backup (every release must be signed with the same key)"
PUB_IN_CONFIG=$(node -p "require('./src-tauri/tauri.conf.json').plugins.updater.pubkey")
[[ ! -f "$KEY.pub" || "$(cat "$KEY.pub")" == "$PUB_IN_CONFIG" ]] || die "$KEY.pub doesn't match the pubkey in tauri.conf.json"
if [[ $DRY == 0 ]]; then
  command -v gh >/dev/null || die "Install the GitHub CLI (brew install gh)"
  gh auth status >/dev/null 2>&1 || die "Sign in to GitHub: gh auth login"
  [[ "$(git branch --show-current)" == "main" ]] || die "Release from main"
  [[ -z "$(git status --porcelain)" ]] || die "Commit or stash your changes first"
  git fetch -q origin main
  [[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || die "main isn't the same as origin/main — pull or push first"
fi

CURRENT=$(node -p "require('./package.json').version")
VERSION=$(node -e '
  const [cur, bump] = process.argv.slice(1);
  const [a, b, c] = cur.split(".").map(Number);
  const v = bump === "major" ? `${a + 1}.0.0` : bump === "minor" ? `${a}.${b + 1}.0` : bump === "patch" ? `${a}.${b}.${c + 1}` : bump.replace(/^v/, "");
  if (!/^\d+\.\d+\.\d+$/.test(v)) { console.error("Not a version: " + bump); process.exit(1); }
  console.log(v);
' "$CURRENT" "$BUMP")
TAG="v$VERSION"
if [[ $DRY == 0 ]]; then
  git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && die "$TAG already exists"
  gh release view "$TAG" -R "$REPO" >/dev/null 2>&1 && die "A $TAG release already exists on GitHub"
fi

if security find-identity -p codesigning 2>/dev/null | grep -q "\"$CERT\""; then
  IDENTITY="$CERT"
else
  IDENTITY="-"
  echo "! No \"$CERT\" certificate — signing ad-hoc. People will have to allow the app again after each update."
  echo "  Run scripts/signing-cert.sh once to fix that."
fi

echo "Dev Inbox $CURRENT → $VERSION ($TAG), signed with: $IDENTITY$([[ $DRY == 1 ]] && echo ', dry run')"

# ---- Version --------------------------------------------------------------------------------------
# Kept aside so a dry run or a "no" puts them back exactly (uncommitted changes included).
VERSION_FILES=(package.json package-lock.json src-tauri/Cargo.toml src-tauri/Cargo.lock)
SAVED=$(mktemp -d)
for f in "${VERSION_FILES[@]}"; do mkdir -p "$SAVED/$(dirname "$f")"; cp "$f" "$SAVED/$f"; done
restore() { for f in "${VERSION_FILES[@]}"; do cp "$SAVED/$f" "$f"; done; }

step "Setting the version to $VERSION"
npm version "$VERSION" --no-git-tag-version --allow-same-version >/dev/null
sed -i '' -E "1,/^version = \".*\"/s/^version = \".*\"/version = \"$VERSION\"/" src-tauri/Cargo.toml

# ---- Tests ----------------------------------------------------------------------------------------
step "Testing"
if ! out=$(cd src-tauri && cargo test --quiet 2>&1); then
  echo "$out" | tail -30
  restore
  die "Tests failed"
fi
echo "$out" | grep -E "^test result" | head -1

# ---- Build ----------------------------------------------------------------------------------------
step "Building (a few minutes)"
export TAURI_SIGNING_PRIVATE_KEY
TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
npm run tauri build -- --bundles app,dmg --config "{\"bundle\":{\"macOS\":{\"signingIdentity\":\"$IDENTITY\"}}}"

BUNDLE="src-tauri/target/release/bundle"
APP="$BUNDLE/macos/Dev Inbox.app"
TARBALL="$BUNDLE/macos/Dev Inbox.app.tar.gz"
DMG=$(ls "$BUNDLE"/dmg/Dev\ Inbox_"$VERSION"_*.dmg | head -1)
[[ -f "$TARBALL" && -f "$TARBALL.sig" && -f "$DMG" ]] || die "The build didn't produce the app, its update archive and signature, and the .dmg"

step "Checking the build"
codesign --verify --deep --strict "$APP" || die "The app's signature doesn't verify"
for helper in "$APP"/Contents/MacOS/*; do codesign --verify "$helper" || die "$(basename "$helper") isn't signed"; done
BUILT=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$APP/Contents/Info.plist")
[[ "$BUILT" == "$VERSION" ]] || die "Built $BUILT, expected $VERSION"
echo "✓ $VERSION, signature OK"

# ---- Stage ----------------------------------------------------------------------------------------
# Fixed names, so README links can point at releases/latest/download/<name>.
rm -rf "$STAGE" && mkdir -p "$STAGE"
cp "$DMG" "$STAGE/Dev-Inbox-macOS-arm64.dmg"
cp "$TARBALL" "$STAGE/Dev-Inbox-macOS-arm64.app.tar.gz"
cp "$TARBALL.sig" "$STAGE/Dev-Inbox-macOS-arm64.app.tar.gz.sig"

LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || true)
{
  echo "## What's new"
  echo
  git log --format='- %s' ${LAST_TAG:+"$LAST_TAG..HEAD"} --no-merges | grep -v "^- Release v" | head -40
} > "$STAGE/notes.md"

node -e '
  const fs = require("fs");
  const [stage, version, repo, tag, arch] = process.argv.slice(1);
  const notes = fs.readFileSync(`${stage}/notes.md`, "utf8").replace(/^## What.s new\n\n/, "").trim();
  const manifest = {
    version,
    notes,
    pub_date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
    platforms: {
      [`darwin-${arch}`]: {
        signature: fs.readFileSync(`${stage}/Dev-Inbox-macOS-arm64.app.tar.gz.sig`, "utf8").trim(),
        url: `https://github.com/${repo}/releases/download/${tag}/Dev-Inbox-macOS-arm64.app.tar.gz`,
      },
    },
  };
  fs.writeFileSync(`${stage}/latest.json`, JSON.stringify(manifest, null, 2) + "\n");
' "$STAGE" "$VERSION" "$REPO" "$TAG" "$ARCH"

echo "Staged in $STAGE:"; ls -lh "$STAGE" | tail -n +2

if [[ $DRY == 1 ]]; then
  restore
  echo; echo "Dry run: nothing committed or published (version files put back)."
  exit 0
fi

# ---- Publish --------------------------------------------------------------------------------------
echo
read -r -p "Publish $TAG to github.com/$REPO? Installed apps will update to it. [y/N] " ok
if [[ "$ok" != "y" && "$ok" != "Y" ]]; then
  restore
  die "Not published (version files put back)"
fi

step "Publishing $TAG"
git add package.json package-lock.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -q -m "Release $TAG"
git tag -a "$TAG" -m "Dev Inbox $VERSION"
git push -q origin main "$TAG"
gh release create "$TAG" -R "$REPO" \
  --title "Dev Inbox $VERSION" \
  --notes-file "$STAGE/notes.md" \
  "$STAGE/Dev-Inbox-macOS-arm64.dmg" \
  "$STAGE/Dev-Inbox-macOS-arm64.app.tar.gz" \
  "$STAGE/Dev-Inbox-macOS-arm64.app.tar.gz.sig" \
  "$STAGE/latest.json"
echo
echo "✓ Released: https://github.com/$REPO/releases/tag/$TAG"
echo "  Installed copies pick it up within 6 hours (or Settings → Updates → Check now)."
