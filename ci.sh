#!/usr/bin/env bash
set -euo pipefail

echo "=== amsal ci ==="

echo "--- build ---"
cargo build --verbose

echo "--- test ---"
cargo test --verbose

echo "--- release build ---"
cargo build --release

echo "=== all green ==="
