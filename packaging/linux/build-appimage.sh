#!/usr/bin/env bash
# Assembles the Linux release from a built eskiz binary:
#
#   1. an AppDir, turned into Eskiz-<version>-x86_64.AppImage when appimagetool is available.
#      It bundles no system libraries: the build machine's libraries need a newer glibc than the
#      2.28 the binary is built for. A check fails the build if any ELF file in the AppDir needs
#      a GLIBC symbol newer than 2.28 (override with MAX_GLIBC);
#   2. a portable Eskiz-<version>-linux-x86_64.tar.gz with install.sh and DEPENDENCIES.txt.
#
# Usage: build-appimage.sh [path/to/eskiz]
#   default binary: target/x86_64-unknown-linux-gnu/release/eskiz (relative to eskiz/)
#
# Environment:
#   APPIMAGETOOL  path to appimagetool (default: appimagetool on PATH). CI downloads it.
#                 Locally, if it is missing the AppImage step is skipped with a message.
#                 When CI is set, a missing appimagetool is an error.
#   VERSION       overrides the version read from the root Cargo.toml.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ESKIZ_DIR="$(cd "$HERE/../.." && pwd)"
PNG_DIR="$ESKIZ_DIR/packaging/icons/png"
OUT_DIR="$ESKIZ_DIR/dist/release"

# Keeps macOS tar from adding AppleDouble (._*) files to the archives.
export COPYFILE_DISABLE=1

BIN="${1:-$ESKIZ_DIR/target/x86_64-unknown-linux-gnu/release/eskiz}"
if [ ! -f "$BIN" ]; then
    echo "eskiz binary not found: $BIN" >&2
    echo "build it first: cargo zigbuild --release -p eskiz-app --target x86_64-unknown-linux-gnu.2.28" >&2
    exit 1
fi

VERSION="${VERSION:-$(awk -F'"' '/^version = /{print $2; exit}' "$ESKIZ_DIR/Cargo.toml")}"
NAME="Eskiz-$VERSION-linux-x86_64"
APPDIR="$OUT_DIR/Eskiz.AppDir"
TARDIR="$OUT_DIR/$NAME"
ICON_SIZES="16 32 48 64 128 256 512"
# The oldest glibc the AppImage promises to run on.
MAX_GLIBC="${MAX_GLIBC:-2.28}"

# Fails if an ELF file under $1 needs a GLIBC_x.y symbol version newer than $MAX_GLIBC.
check_glibc_baseline() {
    local root="$1" f v bad=0 seen=0
    command -v objdump >/dev/null || { echo "objdump not found (install binutils)" >&2; return 1; }
    while IFS= read -r f; do
        if ! head -c 4 "$f" 2>/dev/null | grep -q 'ELF'; then continue; fi
        seen=$((seen + 1))
        for v in $(objdump -T "$f" 2>/dev/null | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -u); do
            if [ "$(printf '%s\n%s\n' "$v" "$MAX_GLIBC" | sort -V | tail -n1)" != "$MAX_GLIBC" ]; then
                echo "error: ${f#"$root"/} needs GLIBC_$v, newer than the GLIBC_$MAX_GLIBC baseline" >&2
                bad=1
            fi
        done
    done < <(find "$root" -type f)
    [ "$seen" -gt 0 ] || { echo "error: no ELF files found under $root" >&2; return 1; }
    return "$bad"
}

step() { echo; echo "== $*"; }

mkdir -p "$OUT_DIR"
rm -rf "$APPDIR" "$TARDIR" "$OUT_DIR/$NAME.tar.gz"

# Lays out the usr/ tree shared by the AppDir and the tarball: bin/, share/.
# $1 is the destination root.
layout_usr() {
    local root="$1"
    mkdir -p "$root/bin" "$root/share/applications" "$root/share/mime/packages"
    cp "$BIN" "$root/bin/eskiz"
    chmod 755 "$root/bin/eskiz"
    cp "$HERE/eskiz.desktop" "$root/share/applications/eskiz.desktop"
    cp "$HERE/eskiz.xml" "$root/share/mime/packages/eskiz.xml"
    for size in $ICON_SIZES; do
        mkdir -p "$root/share/icons/hicolor/${size}x${size}/apps"
        cp "$PNG_DIR/eskiz-$size.png" "$root/share/icons/hicolor/${size}x${size}/apps/eskiz.png"
    done
    # Licences and third-party notices (AppImage: usr/share/doc/eskiz, tarball: share/doc/eskiz).
    mkdir -p "$root/share/doc/eskiz"
    cp "$ESKIZ_DIR/THIRD_PARTY_NOTICES.md" "$ESKIZ_DIR/LICENSE-MIT" "$ESKIZ_DIR/LICENSE-APACHE" \
        "$root/share/doc/eskiz/"
}

step "AppDir"
layout_usr "$APPDIR/usr"
cp "$HERE/eskiz.desktop" "$APPDIR/eskiz.desktop"
cp "$PNG_DIR/eskiz-512.png" "$APPDIR/eskiz.png"
ln -s eskiz.png "$APPDIR/.DirIcon"

step "glibc baseline (GLIBC_$MAX_GLIBC)"
check_glibc_baseline "$APPDIR"

cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/eskiz" "$@"
APPRUN
chmod 755 "$APPDIR/AppRun"

step "AppImage"
TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
APPIMAGE="$OUT_DIR/Eskiz-$VERSION-x86_64.AppImage"
if [ -n "$TOOL" ]; then
    ARCH=x86_64 "$TOOL" "$APPDIR" "$APPIMAGE"
elif [ -n "${CI:-}" ]; then
    echo "appimagetool not found (set APPIMAGETOOL)" >&2
    exit 1
else
    echo "skipped: appimagetool not found. Set APPIMAGETOOL=/path/to/appimagetool to build the AppImage."
fi

step "Portable tarball"
layout_usr "$TARDIR"
cp "$HERE/install.sh" "$TARDIR/install.sh"
chmod 755 "$TARDIR/install.sh"
cat > "$TARDIR/DEPENDENCIES.txt" <<'DEPS'
Eskiz runtime dependencies (Linux, x86_64)

Neither the tarball nor the AppImage bundles any libraries. Install these packages first.

Any session (drawing):
  Debian/Ubuntu: libgl1 libvulkan1 libgl1-mesa-dri mesa-vulkan-drivers
  Fedora:        mesa-libGL vulkan-loader mesa-dri-drivers mesa-vulkan-drivers

X11 session (DISPLAY set, WAYLAND_DISPLAY not set):
  Debian/Ubuntu: libxkbcommon-x11-0 libxkbcommon0 libx11-xcb1
  Fedora:        libxkbcommon-x11 libxkbcommon libX11-xcb

Wayland session (WAYLAND_DISPLAY set):
  Debian/Ubuntu: libxkbcommon0 libwayland-client0
  Fedora:        libxkbcommon libwayland-client

All of them, Debian/Ubuntu:
  sudo apt install libxkbcommon-x11-0 libxkbcommon0 libx11-xcb1 libwayland-client0 libgl1 libvulkan1 libgl1-mesa-dri mesa-vulkan-drivers

All of them, Fedora:
  sudo dnf install libxkbcommon-x11 libxkbcommon libX11-xcb libwayland-client mesa-libGL vulkan-loader mesa-dri-drivers mesa-vulkan-drivers

If Eskiz exits with "missing system libraries", the message names the packages it needs.
DEPS
tar -C "$OUT_DIR" -czf "$OUT_DIR/$NAME.tar.gz" "$NAME"
rm -rf "$TARDIR"

step "Artifacts"
ls -la "$OUT_DIR"
