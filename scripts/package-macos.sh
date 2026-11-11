#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
export MACOSX_DEPLOYMENT_TARGET=14.0
cargo build --locked --release --lib --bin console-ui
app_root="$project_root/dist/Console UI.app"
rm -rf -- "$app_root"
mkdir -p "$app_root/Contents/MacOS" "$app_root/Contents/Frameworks" "$app_root/Contents/Resources"
cp platforms/macos/Info.plist "$app_root/Contents/Info.plist"
cp target/release/console-ui "$app_root/Contents/MacOS/console-ui"
cp target/release/libconsole_ui.dylib "$app_root/Contents/Frameworks/"
install_name_tool -id '@rpath/libconsole_ui.dylib' "$app_root/Contents/Frameworks/libconsole_ui.dylib"
xcrun swiftc -swift-version 5 -O -parse-as-library \
  -import-objc-header platforms/macos/include/console_ui.h \
  -framework SwiftUI -framework AppKit -framework Charts \
  -L target/release -lconsole_ui \
  -Xlinker -rpath -Xlinker '@executable_path/../Frameworks' \
  platforms/macos/Sources/ConsoleUI.swift -o "$app_root/Contents/MacOS/ConsoleUI"
linked_library="$(otool -L "$app_root/Contents/MacOS/ConsoleUI" | awk '/libconsole_ui[.]dylib/{print $1}')"
install_name_tool -change "$linked_library" '@rpath/libconsole_ui.dylib' "$app_root/Contents/MacOS/ConsoleUI"
cp LICENSE THIRD_PARTY.md "$app_root/Contents/Resources/"
codesign --force --deep --sign - "$app_root"
ditto -c -k --sequesterRsrc --keepParent "$app_root" "$project_root/dist/console-ui-macos-$(uname -m).zip"
(cd dist && shasum -a 256 "console-ui-macos-$(uname -m).zip" > "console-ui-macos-$(uname -m).zip.sha256")
