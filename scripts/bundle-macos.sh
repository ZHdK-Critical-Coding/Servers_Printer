#!/bin/bash
# Builds "Printer Server.app" in dist/. With --universal it runs on Apple Silicon and Intel Macs
# (needs: rustup target add aarch64-apple-darwin x86_64-apple-darwin).
set -e
cd "$(dirname "$0")/.."

APP="dist/Printer Server.app"

# Keep local paths (and with them the user name) out of the binary. The last matching rule wins.
# Encoded form (flags separated by 0x1f), because the paths may contain spaces.
SEP=$'\x1f'
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$HOME=/home$SEP--remap-path-prefix=$HOME/.cargo=/cargo\
$SEP--remap-path-prefix=$HOME/.rustup=/rustup$SEP--remap-path-prefix=$PWD=/printer-server"
export CFLAGS="-ffile-prefix-map=$HOME=/home"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

if [ "$1" == "--universal" ]; then
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  BINARY=target/printer-server-universal
  lipo -create -output "$BINARY" \
    target/aarch64-apple-darwin/release/printer-server \
    target/x86_64-apple-darwin/release/printer-server
else
  cargo build --release
  BINARY=target/release/printer-server
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BINARY" "$APP/Contents/MacOS/printer-server"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Printer Server</string>
  <key>CFBundleDisplayName</key><string>Printer Server</string>
  <key>CFBundleIdentifier</key><string>ch.zhdk.criticalcoding.printerserver</string>
  <key>CFBundleExecutable</key><string>printer-server</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
# Ad-hoc signature, so macOS starts it on this computer
codesign --force --sign - "$APP"
echo "Built $APP"
