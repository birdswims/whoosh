#!/bin/bash
# Build Whoosh.app and a drag-to-Applications disk image.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

HELPER_PLIST="$ROOT/macos/Whoosh/Resources/HelperInfo.plist"
APP_PLIST="$ROOT/macos/Whoosh/Resources/Info.plist"
STAGE="$ROOT/dist/stage"
LAYOUT="$ROOT/dist/layout"
APP="$STAGE/Whoosh.app"
ART="$ROOT/dist/art"
RW="$ROOT/dist/whoosh-rw.dmg"
OUT="$ROOT/dist/Whoosh.dmg"

echo "Building the Whoosh engine"
cargo rustc --release --bin whoosh -- \
  -C "link-arg=-Wl,-sectcreate,__TEXT,__info_plist,${HELPER_PLIST}"

echo "Building the SwiftUI app"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
# Swift libraries ship with macOS, so the app links them dynamically.
SWIFT_FLAGS=(-c release --package-path "$ROOT/macos/Whoosh")
swift build "${SWIFT_FLAGS[@]}"

BIN_DIR="$(swift build "${SWIFT_FLAGS[@]}" --show-bin-path)"
SWIFT_APP="$BIN_DIR/Whoosh"
RENDER="$BIN_DIR/RenderIcon"

echo "Drawing the icon and logo"
rm -rf "$ART"
mkdir -p "$ART"
"$RENDER" "$ART"
iconutil -c icns "$ART/AppIcon.iconset" -o "$ART/AppIcon.icns"
cp "$ART/logo.png" "$ROOT/macos/Whoosh/Resources/logo.png"

echo "Assembling Whoosh.app"
rm -rf "$STAGE"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$APP_PLIST" "$APP/Contents/Info.plist"
cp "$ART/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
cp "$ART/logo.png" "$APP/Contents/Resources/logo.png"
cp "$SWIFT_APP" "$APP/Contents/MacOS/Whoosh"
# The volume is often case-insensitive, so the engine cannot be named "whoosh".
cp "$ROOT/target/release/whoosh" "$APP/Contents/MacOS/whoosh-core"
chmod +x "$APP/Contents/MacOS/Whoosh" "$APP/Contents/MacOS/whoosh-core"
printf 'APPL????' > "$APP/Contents/PkgInfo"

SHARE="$APP/Contents/PlugIns/WhooshShare.appex"
mkdir -p "$SHARE/Contents/MacOS" "$SHARE/Contents/Resources"
cp "$ROOT/macos/Whoosh/Share/Info.plist" "$SHARE/Contents/Info.plist"
cp "$BIN_DIR/WhooshShare" "$SHARE/Contents/MacOS/WhooshShare"
cp "$ART/AppIcon.icns" "$SHARE/Contents/Resources/AppIcon.icns"
chmod +x "$SHARE/Contents/MacOS/WhooshShare"

codesign --force --sign - \
  --entitlements "$ROOT/macos/Whoosh/Share/WhooshShare.entitlements" \
  --identifier com.whoosh.macos.share \
  "$SHARE"
codesign --force --sign - --identifier com.whoosh.macos.core "$APP/Contents/MacOS/whoosh-core"
codesign --force --sign - --identifier com.whoosh.macos "$APP"
codesign --verify --strict "$SHARE"
codesign --verify --strict "$APP"

echo "Packing the disk image"
rm -rf "$LAYOUT"
mkdir -p "$LAYOUT/.background"
cp -R "$APP" "$LAYOUT/Whoosh.app"
cp "$ART/background.png" "$LAYOUT/.background/background.png"
ln -s /Applications "$LAYOUT/Applications"
rm -f "$RW" "$OUT"

hdiutil create -volname Whoosh -srcfolder "$LAYOUT" -format UDRW -ov "$RW" >/dev/null

MOUNT=""
cleanup() {
  if [[ -n "${MOUNT}" && -d "${MOUNT}" ]]; then
    hdiutil detach "$MOUNT" >/dev/null 2>&1 || hdiutil detach -force "$MOUNT" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

ATTACH="$(hdiutil attach -readwrite -noverify -noautoopen "$RW")"
MOUNT="$(printf '%s\n' "$ATTACH" | awk '/\/Volumes\// { print $NF; exit }')"
if [[ -z "${MOUNT}" || ! -d "${MOUNT}" ]]; then
  echo "Could not mount the disk image" >&2
  printf '%s\n' "$ATTACH" >&2
  exit 1
fi

# Finder layout is presentation. A failure still leaves a usable image.
if ! osascript <<EOF
tell application "Finder"
  tell disk "Whoosh"
    open
    set theWindow to container window
    set current view of theWindow to icon view
    set toolbar visible of theWindow to false
    set statusbar visible of theWindow to false
    set bounds of theWindow to {80, 80, 800, 540}
    set theOptions to icon view options of theWindow
    set arrangement of theOptions to not arranged
    set icon size of theOptions to 88
    set text size of theOptions to 13
    set background picture of theOptions to file ".background:background.png"
    set position of item "Whoosh.app" of theWindow to {190, 268}
    set position of item "Applications" of theWindow to {530, 268}
    update without registering applications
    delay 2
    close
  end tell
end tell
EOF
then
  echo "Finder layout was skipped; the disk image still contains Whoosh.app"
fi

sync
cleanup
trap - EXIT
MOUNT=""

hdiutil convert "$RW" -format UDZO -imagekey zlib-level=9 -ov -o "$OUT" >/dev/null
rm -f "$RW"
echo "Wrote $OUT"
