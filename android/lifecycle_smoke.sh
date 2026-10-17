#!/usr/bin/env bash
# Lifecycle smoke test for the Android build: the checks run by hand on the
# emulator. Needs a running emulator/device and adb; NOT run by CI. Usage:
#
#   android/lifecycle_smoke.sh [-s SERIAL] [--apk eskiz.apk] [--out DIR] [--idle SECS]
#
# Steps (screenshots in DIR, verdicts printed at the end):
#   1. install, launch, tap the first gallery card (the document must already exist: create one
#      with + first, or run input_smoke.sh once) so a canvas is open
#   2. idle --idle seconds (default 45, longer than eframe's 30 s save), then ONE finger swipe:
#      the screenshot must differ from before (first stroke after idle is not dropped)
#   3. home, return: the screenshot must equal the one before leaving
#   4. four rotations (user_rotation 1,2,3,0): app alive after each, a screenshot per rotation
#   5. swipe, home, `am kill`, relaunch: gallery shows the document; open it and the canvas must
#      equal the screenshot taken before the kill (crash-recovery journal)
# Exit code 0 only when every automatic verdict passed.
# The "equal" checks compare the canvas area only, with a small tolerance (tools/canvas_diff.py, needs python3).
# Look at the PNGs for the rest (status bar clear of the top bar, canvas refit after a rotation).
set -u
serial_args=(); apk="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/app/build/outputs/apk/release/app-release.apk"
out=""; idle=45
while [[ $# -gt 0 ]]; do
  case "$1" in
    -s) serial_args=(-s "$2"); shift 2 ;;
    --apk) apk="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --idle) idle="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
adb_bin="${ADB:-$(command -v adb || echo adb)}"  # a full path: the adb() wrapper below must not call itself
adb() { "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} "$@"; }
# GNU `timeout` (Linux), `gtimeout` (Homebrew coreutils on macOS), else run without a limit.
if command -v timeout >/dev/null; then t() { local s="$1"; shift; timeout "$s" "$@"; }
elif command -v gtimeout >/dev/null; then t() { local s="$1"; shift; gtimeout "$s" "$@"; }
else t() { shift; "$@"; }; fi
command -v "$adb_bin" > /dev/null || { echo "adb not found (set ADB=...)" >&2; exit 2; }
[[ -f "$apk" ]] || { echo "APK not found: $apk" >&2; exit 2; }
[[ -n "$out" ]] || out="lifecycle-smoke-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"
log() { echo "[lifecycle] $*" | tee -a "$out/steps.txt"; }
fail=0
verdict() { # verdict "text" 0|1 (1 = failed)
  if [[ "$2" == 0 ]]; then log "PASS $1"; else log "FAIL $1"; fail=1; fi
}
shot() { t 30 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} exec-out screencap -p > "$out/$1.png" 2> /dev/null; }
# Only the canvas area is compared (tools/canvas_diff.py crops out the status bar, top bar, sidebar and
# toasts) with a tolerance: "same" = at most 0.05 % of the canvas pixels differ, "changed" = more.
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
diff_pct() { python3 -I "$here/tools/canvas_diff.py" "$out/$1.png" "$out/$2.png" 2> /dev/null || echo 100; }
same() { local p; p="$(diff_pct "$1" "$2")"; log "canvas diff $1 vs $2: ${p}%"; awk -v p="$p" 'BEGIN { exit !(p <= 0.05) }'; }
alive() { [[ -n "$(adb shell pidof app.eskiz 2> /dev/null | tr -d '\r')" ]]; }

t 240 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} wait-for-device || { log "no device"; exit 3; }
until [[ "$(adb shell getprop sys.boot_completed 2> /dev/null | tr -d '\r')" == "1" ]]; do sleep 2; done
adb shell input keyevent KEYCODE_WAKEUP > /dev/null 2>&1; adb shell wm dismiss-keyguard > /dev/null 2>&1
adb shell settings put system accelerometer_rotation 0
adb shell settings put system user_rotation 0
t 180 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} install -r "$apk" > "$out/install.txt" 2>&1 || { log "install failed"; exit 4; }

size="$(adb shell wm size | tr -d '\r' | grep -Eo '[0-9]+x[0-9]+' | tail -1)"
W="${size%x*}"; H="${size#*x}"
launch() { adb shell am force-stop app.eskiz; adb logcat -c; adb shell am start -n app.eskiz/.EskizActivity > /dev/null 2>&1
  for _ in $(seq 1 30); do alive && break; sleep 1; done; sleep 8; }
swipe() { t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input touchscreen swipe "$1" "$2" "$3" "$4" 500; sleep 1; }
open_first() { t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input touchscreen tap "$((W * 15 / 100))" "$((H * 25 / 100))"; sleep 3; }

log "1. launch and open the first document (screen ${W}x${H})"
launch; shot 00-gallery; open_first; shot 01-canvas-open
alive; verdict "app alive after launch" $?

log "2. idle ${idle}s then one swipe"
sleep "$idle"; shot 02-before-idle-swipe
swipe "$((W * 30 / 100))" "$((H * 45 / 100))" "$((W * 70 / 100))" "$((H * 45 / 100))"; shot 03-after-idle-swipe
same 02-before-idle-swipe 03-after-idle-swipe; verdict "first stroke after ${idle}s idle draws" $(( $? == 0 ? 1 : 0 ))

log "3. home and return"
shot 04-before-home
adb shell input keyevent KEYCODE_HOME; sleep 3
adb shell am start -n app.eskiz/.EskizActivity > /dev/null 2>&1; sleep 5; shot 05-after-return
alive; verdict "app alive after home/return" $?
same 04-before-home 05-after-return; verdict "canvas unchanged after home/return" $?

log "4. four rotations"
for r in 1 2 3 0; do
  adb shell settings put system user_rotation "$r"; sleep 4; shot "06-rotation-$r"
  alive; verdict "app alive in rotation $r" $?
done
adb shell settings put system accelerometer_rotation 1

log "5. stroke, home, kill, relaunch"
swipe "$((W * 30 / 100))" "$((H * 55 / 100))" "$((W * 70 / 100))" "$((H * 60 / 100))"; shot 07-before-kill
adb shell input keyevent KEYCODE_HOME; sleep 3
adb shell am kill app.eskiz; sleep 2
alive && log "note: process still alive after am kill (the system may keep it; use 'adb shell am force-stop' is NOT equivalent)"
adb shell am start -n app.eskiz/.EskizActivity > /dev/null 2>&1; sleep 10; shot 08-gallery-after-kill
open_first; sleep 2; shot 09-canvas-after-kill
same 07-before-kill 09-canvas-after-kill; verdict "canvas after kill and relaunch equals the one before" $?

adb logcat -d -v time > "$out/logcat-full.txt" 2>&1
grep -E "eskiz|AndroidRuntime|FATAL|DEBUG|libc" "$out/logcat-full.txt" > "$out/logcat.txt" 2> /dev/null
crashes="$(grep -cE "FATAL EXCEPTION|SIGSEGV|panicked" "$out/logcat.txt")"
verdict "no crashes in the log ($crashes)" $(( crashes > 0 ? 1 : 0 ))
log "files in $out"
exit "$fail"
