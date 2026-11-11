#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
profile="${1:-dev}"
case "$profile" in
  dev) profile_root=target/debug; cargo_flags=(--profile dev); swift_flags=(-Onone -g) ;;
  release) profile_root=target/release; cargo_flags=(--profile release); swift_flags=(-O) ;;
  *) echo 'Usage: build-macos.sh [dev|release]' >&2; exit 2 ;;
esac
export MACOSX_DEPLOYMENT_TARGET=14.0
cargo build --locked "${cargo_flags[@]}" --lib --bin console-ui
app_root="$project_root/dist/Console UI.app"
rm -rf -- "$app_root"
mkdir -p "$app_root/Contents/MacOS" "$app_root/Contents/Frameworks" "$app_root/Contents/Resources"
cp platforms/macos/Info.plist "$app_root/Contents/Info.plist"
cp "$profile_root/console-ui" "$app_root/Contents/MacOS/console-ui"
cp "$profile_root/libconsole_ui.dylib" "$app_root/Contents/Frameworks/"
install_name_tool -id '@rpath/libconsole_ui.dylib' "$app_root/Contents/Frameworks/libconsole_ui.dylib"
xcrun swiftc -swift-version 5 "${swift_flags[@]}" -parse-as-library \
  -import-objc-header platforms/macos/include/console_ui.h \
  -framework SwiftUI -framework AppKit -framework Charts \
  -L "$profile_root" -lconsole_ui \
  -Xlinker -rpath -Xlinker '@executable_path/../Frameworks' \
  platforms/macos/Sources/ConsoleUI.swift -o "$app_root/Contents/MacOS/ConsoleUI"
linked_library="$(otool -L "$app_root/Contents/MacOS/ConsoleUI" | awk '/libconsole_ui[.]dylib/{print $1}')"
install_name_tool -change "$linked_library" '@rpath/libconsole_ui.dylib' "$app_root/Contents/MacOS/ConsoleUI"
cp LICENSE THIRD_PARTY.md "$app_root/Contents/Resources/"
codesign --force --deep --sign - "$app_root"
