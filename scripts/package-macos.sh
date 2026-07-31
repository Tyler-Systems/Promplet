#!/bin/sh
# Builds target/release/Promplet.app: a minimal, ad-hoc-signed bundle.
#
# The bundle matters on macOS beyond Finder niceties: it gives Promplet a
# stable identity for the Accessibility permission, and LSUIElement keeps it
# out of the Dock from the first frame.
set -eu
cd "$(dirname "$0")/.."

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="target/release/Promplet.app"

cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/promplet "$APP/Contents/MacOS/promplet"

ICONSET="$(mktemp -d)/promplet.iconset"
mkdir -p "$ICONSET"
for SIZE in 16 32 128 256; do
    DOUBLE=$((SIZE * 2))
    sips -z "$SIZE" "$SIZE" assets/promplet-icon.png \
        --out "$ICONSET/icon_${SIZE}x${SIZE}.png" >/dev/null
    sips -z "$DOUBLE" "$DOUBLE" assets/promplet-icon.png \
        --out "$ICONSET/icon_${SIZE}x${SIZE}@2x.png" >/dev/null
done
sips -z 512 512 assets/promplet-icon.png \
    --out "$ICONSET/icon_512x512.png" >/dev/null
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/promplet.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>com.tylersystems.promplet</string>
    <key>CFBundleName</key>
    <string>Promplet</string>
    <key>CFBundleDisplayName</key>
    <string>Promplet</string>
    <key>CFBundleExecutable</key>
    <string>promplet</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleIconFile</key>
    <string>promplet</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

# Ad-hoc signing pins the Accessibility permission to this exact binary, so
# every rebuild needs the permission granted again. Set CODESIGN_IDENTITY to
# a real signing identity to keep the permission across rebuilds.
codesign --force --sign "${CODESIGN_IDENTITY:--}" "$APP"
echo "Built $APP"
