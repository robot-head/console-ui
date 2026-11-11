#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
cargo build --locked --release -p console-ui-gtk
package_root="$project_root/dist/console-ui-linux-$(uname -m)"
mkdir -p "$package_root/bin" "$package_root/share/applications"
cp target/release/console-ui-gtk "$package_root/bin/"
cp platforms/linux/org.consoleui.Diagnostics.desktop "$package_root/share/applications/"
cp README.md LICENSE THIRD_PARTY.md "$package_root/"
tar -C "$package_root" -czf "$package_root.tar.gz" .
(cd dist && sha256sum "console-ui-linux-$(uname -m).tar.gz" > "console-ui-linux-$(uname -m).tar.gz.sha256")
