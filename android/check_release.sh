#!/usr/bin/env bash
# Release packaging checks for the Android build (A5). Run from anywhere:
#   eskiz/android/check_release.sh            build release APK + AAB, then check them
#   eskiz/android/check_release.sh --no-build check the artifacts that already exist
#
# Checks:
#   1. the native library is stored uncompressed in the APK (so it can be mmapped);
#   2. zipalign -c -P 16 -v 4: stored .so files are 16 KB page aligned;
#   3. llvm-objdump -p: every LOAD segment of every packaged libeskiz_app.so says "align 2**14";
#   4. bundletool validate on the AAB (bundletool comes from Google's Maven via Gradle);
#   5. the APK carries the adaptive launcher icon and the AAB is signed (debug key unless a
#      release keystore was given; the debug key is NOT valid for Play).
#
# Needs: ANDROID_HOME (or ANDROID_SDK_ROOT), cargo-ndk, the Rust Android targets, java 17+.
# Real signing keys belong to the maintainer: see DEBUG_KEYSTORE.md.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-/opt/android-sdk}}"
export ANDROID_HOME="$sdk"
lib="libeskiz_app.so"
build=1
[[ "${1:-}" == "--no-build" ]] && build=0

bt="$(ls -d "$sdk"/build-tools/* | sort -V | tail -1)"
ndk="$(ls -d "$sdk"/ndk/* | sort -V | tail -1)"
zipalign="$bt/zipalign"
objdump="$(ls "$ndk"/toolchains/llvm/prebuilt/*/bin/llvm-objdump)"
[[ -f "$here/local.properties" ]] || echo "sdk.dir=$sdk" > "$here/local.properties"

fail=0
ok()   { echo "  ok:   $*"; }
bad()  { echo "  FAIL: $*"; fail=1; }

cd "$here"
if [[ $build == 1 ]]; then
  ./gradlew --no-daemon assembleRelease bundleRelease fetchBundletool
fi
apk="$here/app/build/outputs/apk/release/app-release.apk"
aab="$here/app/build/outputs/bundle/release/app-release.aab"
[[ -f "$apk" ]] || { echo "missing $apk"; exit 2; }
[[ -f "$aab" ]] || { echo "missing $aab"; exit 2; }

echo "== 1. $lib is stored uncompressed"
if unzip -v "$apk" 'lib/*.so' | awk '/\.so$/ {print $2}' | grep -qv '^Stored$'; then
  bad "a native library is compressed"; else ok "all lib/*.so are Stored"; fi

echo "== 2. zipalign -c -P 16 -v 4"
if "$zipalign" -c -P 16 -v 4 "$apk" > /tmp/zipalign.out 2>&1; then
  grep -E "\.so" /tmp/zipalign.out | sed 's/^/        /'
  ok "zipalign: Verification successful (16 KB page aligned)"
else
  tail -5 /tmp/zipalign.out; bad "zipalign -P 16 failed"
fi

echo "== 3. llvm-objdump -p: LOAD segment alignment"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
(cd "$tmp" && unzip -q "$apk" 'lib/*')
n=0
while IFS= read -r so; do
  n=$((n+1))
  aligns="$("$objdump" -p "$so" | awk '$1=="LOAD" {print $NF}' | sort -u | tr '\n' ' ')"
  abi="$(basename "$(dirname "$so")")"
  # 2**14 is 16 KB. arm64 and x86_64 must both be at least that.
  if "$objdump" -p "$so" | awk '$1=="LOAD" {print $NF}' | grep -qv '^2\*\*1[4-9]$'; then
    bad "$abi $lib LOAD align: $aligns"
  else
    ok "$abi $lib LOAD align: $aligns"
  fi
done < <(find "$tmp/lib" -name "$lib")
[[ $n -gt 0 ]] || bad "$lib not found in the APK"

echo "== 4. bundletool validate"
bundletool_dir="$here/app/build/bundletool"
if [[ -d "$bundletool_dir" ]]; then
  if java -cp "$bundletool_dir/*" com.android.tools.build.bundletool.BundleToolMain validate --bundle="$aab" > /tmp/bundletool.out 2>&1; then
    grep -E "^The App Bundle is valid" /tmp/bundletool.out | sed 's/^/        /' || true
    ok "bundletool validate: $(basename "$aab")"
  else tail -15 /tmp/bundletool.out; bad "bundletool validate failed"; fi
else bad "bundletool missing: run ./gradlew fetchBundletool"; fi

echo "== 5. icon and signature"
icon="$("$bt/aapt2" dump badging "$apk" 2>/dev/null | grep -m1 "^application-icon-[0-9]*:" || true)"
# An adaptive icon is an XML resource (res/xx.xml); a legacy icon is a PNG.
if [[ "$icon" == *.xml\'* ]]; then ok "adaptive launcher icon: ${icon#*:}"; else bad "launcher icon is not adaptive: $icon"; fi
if [[ -x "$bt/apksigner" ]] && "$bt/apksigner" verify --print-certs "$apk" | grep -q "certificate DN"; then
  dn="$("$bt/apksigner" verify --print-certs "$apk" | grep -m1 'certificate DN')"
  ok "APK signed: $dn"
  case "$dn" in *"Android Debug"*) echo "        (debug key: fine for CI and sideload, NOT for Play)";; esac
else bad "APK signature did not verify"; fi

echo
if [[ $fail == 0 ]]; then echo "release checks passed"; else echo "release checks FAILED"; exit 1; fi
