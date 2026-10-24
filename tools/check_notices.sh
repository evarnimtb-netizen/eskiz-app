#!/usr/bin/env bash
# Checks that every locally produced Eskiz package carries THIRD_PARTY_NOTICES.md, LICENSE-MIT and
# LICENSE-APACHE (and that the notices hold the IJG sentence). Fails if any is missing.
#
#   tools/check_notices.sh [dist-dir]      default: dist/release (relative to eskiz/)
#
# Looked at, when present: Eskiz.app (and, on macOS, the DMG), Eskiz-*-windows-x64.zip,
# Eskiz-*-x86_64.AppImage, Eskiz.AppDir, Eskiz-*-linux-x86_64.tar.gz, and any *.apk under
# android/app/build/outputs. Packages that were not built here are reported as "skipped".
# Set REQUIRE_ANY=1 to fail when no package at all was found.
set -uo pipefail
cd "$(dirname "$0")/.."
DIST="${1:-dist/release}"
FILES="THIRD_PARTY_NOTICES.md LICENSE-MIT LICENSE-APACHE"
IJG="This software is based in part on the work of the Independent JPEG Group."
fail=0 checked=0
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

ok()   { echo "ok      $1"; }
bad()  { echo "MISSING $1: $2"; fail=1; }
skip() { echo "skipped $1"; }

# check_listing <package> <prefix-path-in-package> <listing-file>: each FILES entry must be in the listing.
check_listing() {
    local pkg="$1" prefix="$2" list="$3" f missing=0
    for f in $FILES; do
        if ! grep -Eq "(^|/)${prefix}${f}\$" "$list"; then bad "$pkg" "$prefix$f"; missing=1; fi
    done
    [ "$missing" = 0 ] && ok "$pkg ($prefix)"
    checked=$((checked + 1))
}

# check_dir <package> <dir>: unpacked tree, files directly in <dir>.
check_dir() {
    local pkg="$1" dir="$2" f missing=0
    for f in $FILES; do
        [ -s "$dir/$f" ] || { bad "$pkg" "$dir/$f"; missing=1; }
    done
    if [ "$missing" = 0 ]; then
        if grep -qF "$IJG" "$dir/THIRD_PARTY_NOTICES.md"; then ok "$pkg ($dir)"; else bad "$pkg" "IJG sentence in $dir/THIRD_PARTY_NOTICES.md"; fi
    fi
    checked=$((checked + 1))
}

shopt -s nullglob

# macOS: the .app folder, and the DMG when hdiutil exists.
for app in "$DIST"/Eskiz.app; do [ -d "$app" ] || continue; check_dir "$app" "$app/Contents/Resources"; done
for dmg in "$DIST"/*.dmg; do
    if command -v hdiutil >/dev/null; then
        mnt="$TMP/dmg"; mkdir -p "$mnt"
        if hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$dmg" >/dev/null; then
            check_dir "$dmg" "$mnt/Eskiz.app/Contents/Resources"
            hdiutil detach "$mnt" >/dev/null
        else bad "$dmg" "could not mount"; fi
    else skip "$dmg (no hdiutil on this host; check the Eskiz.app folder or run on macOS)"; fi
done

# Windows zips: files sit in the one top folder.
for z in "$DIST"/Eskiz-*-windows-*.zip; do
    unzip -Z1 "$z" > "$TMP/zip.lst" && check_listing "$z" "" "$TMP/zip.lst" || bad "$z" "cannot read"
    unzip -p "$z" '*/THIRD_PARTY_NOTICES.md' > "$TMP/n.md" 2>/dev/null
    grep -qF "$IJG" "$TMP/n.md" || bad "$z" "IJG sentence"
done

# Linux tarballs: share/doc/eskiz/.
for t in "$DIST"/Eskiz-*-linux-*.tar.gz; do
    tar -tzf "$t" > "$TMP/tar.lst" && check_listing "$t" "share/doc/eskiz/" "$TMP/tar.lst" || bad "$t" "cannot read"
    tar -xzOf "$t" --wildcards '*/share/doc/eskiz/THIRD_PARTY_NOTICES.md' > "$TMP/n.md" 2>/dev/null
    grep -qF "$IJG" "$TMP/n.md" || bad "$t" "IJG sentence"
done

# AppDir and AppImage: usr/share/doc/eskiz/.
for d in "$DIST"/Eskiz.AppDir; do [ -d "$d" ] || continue; check_dir "$d" "$d/usr/share/doc/eskiz"; done
for a in "$DIST"/Eskiz-*.AppImage; do
    [ -e "$a" ] || continue
    x="$TMP/appimage"; rm -rf "$x"; mkdir -p "$x"
    # An absolute path, whether DIST was given absolute or relative.
    case "$a" in /*) abs="$a" ;; *) abs="$PWD/$a" ;; esac
    if (cd "$x" && "$abs" --appimage-extract 'usr/share/doc/eskiz/*' >/dev/null 2>&1); then
        check_dir "$a" "$x/squashfs-root/usr/share/doc/eskiz"
    else bad "$a" "cannot extract (not executable here?)"; fi
done

# Android APKs: assets/licenses/.
for apk in android/app/build/outputs/apk/*/*.apk "$DIST"/*.apk; do
    unzip -Z1 "$apk" > "$TMP/apk.lst" && check_listing "$apk" "assets/licenses/" "$TMP/apk.lst" || bad "$apk" "cannot read"
    unzip -p "$apk" assets/licenses/THIRD_PARTY_NOTICES.md > "$TMP/n.md" 2>/dev/null
    grep -qF "$IJG" "$TMP/n.md" || bad "$apk" "IJG sentence"
done

if [ "$checked" = 0 ]; then
    echo "no packages found under $DIST (nothing to check)"
    [ "${REQUIRE_ANY:-0}" = 1 ] && fail=1
fi
[ "$fail" = 0 ] && echo "check_notices: ok ($checked package(s))" || echo "check_notices: FAILED"
exit "$fail"
