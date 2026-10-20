#!/bin/bash
# Builds, signs and notarizes "Printer Server.app", so it starts on other Macs without warnings.
# Results: dist/Printer Server.app, dist/Printer-Server-<version>.zip and .dmg (all stapled).
#
# Needs once:
#   1. A "Developer ID Application" certificate in the keychain (Xcode → Settings → Accounts).
#   2. Notary credentials, stored in the keychain with an app-specific password
#      (appleid.apple.com → Sign-In and Security → App-Specific Passwords):
#        xcrun notarytool store-credentials printer-server \
#          --apple-id you@example.com --team-id ABCDE12345 --password xxxx-xxxx-xxxx-xxxx
#
# Options:
#   --native      only for the architecture of this Mac (default: Apple Silicon and Intel)
#   --sign-only   sign and check the signature, but do not send anything to Apple
# Environment:
#   NOTARY_PROFILE  keychain profile name (default: printer-server)
#   IDENTITY        signing identity (default: the first "Developer ID Application" found)
set -e
cd "$(dirname "$0")/.."

NATIVE=false
SIGN_ONLY=false
for arg in "$@"; do
  case $arg in
    --native) NATIVE=true ;;
    --sign-only) SIGN_ONLY=true ;;
    *) echo "Unknown option $arg" >&2; exit 1 ;;
  esac
done

PROFILE=${NOTARY_PROFILE:-printer-server}
IDENTITY=${IDENTITY:-$(security find-identity -v -p codesigning | grep -m1 -o '"Developer ID Application: [^"]*"' | tr -d '"')}
if [ -z "$IDENTITY" ]; then
  echo "No 'Developer ID Application' certificate found in the keychain." >&2
  exit 1
fi
if ! $SIGN_ONLY && ! xcrun notarytool history --keychain-profile "$PROFILE" > /dev/null 2>&1; then
  echo "No notary credentials named '$PROFILE' in the keychain. Create them first:" >&2
  echo "  xcrun notarytool store-credentials $PROFILE --apple-id <apple id> --team-id <team id> --password <app-specific password>" >&2
  exit 1
fi

APP="dist/Printer Server.app"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
ZIP="dist/Printer-Server-$VERSION.zip"
DMG="dist/Printer-Server-$VERSION.dmg"

echo "== Build"
if $NATIVE; then
  ./scripts/bundle-macos.sh
else
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  ./scripts/bundle-macos.sh --universal
fi

echo "== Sign with $IDENTITY"
# Hardened runtime and a secure timestamp are required for notarization
codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"

if $SIGN_ONLY; then
  echo "Signed, not notarized (--sign-only): $APP"
  exit 0
fi

echo "== Notarize the app"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$APP"
# Zip again, now with the stapled ticket inside
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

echo "== Disk image"
STAGING=$(mktemp -d)
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
rm -f "$DMG"
hdiutil create -volname "Printer Server" -srcfolder "$STAGING" -ov -format UDZO "$DMG"
rm -rf "$STAGING"
codesign --force --timestamp --sign "$IDENTITY" "$DMG"
xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$DMG"

echo "== Check"
spctl --assess --type execute --verbose "$APP"
spctl --assess --type open --context context:primary-signature --verbose "$DMG"
echo "Done: $ZIP and $DMG"
