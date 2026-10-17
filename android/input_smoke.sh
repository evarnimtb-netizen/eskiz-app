#!/usr/bin/env bash
# Input smoke test for the Android build (A2). Needs a running emulator or a device with USB
# debugging, and adb. NOT run by CI (no emulator in the cloud). Usage:
#
#   android/input_smoke.sh [-s SERIAL] [--apk eskiz.apk] [--out DIR] [--boot-timeout SECS]
#
# It installs the APK, launches Eskiz, then drives the screen with
#   adb shell input stylus swipe ...      (tool type STYLUS: the pen path)
#   adb shell input touchscreen swipe ... (tool type FINGER: the touch path)
#   two finger swipes started together    (best effort, see below)
# (see also lifecycle_smoke.sh for the idle / home / rotation / kill checks)
# and after each step saves a screenshot (adb exec-out screencap -p) plus the logcat lines the
# app wrote ("eskiz" tag: input: ...). Everything lands in DIR (default ./input-smoke-<time>).
# Look at the PNGs: a stroke should be visible after the stylus and the finger swipe, the
# two-finger step should pan/zoom the canvas, and logcat.txt should contain "input: pen down",
# "input: pen up" for the stylus swipe. Exit code 0 means the script ran, not that it passed:
# the verdict is the screenshots and the log, which the script summarises at the end.
#
# `adb shell input` cannot inject a real two-pointer gesture (every command is its own
# one-pointer stream). The script starts two swipes in parallel anyway; if the emulator shows
# no pinch, use the emulator's extended controls (Virtual sensors / multi-touch with
# Ctrl+click-drag) to check pinch, rotate and the two-finger tap undo by hand.
set -u

serial_args=()
apk="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/app/build/outputs/apk/release/app-release.apk"
out=""
boot_timeout=240
while [[ $# -gt 0 ]]; do
  case "$1" in
    -s) serial_args=(-s "$2"); shift 2 ;;
    --apk) apk="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --boot-timeout) boot_timeout="$2"; shift 2 ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
adb_bin="${ADB:-$(command -v adb || echo adb)}"  # a full path: the adb() wrapper below must not call itself
adb() { "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} "$@"; }
# A command that can hang on a wedged emulator must not hang the script.
# GNU `timeout` (Linux), `gtimeout` (Homebrew coreutils on macOS), else run without a limit.
if command -v timeout >/dev/null; then t() { local s="$1"; shift; timeout "$s" "$@"; }
elif command -v gtimeout >/dev/null; then t() { local s="$1"; shift; gtimeout "$s" "$@"; }
else t() { shift; "$@"; }; fi

command -v "$adb_bin" > /dev/null || { echo "adb not found (set ADB=/path/to/adb)" >&2; exit 2; }
[[ -f "$apk" ]] || { echo "APK not found: $apk (build it or pass --apk)" >&2; exit 2; }
[[ -n "$out" ]] || out="input-smoke-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"
log() { echo "[smoke] $*" | tee -a "$out/steps.txt"; }

log "waiting for a device (max ${boot_timeout}s)"
t "$boot_timeout" "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} wait-for-device || { log "no device"; exit 3; }
deadline=$((SECONDS + boot_timeout))
until [[ "$(adb shell getprop sys.boot_completed 2> /dev/null | tr -d '\r')" == "1" ]]; do
  (( SECONDS < deadline )) || { log "device did not finish booting"; exit 3; }
  sleep 2
done
log "booted: $(adb shell getprop ro.product.model | tr -d '\r') API $(adb shell getprop ro.build.version.sdk | tr -d '\r') $(adb shell getprop ro.product.cpu.abi | tr -d '\r')"
adb shell input keyevent KEYCODE_WAKEUP > /dev/null 2>&1
adb shell wm dismiss-keyguard > /dev/null 2>&1

log "installing $apk"
t 180 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} install -r "$apk" > "$out/install.txt" 2>&1 || { cat "$out/install.txt"; log "install failed"; exit 4; }

adb shell am force-stop app.eskiz
adb logcat -c
log "launching"
t 30 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell am start -n app.eskiz/.EskizActivity > "$out/launch.txt" 2>&1 || true
# Wait for the process and a few seconds for the first frames.
for _ in $(seq 1 30); do
  [[ -n "$(adb shell pidof app.eskiz 2> /dev/null | tr -d '\r')" ]] && break
  sleep 1
done
sleep 8

shot() { t 30 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} exec-out screencap -p > "$out/$1.png" 2> /dev/null; log "screenshot $1.png ($(wc -c < "$out/$1.png" 2> /dev/null || echo 0) bytes)"; }
mark() { adb shell log -t eskiz-smoke "$1" > /dev/null 2>&1; }

size="$(adb shell wm size | tr -d '\r' | grep -Eo '[0-9]+x[0-9]+' | tail -1)"
W="${size%x*}"; H="${size#*x}"
W="${W:-1080}"; H="${H:-1920}"
log "screen ${W}x${H}"
# Canvas area: the middle of the screen. Strokes are well inside it.
x1=$((W * 30 / 100)); x2=$((W * 70 / 100)); y1=$((H * 40 / 100)); y2=$((H * 45 / 100)); y3=$((H * 55 / 100))

shot 00-launch

mark "step: stylus swipe"
t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input stylus swipe "$x1" "$y1" "$x2" "$y2" 600
sleep 1; shot 01-after-stylus-swipe

mark "step: finger swipe"
t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input touchscreen swipe "$x1" "$y3" "$x2" "$y3" 600
sleep 1; shot 02-after-finger-swipe

mark "step: stylus tap then finger tap"
t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input stylus tap "$((W / 2))" "$((H / 2))"
t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input touchscreen tap "$((W / 2))" "$((H * 60 / 100))"
sleep 1; shot 03-after-taps

mark "step: two parallel finger swipes (best effort)"
( t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input touchscreen swipe "$((W * 40 / 100))" "$((H * 50 / 100))" "$((W * 25 / 100))" "$((H * 50 / 100))" 700 ) &
( t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input touchscreen swipe "$((W * 60 / 100))" "$((H * 50 / 100))" "$((W * 75 / 100))" "$((H * 50 / 100))" 700 ) &
wait
sleep 1; shot 04-after-two-finger-swipes

mark "step: done"
adb logcat -d -v time > "$out/logcat-full.txt" 2>&1
grep -E "eskiz|AndroidRuntime|FATAL|DEBUG|libc" "$out/logcat-full.txt" > "$out/logcat.txt" 2> /dev/null
alive="$(adb shell pidof app.eskiz 2> /dev/null | tr -d '\r')"

echo
log "summary"
log "  app still running: $([[ -n "$alive" ]] && echo yes || echo "NO, crashed?")"
for k in "pen down" "pen up" "pen cancel" "finger"; do
  log "  logcat lines with 'input: $k': $(grep -c "input: $k" "$out/logcat.txt")"
done
log "  crashes in log: $(grep -cE "FATAL EXCEPTION|SIGSEGV|panicked" "$out/logcat.txt")"
if command -v cmp > /dev/null; then
  for pair in "00-launch 01-after-stylus-swipe" "01-after-stylus-swipe 02-after-finger-swipe"; do
    set -- $pair
    cmp -s "$out/$1.png" "$out/$2.png" && log "  $1 and $2 are IDENTICAL (nothing drawn?)" || log "  $1 -> $2 changed"
  done
fi
log "files in $out"
[[ -n "$alive" ]]
