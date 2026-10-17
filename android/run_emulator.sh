#!/usr/bin/env bash
# Boot a headless API 36 x86_64 emulator, install the debug APK, launch Eskiz, and print the
# logcat lines that show the wgpu adapter and the first frame.
# Needs hardware acceleration (/dev/kvm). Not run in the A0 cloud session: that machine has no KVM.
set -euo pipefail
export ANDROID_HOME="${ANDROID_HOME:-/opt/android-sdk}"
SDK="$ANDROID_HOME/cmdline-tools/latest/bin"
AVD=eskiz_api36
IMG="system-images;android-36;google_apis;x86_64"
HERE="$(cd "$(dirname "$0")" && pwd)"

[ -e /dev/kvm ] || { echo "no /dev/kvm: the x86_64 emulator would be unusably slow" >&2; exit 1; }
yes | "$SDK/sdkmanager" --licenses >/dev/null || true
"$SDK/sdkmanager" "emulator" "$IMG"
"$SDK/avdmanager" list avd | grep -q "$AVD" || echo no | "$SDK/avdmanager" create avd -n "$AVD" -k "$IMG" -d pixel_tablet
(cd "$HERE" && ./gradlew assembleDebug)
"$ANDROID_HOME/emulator/emulator" -avd "$AVD" -no-window -no-audio -no-boot-anim -gpu swiftshader_indirect >/tmp/emulator.log 2>&1 &
ADB="$ANDROID_HOME/platform-tools/adb"
"$ADB" wait-for-device
until [ "$("$ADB" shell getprop sys.boot_completed | tr -d '\r')" = 1 ]; do sleep 2; done
"$ADB" logcat -c
"$ADB" install -r "$HERE/app/build/outputs/apk/debug/app-debug.apk"
"$ADB" shell am start -n app.eskiz/.EskizActivity
sleep 20
"$ADB" logcat -d -s eskiz:V wgpu:V wgpu_hal:V RustStdoutStderr:V AndroidRuntime:E DEBUG:F | tail -60
