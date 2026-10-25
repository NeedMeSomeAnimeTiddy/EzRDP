#!/bin/sh
# Builds the Windows version on Linux and runs its tests under Wine, so Windows mistakes are
# caught before pushing. Only a check: the EzRDP.exe people download comes from GitHub Actions.
# Usage: server/check-windows.sh   (installs what it needs the first time; needs root/sudo)
set -e
cd "$(dirname "$0")"
SUDO=$(command -v sudo || true)

if ! command -v x86_64-w64-mingw32-gcc >/dev/null || ! command -v wine >/dev/null; then
    $SUDO apt-get update
    $SUDO apt-get install -y mingw-w64 wine64
fi
rustup target add x86_64-pc-windows-gnu >/dev/null

# Tests run under Wine and need MinGW's C++ library, which isn't built into the test program.
export WINEDEBUG=-all
export WINEPATH="$(ls -d /usr/lib/gcc/x86_64-w64-mingw32/*-posix | head -1);/usr/x86_64-w64-mingw32/lib"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine
cargo test --release --target x86_64-pc-windows-gnu
