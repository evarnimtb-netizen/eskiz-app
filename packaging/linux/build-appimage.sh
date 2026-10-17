#!/usr/bin/env bash
# Assembles the Linux release from a built eskiz binary:
#
#   1. an AppDir, turned into Eskiz-<version>-x86_64.AppImage when appimagetool is available.
#      It bundles libxkbcommon-x11.so.0 and libxkbcommon.so.0 from this machine in usr/lib, and
#      AppRun puts that directory on LD_LIBRARY_PATH;
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
# Libraries winit loads with dlopen that are not on every system. They are bundled in the AppImage.
# Debian/Ubuntu packages: libxkbcommon-x11-0 and libxkbcommon0 (CI installs the first).
BUNDLED_LIBS="libxkbcommon-x11.so.0 libxkbcommon.so.0"

step() { echo; echo "== $*"; }

# Prints the path of a shared library on this machine: ldconfig's cache first, then the usual
# library directories. Fails if the library is not found.
find_host_lib() {
    local soname="$1" path="" dir ldconfig_bin
    ldconfig_bin="$(command -v ldconfig || echo /sbin/ldconfig)"
    if [ -x "$ldconfig_bin" ]; then
        path="$("$ldconfig_bin" -p | awk -v s="$soname" '$1 == s { print $NF; exit }')"
    fi
    if [ -z "$path" ] || [ ! -e "$path" ]; then
        path=""
        for dir in /usr/lib/x86_64-linux-gnu /lib/x86_64-linux-gnu /usr/lib64 /usr/lib /lib64; do
            if [ -e "$dir/$soname" ]; then
                path="$dir/$soname"
                break
            fi
        done
    fi
    [ -n "$path" ] || return 1
    printf '%s\n' "$path"
}

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

# Copies the licence files of the Debian/Ubuntu packages that own the bundled libxkbcommon libraries
# into $1/share/doc/eskiz/libxkbcommon/ (from /usr/share/doc/<package>/copyright). Skipped with a
# note when the build host has no such file (other distributions keep them elsewhere).
copy_xkbcommon_licence() {
    local root="$1" pkg found=0
    for pkg in libxkbcommon0 libxkbcommon-x11-0; do
        if [ -f "/usr/share/doc/$pkg/copyright" ]; then
            mkdir -p "$root/share/doc/eskiz/libxkbcommon"
            cp "/usr/share/doc/$pkg/copyright" "$root/share/doc/eskiz/libxkbcommon/copyright-$pkg"
            found=1
        fi
    done
    if [ "$found" = 0 ]; then
        echo "note: no libxkbcommon copyright file under /usr/share/doc on this host; not copied" >&2
    fi
}

step "AppDir"
layout_usr "$APPDIR/usr"
cp "$HERE/eskiz.desktop" "$APPDIR/eskiz.desktop"
cp "$PNG_DIR/eskiz-512.png" "$APPDIR/eskiz.png"
ln -s eskiz.png "$APPDIR/.DirIcon"

step "Bundled runtime libraries"
mkdir -p "$APPDIR/usr/lib"
copy_xkbcommon_licence "$APPDIR/usr"
for soname in $BUNDLED_LIBS; do
    if ! src="$(find_host_lib "$soname")"; then
        echo "$soname not found on this machine. Install libxkbcommon-x11-0 (Debian/Ubuntu) or" >&2
        echo "libxkbcommon-x11 (Fedora), then build again." >&2
        exit 1
    fi
    # -L copies the file behind any symlink under the soname the loader asks for.
    cp -L "$src" "$APPDIR/usr/lib/$soname"
    echo "  $soname <- $src"
done

cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export LD_LIBRARY_PATH
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

The tarball does not bundle any libraries. Install these packages first. The AppImage bundles
libxkbcommon-x11.so.0 and libxkbcommon.so.0, but it still needs the rest.

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
