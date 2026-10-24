#!/bin/bash
# Optional interop check for an exported .usdz. Usage: tools/usdz_check.sh file.usdz
# Runs usdchecker --arkit and usdcat if the OpenUSD tools are installed; says "skipped" otherwise.
# Exits non-zero when a tool that ran reported a problem.
f=${1:?usage: usdz_check.sh file.usdz}
status=0
if command -v usdchecker >/dev/null; then
    usdchecker --arkit "$f" || { echo "usdchecker: FAILED"; status=1; }
else
    echo "usdchecker: skipped (not installed)"
fi
if command -v usdcat >/dev/null; then
    if usdcat --flatten "$f" >/dev/null; then echo "usdcat: ok"; else echo "usdcat: FAILED"; status=1; fi
else
    echo "usdcat: skipped (not installed)"
fi
if command -v qlmanage >/dev/null; then echo "Quick Look: run  qlmanage -p $f"; fi
exit $status
