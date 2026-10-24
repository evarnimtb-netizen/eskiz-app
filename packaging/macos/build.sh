#!/usr/bin/env bash
# Builds Eskiz for macOS: a universal (arm64 + x86_64) Eskiz.app, ad-hoc signed, in a DMG
# with an Applications symlink.
#
# Output: eskiz/dist/release/Eskiz-<version>-macos-universal.dmg (and the Eskiz.app folder).
# Requires Xcode command line tools (lipo, codesign, hdiutil). Rust comes from the Homebrew
# rustup if present, which must have both Apple targets (added below if missing).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ESKIZ_DIR="$(cd "$HERE/../.." && pwd)"
ICON="$ESKIZ_DIR/packaging/icons/eskiz.icns"
OUT_DIR="$ESKIZ_DIR/dist/release"

# Local machines: prefer the Homebrew rustup, which has the Apple targets. CI already has
# a toolchain set up by its own action, so the PATH is left alone there.
if [ -z "${CI:-}" ] && [ -d /opt/homebrew/opt/rustup/bin ]; then
    export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
fi

# The root Cargo.toml defines the version once, in [workspace.package].
VERSION="$(awk -F'"' '/^version = /{print $2; exit}' "$ESKIZ_DIR/Cargo.toml")"
if [ -z "$VERSION" ]; then
    echo "could not read the version from $ESKIZ_DIR/Cargo.toml" >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APP="$OUT_DIR/Eskiz.app"
DMG="$OUT_DIR/Eskiz-$VERSION-macos-universal.dmg"

step() { echo; echo "== $*"; }

cd "$ESKIZ_DIR"

step "Rust targets"
rustup target add aarch64-apple-darwin x86_64-apple-darwin

step "Build arm64 and x86_64 (version $VERSION)"
cargo build --release --locked -p eskiz-app --target aarch64-apple-darwin
cargo build --release --locked -p eskiz-app --target x86_64-apple-darwin

step "Universal binary"
lipo -create -output "$WORK/eskiz" \
    target/aarch64-apple-darwin/release/eskiz \
    target/x86_64-apple-darwin/release/eskiz
strip -x "$WORK/eskiz"
lipo -info "$WORK/eskiz"

step "Assemble Eskiz.app"
# Only this package's files are replaced; other outputs in dist/release are kept.
rm -rf "$APP" "$DMG"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$WORK/eskiz" "$APP/Contents/MacOS/eskiz"
chmod 755 "$APP/Contents/MacOS/eskiz"
cp "$ICON" "$APP/Contents/Resources/eskiz.icns"
# Licences and third-party notices travel inside the app (copied before signing so they are sealed).
cp "$ESKIZ_DIR/THIRD_PARTY_NOTICES.md" "$ESKIZ_DIR/LICENSE-MIT" "$ESKIZ_DIR/LICENSE-APACHE" "$APP/Contents/Resources/"
sed "s/0.1.3/$VERSION/g" "$HERE/Info.plist.in" > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
plutil -lint "$APP/Contents/Info.plist"

step "Ad-hoc code signature"
codesign --force --sign - "$APP"
codesign --verify --strict --verbose=2 "$APP"

step "DMG with Applications symlink"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Eskiz $VERSION" -srcfolder "$STAGE" -ov -format UDZO \
    -imagekey zlib-level=9 "$DMG"

step "Artifacts"
ls -la "$DMG"
du -h "$DMG"
echo "app bundle: $APP"
