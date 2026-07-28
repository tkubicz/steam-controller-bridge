#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
APP_DIR="$PROJECT_DIR/dist/Steam Controller Bridge.app"
CONTENTS_DIR="$APP_DIR/Contents"

cd "$PROJECT_DIR"
cargo build --release -p sc-bridge-menu

rm -rf "$APP_DIR"
mkdir -p "$CONTENTS_DIR/MacOS" "$CONTENTS_DIR/Resources"
cp "$PROJECT_DIR/target/release/sc-bridge-menu" "$CONTENTS_DIR/MacOS/sc-bridge-menu"
cp "$PROJECT_DIR/packaging/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
cp "$PROJECT_DIR/packaging/macos/MenuBarTemplate.svg" \
  "$CONTENTS_DIR/Resources/MenuBarTemplate.svg"

/usr/bin/codesign --force --deep --sign - "$APP_DIR"
/usr/bin/codesign --verify --deep --strict "$APP_DIR"

echo "$APP_DIR"
