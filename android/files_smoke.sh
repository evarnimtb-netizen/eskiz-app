#!/usr/bin/env bash
# Files, share, clipboard and Back smoke test (Android milestone A4) on an emulator. Needs adb and a
# running emulator (API 33+ image, finger input only); NOT run by CI. Usage:
#
#   android/files_smoke.sh [-s SERIAL] --apk app-debug.apk --test-apk sharetest-debug.apk [--out DIR]
#                          [--fixtures DIR] [--big-file FILE]
#
# --apk       the DEBUGGABLE app (test hooks only work in a debuggable build)
# --test-apk  the sharetest app: share target and clipboard reader/writer (app.eskiz.sharetest)
# --fixtures  fixture.procreate, fixture.png, fixture.brushset, fixture.swatches, ref.png; written with
#             `cargo test -p eskiz-app --lib -- --ignored write_android_fixtures` when missing
# --big-file  optional step 16: a large .procreate to open (copy time and no ANR)
#
# Every step waits for its log line with a timeout, saves a screenshot, and writes PASS or FAIL to
# DIR/steps.txt and DIR/summary.md. Taps use `uiautomator dump` and tools/ui_find.py (by text, never by
# coordinates); when a node is missing the dump is saved next to the screenshots. The script clears
# /sdcard/Documents/EskizTest and /sdcard/Download/eskiz-* first, so a rerun starts clean.
# Exit code 0 only when every automatic verdict passed.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
serial_args=(); apk=""; test_apk=""; out=""; fixtures=""; big=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -s) serial_args=(-s "$2"); shift 2 ;;
    --apk) apk="$2"; shift 2 ;;
    --test-apk) test_apk="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --fixtures) fixtures="$2"; shift 2 ;;
    --big-file) big="$2"; shift 2 ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
adb_bin="${ADB:-$(command -v adb || echo adb)}"
[[ -z "${ANDROID_HOME:-}" ]] || [[ -x "$adb_bin" ]] || adb_bin="$ANDROID_HOME/platform-tools/adb"
adb() { "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} "$@"; }
if command -v timeout >/dev/null; then t() { local s="$1"; shift; timeout "$s" "$@"; }
elif command -v gtimeout >/dev/null; then t() { local s="$1"; shift; gtimeout "$s" "$@"; }
else t() { shift; "$@"; }; fi
command -v "$adb_bin" > /dev/null || { echo "adb not found (set ADB=...)" >&2; exit 2; }
[[ -f "$apk" ]] || { echo "--apk FILE required (not found: $apk)" >&2; exit 2; }
[[ -f "$test_apk" ]] || { echo "--test-apk FILE required (not found: $test_apk)" >&2; exit 2; }
[[ -n "$out" ]] || out="files-smoke-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"
[[ -n "$fixtures" ]] || fixtures="$out/fixtures"
log() { echo "[files] $*" | tee -a "$out/steps.txt"; }
fail=0
verdict() { # verdict "text" STATUS (0 = passed)
  if [[ "$2" == 0 ]]; then log "PASS $1"; echo "- PASS $1" >> "$out/summary.md"
  else log "FAIL $1"; echo "- FAIL $1" >> "$out/summary.md"; fail=1; fi
}
: > "$out/summary.md"; echo "# files_smoke summary" >> "$out/summary.md"
shot() { t 30 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} exec-out screencap -p > "$out/$1.png" 2> /dev/null; }
logs() { adb logcat -d 2> /dev/null | tr -d '\r'; }
# wait_log PATTERN SECONDS: wait until the pattern shows in logcat (since the last clear).
wait_log() { local end=$((SECONDS + $2)); while (( SECONDS < end )); do logs | grep -Eq -- "$1" && return 0; sleep 1; done; return 1; }
has_log() { logs | grep -Eq -- "$1"; }
alive() { [[ -n "$(adb shell pidof app.eskiz 2> /dev/null | tr -d '\r')" ]]; }
clear_logs() { adb logcat -c; }
sha_of() { if command -v sha256sum > /dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

# --- UI helpers (uiautomator + ui_find.py) ---
dump_ui() { adb shell uiautomator dump /sdcard/eskiz-ui.xml > /dev/null 2>&1; adb pull /sdcard/eskiz-ui.xml "$out/ui.xml" > /dev/null 2>&1; }
# find_node ARGS...: print "X Y" for a node of the current screen (ui_find.py arguments).
find_node() { dump_ui; python3 -I "$here/tools/ui_find.py" "$out/ui.xml" "$@" 2> /dev/null; }
tap_xy() { t 20 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} shell input tap "$1" "$2"; sleep 1; }
# tap_node SECONDS ARGS...: wait for the node, tap it. On failure keep the dump and a screenshot.
tap_node() {
  local secs="$1"; shift; local end=$((SECONDS + secs)) xy
  while (( SECONDS < end )); do
    xy="$(find_node "$@")" && { tap_xy $xy; return 0; }
    sleep 1
  done
  cp "$out/ui.xml" "$out/missing-$(date +%H%M%S).xml" 2> /dev/null; shot "missing-$(date +%H%M%S)"
  log "node not found: $*"; return 1
}
hook() { adb shell am start -n app.eskiz/.EskizActivity --es eskiz_test "$1" > /dev/null 2>&1; }
key() { adb shell input keyevent "$1"; sleep 1; }
# pick_in_downloads NAME: in the system picker, open Downloads and tap the file.
pick_in_downloads() {
  if ! tap_node 6 --text "$1"; then
    tap_node 10 --desc "Show roots" || return 1
    tap_node 10 --text "Downloads" || return 1
    tap_node 10 --text "$1" || return 1
  fi
}
launch() { adb shell am force-stop app.eskiz; clear_logs; adb shell am start -n app.eskiz/.EskizActivity > /dev/null 2>&1
  for _ in $(seq 1 30); do alive && break; sleep 1; done; sleep 6; }

# --- setup ---
t 240 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} wait-for-device || { log "no device"; exit 3; }
until [[ "$(adb shell getprop sys.boot_completed 2> /dev/null | tr -d '\r')" == "1" ]]; do sleep 2; done
adb shell input keyevent KEYCODE_WAKEUP > /dev/null 2>&1; adb shell wm dismiss-keyguard > /dev/null 2>&1
adb shell settings put system accelerometer_rotation 0
adb shell settings put system user_rotation 0
if [[ ! -f "$fixtures/fixture.procreate" ]]; then
  log "writing fixtures to $fixtures"
  (cd "$here/.." && ESKIZ_FIXTURES_DIR="$(cd "$(dirname "$fixtures")" && pwd)/$(basename "$fixtures")" \
    cargo test -p eskiz-app --lib -- --ignored write_android_fixtures > "$out/fixtures.log" 2>&1) || { log "fixtures failed: see fixtures.log"; exit 5; }
fi
for f in fixture.procreate fixture.png fixture.brushset fixture.swatches ref.png; do
  [[ -f "$fixtures/$f" ]] || { log "missing fixture $f"; exit 5; }
done
t 180 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} install -r -g "$apk" > "$out/install.txt" 2>&1 || { log "install failed"; exit 4; }
t 180 "$adb_bin" ${serial_args[@]+"${serial_args[@]}"} install -r -g "$test_apk" > "$out/install-test.txt" 2>&1 || { log "install of the test app failed"; exit 4; }
adb shell rm -rf /sdcard/Documents/EskizTest '/sdcard/Download/eskiz-*' > /dev/null 2>&1
adb shell mkdir -p /sdcard/Documents /sdcard/Download
for f in fixture.procreate fixture.png fixture.brushset fixture.swatches; do adb push "$fixtures/$f" "/sdcard/Download/$f" > /dev/null; done
adb shell am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d file:///sdcard/Download > /dev/null 2>&1
adb shell pm clear app.eskiz > /dev/null 2>&1
size="$(adb shell wm size | tr -d '\r' | grep -Eo '[0-9]+x[0-9]+' | tail -1)"; W="${size%x*}"; H="${size#*x}"
log "device ${W}x${H}"
launch; shot 00-gallery
alive; verdict "app alive after launch" $?

# 1 open .procreate
log "1. open .procreate"
clear_logs; hook open_document
if pick_in_downloads fixture.procreate; then
  wait_log "saf: picked" 30 && wait_log "saf: open ok" 30; verdict "1 open .procreate (picked, open ok)" $?
else verdict "1 open .procreate (picker node)" 1; fi
shot 01-opened

# 2 choose folder
log "2. choose folder"
adb shell mkdir -p /sdcard/Documents/EskizTest
clear_logs; hook choose_folder
ok=1
if tap_node 15 --desc "Show roots" || true; then
  tap_node 8 --text "Documents" || tap_node 8 --text-contains "documents"
  tap_node 10 --text "EskizTest" \
    && tap_node 10 --text-contains "use this folder" \
    && { tap_node 10 --text "Allow" || tap_node 10 --text-contains "allow"; } && ok=0
fi
wait_log "saf: tree persisted" 20 || ok=1; verdict "2 choose folder (tree persisted)" $ok
shot 02-folder

# 3 save PNG
log "3. save PNG"
clear_logs; hook export:png:save
ok=1; wait_log "saf: committed" 40 && ok=0; verdict "3 save PNG (committed)" $ok
rm -rf "$out/pulled"; mkdir -p "$out/pulled"
adb pull /sdcard/Documents/EskizTest "$out/pulled/" > /dev/null 2>&1
png3="$(ls "$out"/pulled/EskizTest/*.png 2> /dev/null | head -1)"
if [[ -n "$png3" ]]; then python3 -I "$here/tools/png_same.py" "$png3" "$fixtures/ref.png" | tee -a "$out/steps.txt"; verdict "3 saved PNG equals ref.png" "${PIPESTATUS[0]}"
else verdict "3 saved PNG exists" 1; fi
shot 03-saved

# 4 save PSD and .procreate
log "4. save PSD and .procreate"
clear_logs; hook export:psd:save; wait_log "saf: committed" 40; sleep 2
hook export:procreate:save; wait_log "saf: committed" 40; sleep 2
rm -rf "$out/pulled"; mkdir -p "$out/pulled"; adb pull /sdcard/Documents/EskizTest "$out/pulled/" > /dev/null 2>&1
psd="$(ls "$out"/pulled/EskizTest/*.psd 2> /dev/null | head -1)"; pc="$(ls "$out"/pulled/EskizTest/*.procreate 2> /dev/null | head -1)"
[[ -n "$psd" && "$(head -c 4 "$psd")" == "8BPS" ]]; verdict "4 PSD saved, starts 8BPS" $?
[[ -n "$pc" && "$(head -c 2 "$pc")" == "PK" ]]; verdict "4 .procreate saved, starts PK" $?

# 5 round trip
log "5. round trip"
if [[ -n "$pc" ]]; then
  adb push "$pc" /sdcard/Download/eskiz-roundtrip.procreate > /dev/null
  clear_logs; hook open_document
  if pick_in_downloads eskiz-roundtrip.procreate && wait_log "saf: open ok" 30; then
    rm -f "$out/pulled/EskizTest/"*.png; adb shell rm -f /sdcard/Documents/EskizTest/*.png
    clear_logs; hook export:png:save; wait_log "saf: committed" 40
    rm -rf "$out/pulled5"; mkdir -p "$out/pulled5"; adb pull /sdcard/Documents/EskizTest "$out/pulled5/" > /dev/null 2>&1
    png5="$(ls "$out"/pulled5/EskizTest/*.png 2> /dev/null | head -1)"
    [[ -n "$png5" && -n "$png3" ]] && python3 -I "$here/tools/png_same.py" "$png5" "$png3"; verdict "5 round trip equals step 3 pixel for pixel" $?
  else verdict "5 round trip (open)" 1; fi
else verdict "5 round trip (no .procreate from step 4)" 1; fi
shot 05-roundtrip

# 6 grant survives a restart
log "6. grant survives restart"
launch; hook open_document; sleep 1; key KEYCODE_BACK   # make sure a document is open: reuse the last one
adb shell rm -f /sdcard/Documents/EskizTest/*.png
clear_logs; hook export:png:save
ok=1; wait_log "saf: tree reused" 30 && wait_log "saf: committed" 40 && ok=0; verdict "6 grant survives a restart (tree reused, no picker)" $ok
clear_logs; hook export:png:save; wait_log "saf: committed" 40
adb shell ls /sdcard/Documents/EskizTest | tr -d '\r' | grep -Eq '\(1\)\.png'; verdict "6 second save is '<title> (1).png'" $?
shot 06-restart

# 7 grant gone
log "7. grant gone"
adb shell rm -rf /sdcard/Documents/EskizTest
clear_logs; hook export:png:save; sleep 4
find_node --text-contains "save" > /dev/null; verdict "7 create-document picker opens as the fallback" $?
shot 07-fallback
key KEYCODE_BACK; sleep 1

# 8 imports
log "8. imports"
clear_logs; hook import_image
if pick_in_downloads fixture.png; then wait_log "saf: picked" 30; verdict "8 import image (picked)" $?; else verdict "8 import image (picker node)" 1; fi
clear_logs; hook import_brush
if pick_in_downloads fixture.brushset; then wait_log "saf: picked" 30; verdict "8 import brush set (picked)" $?; else verdict "8 import brush set (picker node)" 1; fi
shot 08-imports
key KEYCODE_BACK; sleep 1

# 9 share
log "9. share"
clear_logs; hook export:png:share
if tap_node 30 --text "Eskiz Share Test"; then
  wait_log "sharetest: received" 30 && has_log "sharetest: .*type=image/png"; verdict "9 share: test app received image/png" $?
  sha9="$(logs | grep -Eo 'sha256=[0-9a-f]{64}' | tail -1 | cut -d= -f2)"
  log "shared sha256 $sha9"
  [[ -n "$sha9" ]]; verdict "9 share: test app logged a SHA-256" $?
else verdict "9 share (chooser node)" 1; fi
shot 09-share
key KEYCODE_BACK; sleep 1

# 10 gallery share
log "10. gallery share"
launch; clear_logs; hook gallery_share_first
if tap_node 30 --text "Eskiz Share Test"; then wait_log "sharetest: .*type=image/png" 30; verdict "10 gallery share: test app received a PNG" $?
else verdict "10 gallery share (chooser node)" 1; fi
shot 10-gallery-share
key KEYCODE_BACK; sleep 1

# 11 copy out
log "11. copy out"
launch; clear_logs; hook new_canvas; sleep 3; hook copy_all; wait_log "clip: put" 20
adb shell am start -n app.eskiz.sharetest/.ClipReadActivity > /dev/null 2>&1
wait_log "sharetest: .*type=image/png" 20; verdict "11 copy out: test app reads image/png from the clipboard" $?
shot 11-copy-out

# 12 paste in
log "12. paste in"
clear_logs
adb shell am start -n app.eskiz.sharetest/.ClipWriteActivity > /dev/null 2>&1
wait_log "sharetest: clip written" 20; sleep 1
adb shell am start -n app.eskiz/.EskizActivity > /dev/null 2>&1; sleep 4
hook paste
wait_log "clip: foreign 64x64" 30; verdict "12 paste in: foreign 64x64 image read" $?
shot 12-paste

# 13 Back
log "13. Back"
launch; clear_logs; hook new_canvas; sleep 3
tap_xy "$((W / 2))" "$((H * 5 / 100))" # top bar area; the popover is opened by the hook below if the tap misses
hook back_state; sleep 1
key KEYCODE_BACK; wait_log "back: (popover|gallery)" 10; verdict "13 Back closes a popover or leaves the canvas" $?
key KEYCODE_BACK; key KEYCODE_BACK; sleep 2
has_log "back: gallery"; verdict "13 Back from the canvas shows the gallery" $?
cur="$(adb shell dumpsys activity activities 2> /dev/null | tr -d '\r' | grep -E 'ResumedActivity|topResumedActivity' | head -1)"
if [[ "$cur" != *app.eskiz/* ]] && alive; then verdict "13 final Back leaves Eskiz, process alive" 0; else log "resumed: $cur"; verdict "13 final Back leaves Eskiz, process alive" 1; fi
launch; shot 13-after-relaunch

# 14 picker cancel
log "14. picker cancel"
clear_logs; hook open_document; sleep 4; key KEYCODE_BACK
wait_log "saf: cancelled" 20; verdict "14 picker cancel logged" $?
! has_log "interrupted"; verdict "14 no error notice after cancel" $?

# 15 death during pick
log "15. death during pick"
clear_logs; hook open_document; sleep 4
adb shell am kill app.eskiz > /dev/null 2>&1; adb shell am force-stop app.eskiz > /dev/null 2>&1 && true
adb shell am start -n app.eskiz/.EskizActivity > /dev/null 2>&1; sleep 4
# The picker is a separate app: bring it back by repeating the request, then pick after the restart.
log "15 note: the process was killed while a picker was open; the pick result of the old request is simulated by relaunching"
alive; verdict "15 app restarts after death during a pick" $?
[[ -z "$(adb logcat -b crash -d 2> /dev/null | tr -d '\r' | grep -E 'FATAL|app.eskiz')" ]]; verdict "15 crash log empty" $?
shot 15-after-death

# 16 big file (optional)
if [[ -n "$big" && -f "$big" ]]; then
  log "16. big file"
  adb push "$big" /sdcard/Download/eskiz-big.procreate > /dev/null
  launch; clear_logs; hook open_document
  if pick_in_downloads eskiz-big.procreate; then
    wait_log "saf: copied in" 180 && wait_log "saf: open ok" 180; verdict "16 big file opens" $?
    logs | grep "saf: copied in" | tee -a "$out/steps.txt"
    ! logs | grep -q "ANR in app.eskiz"; verdict "16 no ANR" $?
  else verdict "16 big file (picker node)" 1; fi
else log "16. big file: skipped (no --big-file)"; fi

adb logcat -d > "$out/logcat.txt" 2>&1
echo >> "$out/summary.md"; echo "Result: $([[ $fail == 0 ]] && echo PASS || echo FAIL)" >> "$out/summary.md"
log "done: $([[ $fail == 0 ]] && echo PASS || echo FAIL) (see $out/summary.md)"
exit "$fail"
