#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p dist
target/debug/examples/instrumented > dist/smoke-macos-target.log 2>&1 &
target_pid=$!
trap 'kill "$target_pid" 2>/dev/null || true' EXIT
'dist/Console UI.app/Contents/MacOS/ConsoleUI' --smoke | tee dist/smoke-macos.log
grep -q NATIVE_MACOS_SMOKE_OK dist/smoke-macos.log
