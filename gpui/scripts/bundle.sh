#!/bin/zsh
# Builds build/YourHarnessGPUI.app (release, ad hoc signed) from gpui/.
set -euo pipefail
DIR="${0:A:h:h}"
ROOT="${DIR:h}"
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
export PATH="$HOME/.cargo/bin:$PATH"
"$DIR/scripts/vendor.sh"
(cd "$DIR" && cargo build --release)
APP="$ROOT/build/YourHarnessGPUI.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$DIR/target/release/yourharness-gpui" "$APP/Contents/MacOS/YourHarnessGPUI"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.mdovhopo.yourharness.gpui</string>
  <key>CFBundleName</key><string>YourHarness GPUI</string>
  <key>CFBundleExecutable</key><string>YourHarnessGPUI</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.1</string>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
PLIST
codesign --force --sign - "$APP" >/dev/null 2>&1
echo "$APP"
