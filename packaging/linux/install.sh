#!/bin/sh
# Installs the Eskiz tarball for the current user (no root needed).
#
#   ./install.sh                 installs under ~/.local
#   PREFIX=/usr/local sudo -E ./install.sh   installs system-wide
#
# Copies the binary, the desktop entry, the hicolor icons and the MIME definitions, then
# refreshes the desktop and MIME databases when the tools are present.
#
# Runtime libraries are not bundled in this tarball. Install these first (DEPENDENCIES.txt has
# the full list):
#   Debian/Ubuntu: libxkbcommon-x11-0 libxkbcommon0 libx11-xcb1 libwayland-client0 libgl1 libvulkan1
#                  libgl1-mesa-dri mesa-vulkan-drivers
#   Fedora:        libxkbcommon-x11 libxkbcommon libX11-xcb libwayland-client mesa-libGL
#                  vulkan-loader mesa-dri-drivers mesa-vulkan-drivers
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"

mkdir -p "$PREFIX/bin"
cp "$HERE/bin/eskiz" "$PREFIX/bin/eskiz"
chmod 755 "$PREFIX/bin/eskiz"

cp -R "$HERE/share/." "$PREFIX/share/"

if command -v update-mime-database >/dev/null 2>&1; then
    update-mime-database "$PREFIX/share/mime" || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$PREFIX/share/applications" || true
fi

echo "Installed Eskiz to $PREFIX/bin/eskiz"
echo "Runtime libraries are listed in DEPENDENCIES.txt next to this script."
