#!/usr/bin/env bash
set -euo pipefail

echo "=== amsal-ffi release build ==="
cargo build --release -p amsal-ffi

LIB=$(find target/release -maxdepth 1 -name "libamsal_ffi.*" -type f | head -1)
if [ -n "$LIB" ]; then
    echo "built: $LIB"
    ls -lh "$LIB"
else
    echo "error: library not found"
    exit 1
fi
