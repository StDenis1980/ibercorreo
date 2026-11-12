#!/usr/bin/env bash
# Build and package Tinta Correo for macOS:
#
#   $DIST/tinta-correo-<version>-macos-<arch>.dmg        Tinta Correo.app on a drag-to-Applications DMG
#   $DIST/ibercorreo-cli-<version>-macos-<arch>.zip      the headless CLI (and MCP server)
#
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build]
#
# Run it on a Mac with the Xcode command line tools (codesign, hdiutil, lipo, plutil) and rustup
# (`rustup target add aarch64-apple-darwin x86_64-apple-darwin` for --arch universal).
#
# Gmail and Microsoft 365 sign-in need the OAuth client ids compiled in (docs/oauth.md): set
# IBERCORREO_GOOGLE_CLIENT_ID, IBERCORREO_GOOGLE_CLIENT_SECRET and IBERCORREO_MICROSOFT_CLIENT_ID
# before building. Without them the build works with password (IMAP/SMTP) accounts.
#
# Signing (env):
#   MACOS_SIGN_IDENTITY   codesign identity (name or SHA-1). Default "-" = ad-hoc: the app runs on
#                         the Mac that built it, and on other Macs only after the person allows it
#                         (System Settings > Privacy & Security > Open Anyway). No notarization here:
#                         Tinta Office has no Apple Developer ID yet.
#   MACOS_KEYCHAIN        keychain holding the identity (optional)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/packaging/macos"
APP_TITLE="Tinta Correo"
SLUG=tinta-correo
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
DIST="${DIST:-$ROOT/dist/release}"
mkdir -p "$DIST"
DIST="$(cd "$DIST" && pwd)"

ARCH=universal
SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="${2:-}"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,21p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  aarch64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac
[[ "$(uname -s)" = Darwin ]] || { echo "error: packaging/macos/package.sh runs on macOS only" >&2; exit 1; }

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
VERSION="${IBERCORREO_VERSION:-$(awk '
  /^\[/ { inpkg = ($0 == "[workspace.package]"); next }
  inpkg && $1 == "version" { v = $3; gsub(/"/, "", v); print v; exit }
' "$ROOT/Cargo.toml")}"
[[ -n "$VERSION" ]] || { echo "error: no [workspace.package] version in Cargo.toml" >&2; exit 1; }
SHORT_VERSION="${VERSION%%-*}"
BUILD_SHA="${IBERCORREO_BUILD_SHA:-$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)}"

# Keep in sync with LSMinimumSystemVersion in Info.plist.in.
export MACOSX_DEPLOYMENT_TARGET=11.0
IDENTITY="${MACOS_SIGN_IDENTITY:--}"
WORK="$CARGO_TARGET_DIR/macos-package"
APP="$WORK/$APP_TITLE.app"
DMG="$DIST/$SLUG-$VERSION-macos-$ARCH.dmg"
CLI_ZIP="$DIST/ibercorreo-cli-$VERSION-macos-$ARCH.zip"

echo "==> $APP_TITLE $VERSION for macOS ($ARCH), identity: $IDENTITY"
for v in IBERCORREO_GOOGLE_CLIENT_ID IBERCORREO_MICROSOFT_CLIENT_ID; do
  if [[ "$SKIP_BUILD" = 0 && -z "${!v:-}" ]]; then
    echo "warning: $v is not set; that sign-in will be missing from this build" >&2
  fi
done

# ---- build -------------------------------------------------------------------------------------
if [[ "$SKIP_BUILD" = 0 ]]; then
  args=()
  for t in "${TARGETS[@]}"; do args+=(--target "$t"); done
  (cd "$ROOT" && cargo build --release --locked -p ibercorreo -p ibercorreo-cli "${args[@]}")
fi

rm -rf "$WORK"
mkdir -p "$WORK/bin"
for bin in ibercorreo ibercorreo-cli; do
  inputs=()
  for t in "${TARGETS[@]}"; do inputs+=("$CARGO_TARGET_DIR/$t/release/$bin"); done
  lipo -create -output "$WORK/bin/$bin" "${inputs[@]}"
  lipo -info "$WORK/bin/$bin"
done

sign() {
  # Hardened runtime + secure timestamp for a real identity; ad-hoc can't be timestamped.
  local ts=(--timestamp)
  [[ "$IDENTITY" = "-" ]] && ts=(--timestamp=none)
  local kc=()
  [[ -n "${MACOS_KEYCHAIN:-}" ]] && kc=(--keychain "$MACOS_KEYCHAIN")
  codesign --force --sign "$IDENTITY" ${kc[@]+"${kc[@]}"} "${ts[@]}" "$@"
}

# ---- the app bundle ----------------------------------------------------------------------------
echo "==> assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# Executable and icon carry the display name (CFBundleExecutable / CFBundleIconFile).
cp "$WORK/bin/ibercorreo" "$APP/Contents/MacOS/$APP_TITLE"
cp "$ROOT/assets/app-icon/ibercorreo.icns" "$APP/Contents/Resources/$APP_TITLE.icns"
cp "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/NOTICE" "$APP/Contents/Resources/"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" -e "s/@BUILD_SHA@/$BUILD_SHA/g" \
  "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"

# Sign inside-out: the executable, then the bundle (no --deep on the final signature).
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/$APP_TITLE"
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

# ---- DMG ---------------------------------------------------------------------------------------
# A plain window (the app and an Applications link); a Tinta background like the other four apps'
# needs a .DS_Store layout written on a Mac, which comes later.
echo "==> building $DMG"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/$APP_TITLE.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
# `hdiutil create -srcfolder`, not `makehybrid`: makehybrid leaves Finder info on every file, which
# `codesign --verify --strict` then rejects. `create` attaches a device while it copies, which
# fails now and then ("Resource busy"), so it gets a few tries.
hdiutil_retry() {
  local what="$1" attempt
  shift
  for attempt in 1 2 3 4 5; do
    if hdiutil "$@"; then
      return 0
    fi
    [[ "$attempt" = 5 ]] && { echo "hdiutil $what failed 5 times" >&2; return 1; }
    echo "warning: hdiutil $what failed (attempt $attempt of 5); retrying in $((attempt * 10))s" >&2
    sleep $((attempt * 10))
  done
}
hdiutil_retry create create -srcfolder "$STAGE" -volname "$APP_TITLE" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"
sign "$DMG"
codesign --verify --strict --verbose=2 "$DMG"
# The app inside the image must still pass --strict.
MOUNT="$WORK/dmg-check"
rm -rf "$MOUNT"
mkdir -p "$MOUNT"
hdiutil_retry attach attach -nobrowse -readonly -mountpoint "$MOUNT" "$DMG" >/dev/null
if ! codesign --verify --strict --deep --verbose=2 "$MOUNT/$APP_TITLE.app"; then
  hdiutil_retry detach detach "$MOUNT" >/dev/null || true
  exit 1
fi
hdiutil_retry detach detach "$MOUNT" >/dev/null

# ---- CLI ---------------------------------------------------------------------------------------
echo "==> building $CLI_ZIP"
CLI_DIR="$WORK/ibercorreo-cli-$VERSION-macos-$ARCH"
mkdir -p "$CLI_DIR"
cp "$WORK/bin/ibercorreo-cli" "$CLI_DIR/"
cp "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/NOTICE" "$ROOT/README.md" "$CLI_DIR/"
sign --options runtime "$CLI_DIR/ibercorreo-cli"
codesign --verify --strict --verbose=2 "$CLI_DIR/ibercorreo-cli"
rm -f "$CLI_ZIP"
ditto -c -k --keepParent "$CLI_DIR" "$CLI_ZIP"

"$WORK/bin/ibercorreo-cli" --version
echo "==> done"
ls -lh "$DMG" "$CLI_ZIP"
