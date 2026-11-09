#!/bin/zsh
# Draws the app icon (scripts/icons/icon.swift) and writes it where the apps take it:
#   assets/icons/YourHarness.icns, YourHarnessNext.icns   the Mac app (scripts/bundle.sh)
#   mobile/ios/YourHarness/Assets.xcassets/AppIcon, AppIconNext   the phone app
#   assets/icons/stable.png, next.png   1024 px, for docs and the web
# Stable goes on releases (RELEASE=1, the App Store build); Next on development builds,
# TestFlight and the Next train. Run it after changing the drawing, and commit what it writes.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
swiftc -O "$ROOT/scripts/icons/icon.swift" -o "$WORK/icon"
"$WORK/icon" "$WORK"
mkdir -p "$ROOT/assets/icons"
for variant in stable next; do
  name=$([[ $variant == stable ]] && echo YourHarness || echo YourHarnessNext)
  set="$WORK/$name.iconset"
  mkdir -p "$set"
  for size in 16 32 128 256 512; do
    cp "$WORK/$variant-mac-$size.png" "$set/icon_${size}x${size}.png"
    cp "$WORK/$variant-mac-$((size * 2)).png" "$set/icon_${size}x${size}@2x.png"
  done
  iconutil -c icns "$set" -o "$ROOT/assets/icons/$name.icns"
  cp "$WORK/$variant-mac-1024.png" "$ROOT/assets/icons/$variant.png"
  catalog=$([[ $variant == stable ]] && echo AppIcon || echo AppIconNext)
  dir="$ROOT/mobile/ios/YourHarness/Assets.xcassets/$catalog.appiconset"
  mkdir -p "$dir"
  cp "$WORK/$variant-ios.png" "$dir/icon.png"
  cat > "$dir/Contents.json" <<JSON
{
  "images" : [
    { "filename" : "icon.png", "idiom" : "universal", "platform" : "ios", "size" : "1024x1024" }
  ],
  "info" : { "author" : "xcode", "version" : 1 }
}
JSON
done
cat > "$ROOT/mobile/ios/YourHarness/Assets.xcassets/Contents.json" <<JSON
{
  "info" : { "author" : "xcode", "version" : 1 }
}
JSON
echo "icons written: assets/icons, mobile/ios/YourHarness/Assets.xcassets"
