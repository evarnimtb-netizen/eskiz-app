#!/usr/bin/env bash
# Regenerates THIRD_PARTY_NOTICES.md (default desktop build, union of the Linux, Windows and macOS
# targets listed in about.toml) with cargo-about, and the Android variant
# THIRD_PARTY_NOTICES-android.md (aarch64-linux-android, --no-default-features --features android).
#
#   tools/gen_notices.sh            both files
#   tools/gen_notices.sh desktop    only THIRD_PARTY_NOTICES.md
#   tools/gen_notices.sh android    only THIRD_PARTY_NOTICES-android.md
#
# Needs cargo-about (cargo install --locked cargo-about). Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."
what="${1:-all}"
manifest=crates/eskiz-app/Cargo.toml

if [ "$what" = all ] || [ "$what" = desktop ]; then
    cargo about generate --locked --manifest-path "$manifest" about.hbs -o THIRD_PARTY_NOTICES.md
    echo "wrote THIRD_PARTY_NOTICES.md"
fi
if [ "$what" = all ] || [ "$what" = android ]; then
    # The android build has its own target and feature set, so it gets its own config.
    cfg="$(mktemp)"
    trap 'rm -f "$cfg"' EXIT
    sed '/^targets = \[/,/^\]/d' about.toml > "$cfg"
    printf 'targets = ["aarch64-linux-android"]\n' >> "$cfg"
    cargo about generate --locked --config "$cfg" --manifest-path "$manifest" \
        --no-default-features --features android about.hbs -o THIRD_PARTY_NOTICES-android.md
    echo "wrote THIRD_PARTY_NOTICES-android.md"
fi
