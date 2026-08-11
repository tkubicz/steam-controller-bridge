#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
APP_DIR="$PROJECT_DIR/dist/Steam Controller Bridge.app"
CONTENTS_DIR="$APP_DIR/Contents"

cd "$PROJECT_DIR"

# Release Please maintains version.txt; the bundle takes its version from there
# rather than carrying a second copy that silently goes stale. Refuse to build
# something mislabelled instead of shipping the wrong version.
VERSION=$(python3 "$PROJECT_DIR/tools/check-workspace-versions.py" --print-version)

cargo build --release -p sc-bridge-menu

rm -rf "$APP_DIR"
mkdir -p "$CONTENTS_DIR/MacOS" "$CONTENTS_DIR/Resources"
cp "$PROJECT_DIR/target/release/sc-bridge-menu" "$CONTENTS_DIR/MacOS/sc-bridge-menu"
cp "$PROJECT_DIR/packaging/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
# Stamp the copy, so the checked-in template holds no version to keep in sync.
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" \
  "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $VERSION" \
  "$CONTENTS_DIR/Info.plist"
cp "$PROJECT_DIR/packaging/macos/MenuBarTemplate.svg" \
  "$CONTENTS_DIR/Resources/MenuBarTemplate.svg"
# Referenced by CFBundleIconFile. Regenerate with tools/make-app-icon.py.
cp "$PROJECT_DIR/packaging/macos/AppIcon.icns" \
  "$CONTENTS_DIR/Resources/AppIcon.icns"

/usr/bin/codesign --force --deep --sign - "$APP_DIR"
/usr/bin/codesign --verify --deep --strict "$APP_DIR"

echo "$APP_DIR"
