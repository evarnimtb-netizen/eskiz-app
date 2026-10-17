#!/usr/bin/env bash
# Host test: the fixed debug keystore opens with the documented debug-only password and
# app/build.gradle.kts signs `debug` (and so the debug-signed release fallback) with it.
set -eu
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
keytool -list -keystore "$here/easel-debug.keystore" -storepass easel-debug -alias easeldebug > /dev/null
grep -q 'rootProject.file("easel-debug.keystore")' "$here/app/build.gradle.kts"
grep -q 'getByName("debug")' "$here/app/build.gradle.kts"
grep -q 'never for Play' -i "$here/DEBUG_KEYSTORE.md"
echo "debug signing: ok"
