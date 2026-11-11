#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p dist
app_binary="${2:-target/debug/console-ui-gtk}"
test -x "$app_binary"
target/debug/examples/instrumented > dist/smoke-linux-target.log 2>&1 &
target_pid=$!
compositor_pid=''
trap 'kill "$target_pid" ${compositor_pid:+"$compositor_pid"} 2>/dev/null || true' EXIT
if [ "${1:-x11}" = wayland ]; then
  runtime_dir="$(mktemp -d)"
  chmod 700 "$runtime_dir"
  export XDG_RUNTIME_DIR="$runtime_dir"
  weston --backend=headless-backend.so --renderer=pixman --shell=kiosk-shell.so --socket=console-ui-smoke --idle-time=0 --no-config > dist/smoke-linux-compositor.log 2>&1 &
  compositor_pid=$!
  for _ in $(seq 1 100); do [ -S "$runtime_dir/console-ui-smoke" ] && break; sleep 0.1; done
  GDK_BACKEND=wayland WAYLAND_DISPLAY=console-ui-smoke dbus-run-session -- timeout 60s "$app_binary" --smoke | tee dist/smoke-linux-wayland.log
  grep -q NATIVE_GTK_SMOKE_OK dist/smoke-linux-wayland.log
else
  GDK_BACKEND=x11 GSK_RENDERER=cairo xvfb-run -a dbus-run-session -- timeout 60s "$app_binary" --smoke | tee dist/smoke-linux-x11.log
  grep -q NATIVE_GTK_SMOKE_OK dist/smoke-linux-x11.log
fi
