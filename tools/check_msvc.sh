#!/usr/bin/env bash
# Compile-check Eskiz for x86_64-pc-windows-msvc from macOS or Linux with cargo-xwin.
# Nothing runs: the output is a Windows exe that only has to compile and link.
#
#   eskiz/tools/check_msvc.sh            # cargo xwin check + clippy (-D warnings), whole workspace
#   eskiz/tools/check_msvc.sh --build    # also a release build of eskiz-app and its size
#
# One-time set-up (needs the user; an automated run must not accept the license):
#   brew install llvm                      # clang-cl, lld, llvm-rc
#   rustup target add x86_64-pc-windows-msvc
#   cargo install cargo-xwin --version 0.23.1 --locked
#   XWIN_CACHE_DIR must name a folder OUTSIDE the repo with room for the Microsoft CRT and SDK
#   (about 0.6 GB for x86_64 only). It defaults to ~/Library/Caches/xwin (macOS) or ~/.cache/xwin.
#   The first run downloads the CRT and SDK, which means accepting Microsoft's license
#   (https://go.microsoft.com/fwlink/?LinkId=2086102). After you have read it, set
#   XWIN_ACCEPT_LICENSE=1 for that first run (this script never sets it for you).
#   Do not commit or redistribute the downloaded files.
#
# Rust comes from rustup's toolchain on purpose: Homebrew's rust has no Windows target libraries.
# Only one cargo job should run at a time on a small Mac.
set -euo pipefail

ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$ROOT/eskiz"
TARGET=x86_64-pc-windows-msvc

case "$(uname -s)" in
  Darwin) DEFAULT_CACHE="$HOME/Library/Caches/xwin" ;;
  *) DEFAULT_CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/xwin" ;;
esac
export XWIN_CACHE_DIR="${XWIN_CACHE_DIR:-$DEFAULT_CACHE}"
export XWIN_ARCH="${XWIN_ARCH:-x86_64}"   # skip the aarch64 SDK: saves download time and disk
export CARGO_INCREMENTAL=0
# Its own target dir: the host build scripts and proc macros are compiled by rustup's rustc here,
# and Homebrew's rustc (the normal macOS build) refuses to reuse them (E0514 incompatible rustc).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/eskiz/target/xwin}"

# clang-cl, lld-link and llvm-rc (the resource compiler for the icon): Homebrew's llvm is keg-only.
for d in /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin; do
  [ -d "$d" ] && PATH="$d:$PATH"
done
export PATH="$HOME/.cargo/bin:$PATH"

need() { command -v "$1" >/dev/null 2>&1 || { echo "check_msvc: missing '$1'. $2" >&2; exit 3; }; }
need rustup "Install rustup (https://rustup.rs)."
need clang-cl "Run: brew install llvm"
need cargo-xwin "Run: cargo install cargo-xwin --version 0.23.1 --locked"
SYSROOT="$(rustup run stable rustc --print sysroot)"
[ -d "$SYSROOT/lib/rustlib/$TARGET" ] || { echo "check_msvc: run: rustup target add $TARGET" >&2; exit 3; }
if [ ! -d "$XWIN_CACHE_DIR/xwin/crt" ] && [ "${XWIN_ACCEPT_LICENSE:-}" != "1" ]; then
  echo "check_msvc: no CRT/SDK cache in $XWIN_CACHE_DIR yet. Read Microsoft's license" >&2
  echo "  (https://go.microsoft.com/fwlink/?LinkId=2086102), then rerun with XWIN_ACCEPT_LICENSE=1." >&2
  exit 3
fi

# rustup's cargo and rustc first, so cargo-xwin's inner `cargo clippy` finds the Windows std.
export PATH="$SYSROOT/bin:$PATH" CARGO="$SYSROOT/bin/cargo" RUSTC="$SYSROOT/bin/rustc"

echo "== cargo xwin check -p eskiz-app --target $TARGET"
cargo xwin check -p eskiz-app --target "$TARGET" --locked
echo "== cargo xwin clippy --workspace --all-targets --target $TARGET -- -D warnings"
cargo xwin clippy --workspace --all-targets --target "$TARGET" --locked -- -D warnings

if [ "${1:-}" = "--build" ]; then
  echo "== cargo xwin build --release -p eskiz-app --target $TARGET"
  cargo xwin build --release -p eskiz-app --target "$TARGET" --locked
  ls -l "$CARGO_TARGET_DIR/$TARGET/release/eskiz.exe"
fi
echo "check_msvc: ok"
