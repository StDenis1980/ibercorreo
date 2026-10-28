#!/usr/bin/env bash
# Build and package Tinta Correo for Linux (x86_64 or aarch64):
#
#   $DIST/tinta-correo-<version>-linux-<arch>.AppImage  any distro with glibc >= the build's
#   $DIST/tinta-correo-<version>-linux-<arch>.deb       Debian, Ubuntu, Mint, Pop!_OS, ...
#   $DIST/tinta-correo-<version>-linux-<arch>.rpm       Fedora, openSUSE, RHEL, ...
#   $DIST/tinta-correo-<version>-linux-<arch>.tar.gz    plain FHS-style tree (bin/, share/)
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar"]
#
# Build on an old distro (Ubuntu 22.04, glibc 2.35) so the binaries run on newer ones, or build
# anywhere with cargo-zigbuild and a glibc floor, then package with --skip-build:
#
#   cargo zigbuild --release --locked -p ibercorreo -p ibercorreo-cli --target x86_64-unknown-linux-gnu.2.35
#   CROSS_TARGET=x86_64-unknown-linux-gnu packaging/linux/package.sh --skip-build
#
# Gmail and Microsoft 365 sign-in need the OAuth client ids compiled in (docs/oauth.md): set
# IBERCORREO_GOOGLE_CLIENT_ID, IBERCORREO_GOOGLE_CLIENT_SECRET and IBERCORREO_MICROSOFT_CLIENT_ID
# before building. Without them the build works with password (IMAP/SMTP) accounts.
#
# Needs: cargo; nfpm for deb/rpm (https://nfpm.goreleaser.com); curl for appimagetool, which is
# downloaded (pinned) into $CARGO_TARGET_DIR when it is not on PATH. Optional:
# desktop-file-validate, appstreamcli.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/packaging/linux"
APP_ID=io.github.stdenis1980.ibercorreo
SLUG=tinta-correo
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
DIST="${DIST:-$ROOT/dist/release}"
mkdir -p "$DIST"

SKIP_BUILD=0
FORMATS="appimage deb rpm tar"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    -h | --help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
VERSION="${IBERCORREO_VERSION:-$(awk '
  /^\[/ { inpkg = ($0 == "[workspace.package]"); next }
  inpkg && $1 == "version" { v = $3; gsub(/"/, "", v); print v; exit }
' "$ROOT/Cargo.toml")}"
[[ -n "$VERSION" ]] || { echo "error: no [workspace.package] version in Cargo.toml" >&2; exit 1; }
BUILD_DATE="$(date -u +%Y-%m-%d)"

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
BASENAME="$SLUG-$VERSION-linux-$ARCH"
echo "==> Tinta Correo $VERSION for Linux $ARCH ($FORMATS)"
for v in IBERCORREO_GOOGLE_CLIENT_ID IBERCORREO_MICROSOFT_CLIENT_ID; do
  if [[ "$SKIP_BUILD" = 0 && -z "${!v:-}" ]]; then
    echo "warning: $v is not set; that sign-in will be missing from this build" >&2
  fi
done

if [[ "$SKIP_BUILD" = 0 ]]; then
  (cd "$ROOT" && cargo build --release --locked -p ibercorreo -p ibercorreo-cli ${CROSS_TARGET:+--target "$CROSS_TARGET"})
fi
BIN="$CARGO_TARGET_DIR/${CROSS_TARGET:+$CROSS_TARGET/}release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (shared by every format) -------------------------------------------------
install -Dm755 "$BIN/ibercorreo" "$STAGE/usr/bin/ibercorreo"
install -Dm755 "$BIN/ibercorreo-cli" "$STAGE/usr/bin/ibercorreo-cli"
strip "$STAGE/usr/bin/ibercorreo" "$STAGE/usr/bin/ibercorreo-cli" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
DOC="$STAGE/usr/share/doc/$SLUG"
mkdir -p "$DOC"
cp "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/NOTICE" "$ROOT/README.md" "$DOC/"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
fi

has() {
  case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac
}

verify_sha256() {
  printf '%s  %s\n' "$1" "$2" | sha256sum -c -
}

# ---- .tar.gz ------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- .deb / .rpm --------------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/docs/install/)" >&2; exit 1; }
  export VERSION RELEASE=1 NFPM_ARCH="$DEB_ARCH"
  # nfpm expands env vars in fields like `version` and `arch`, but not in `contents[].src`.
  sed "s|\${STAGE}|$STAGE|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
  for fmt in deb rpm; do
    if has "$fmt"; then (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"); fi
  done
fi

# ---- AppImage -----------------------------------------------------------------------------------
if has appimage; then
  APPIMAGETOOL_VERSION=1.9.1
  APPIMAGE_RUNTIME_VERSION=20251108
  case "$ARCH" in
    x86_64)
      APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
      APPIMAGE_RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
    aarch64)
      APPIMAGETOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
      APPIMAGE_RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
  esac
  APPDIR="$WORK/$SLUG.AppDir"
  cp -R "$STAGE" "$APPDIR"
  ln -s usr/bin/ibercorreo "$APPDIR/AppRun"
  cp "$STAGE/usr/share/applications/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$STAGE/usr/share/icons/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [[ -z "$TOOL" ]]; then
    TOOL="$CARGO_TARGET_DIR/appimagetool-$APPIMAGETOOL_VERSION-$ARCH.AppImage"
    if [[ ! -x "$TOOL" ]]; then
      curl --proto '=https' -fsSL -o "$TOOL.download" \
        "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-$ARCH.AppImage"
      verify_sha256 "$APPIMAGETOOL_SHA256" "$TOOL.download"
      chmod 755 "$TOOL.download"
      mv "$TOOL.download" "$TOOL"
    fi
    verify_sha256 "$APPIMAGETOOL_SHA256" "$TOOL"
  fi
  # appimagetool otherwise fetches the unpinned type2-runtime `continuous` build and embeds it.
  RUNTIME="$CARGO_TARGET_DIR/appimage-runtime-$APPIMAGE_RUNTIME_VERSION-$ARCH"
  if [[ ! -f "$RUNTIME" ]]; then
    curl --proto '=https' -fsSL -o "$RUNTIME.download" \
      "https://github.com/AppImage/type2-runtime/releases/download/$APPIMAGE_RUNTIME_VERSION/runtime-$ARCH"
    verify_sha256 "$APPIMAGE_RUNTIME_SHA256" "$RUNTIME.download"
    mv "$RUNTIME.download" "$RUNTIME"
  fi
  verify_sha256 "$APPIMAGE_RUNTIME_SHA256" "$RUNTIME"
  OUT="$(cd "$DIST" && pwd)/$BASENAME.AppImage"
  APPDIR="$(cd "$APPDIR" && pwd)"
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  RUNTIME="$(cd "$(dirname "$RUNTIME")" && pwd)/$(basename "$RUNTIME")"
  # Extract-and-run: works without FUSE (containers, CI). The output embeds the static runtime,
  # so users don't need libfuse2 either.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream --runtime-file "$RUNTIME" "$APPDIR" "$OUT")
  echo "wrote $OUT"
fi

if [[ -z "${CROSS_TARGET:-}" || "$CROSS_TARGET" = "$(uname -m)-unknown-linux-gnu" ]]; then
  "$STAGE/usr/bin/ibercorreo-cli" --version
fi
echo "==> done"
ls -lh "$DIST"
