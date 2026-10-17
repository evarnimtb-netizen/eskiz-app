#!/bin/bash
# Optional interop check for an exported .usdz. Usage: tools/usdz_check.sh file.usdz
# Runs usdchecker --arkit and usdcat if the OpenUSD tools are installed; says "skipped" otherwise.
f=${1:?usage: usdz_check.sh file.usdz}
if command -v usdchecker >/dev/null; then usdchecker --arkit "$f"; else echo "usdchecker: skipped (not installed)"; fi
if command -v usdcat >/dev/null; then usdcat --flatten "$f" >/dev/null && echo "usdcat: ok"; else echo "usdcat: skipped (not installed)"; fi
if command -v qlmanage >/dev/null; then echo "Quick Look: run  qlmanage -p $f"; fi
