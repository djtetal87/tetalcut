#!/usr/bin/env bash
# TetalCut for this Mac in one command: build, bundle, sign (ad-hoc), make the DMG and install
# the app in /Applications. TetalCut is a fork of Concat (https://github.com/jub0t/Concat,
# AGPL-3.0-or-later) for AJ Media Studio's own use.
#
#   ./build-tetalcut.sh               build for this Mac's processor and install
#   ./build-tetalcut.sh --no-install  only build TetalCut.app and the DMG (in dist/)
#
# Needs Homebrew (https://brew.sh) and Rust (https://rustup.rs). The rest (FFmpeg, cmake,
# dylibbundler, the pinned Rust toolchain, sherpa-onnx and, on Intel, ONNX Runtime) is fetched
# here. The first build takes a long while (about an hour on an Intel Mac); later ones less.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL=1
[ "${1:-}" = "--no-install" ] && INSTALL=0
cd "$ROOT"

if ! command -v brew >/dev/null 2>&1; then
  echo "TetalCut needs Homebrew. Install it from https://brew.sh and run this again." >&2
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "TetalCut needs Rust. Install it from https://rustup.rs and run this again." >&2
  exit 1
fi

echo "==> Homebrew: ffmpeg, cmake, dylibbundler"
brew install ffmpeg cmake dylibbundler
export FFMPEG_DIR="$(brew --prefix ffmpeg)"

case "$(uname -m)" in
  arm64) ARCH=arm64; SHERPA=osx-arm64; export MACOSX_DEPLOYMENT_TARGET=12.0 ;;
  *) ARCH=x86_64; SHERPA=osx-x64; export MACOSX_DEPLOYMENT_TARGET=13.4 ;;
esac
# as Concat's own builds: the app also runs on older Macs than this one
export RUSTFLAGS="-C link-arg=$(xcrun clang --print-file-name=libclang_rt.osx.a)"
export GGML_NATIVE=OFF GGML_AVX2=OFF GGML_FMA=OFF GGML_F16C=OFF GGML_BMI2=OFF

DEPS="$ROOT/.deps"
mkdir -p "$DEPS"

echo "==> sherpa-onnx ($SHERPA)"
SHERPA_VERSION=$(grep -A1 'name = "sherpa-onnx-sys"' src/Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')
STEM="sherpa-onnx-v$SHERPA_VERSION-$SHERPA-static-lib"
if [ ! -d "$DEPS/sherpa/$STEM/lib" ]; then
  scripts/ci/fetch.sh "sherpa-onnx-$SHERPA-static-lib" "$DEPS/sherpa.tar.bz2"
  mkdir -p "$DEPS/sherpa"
  tar -xjf "$DEPS/sherpa.tar.bz2" -C "$DEPS/sherpa"
fi
export SHERPA_ONNX_LIB_DIR="$DEPS/sherpa/$STEM/lib"

if [ "$ARCH" = x86_64 ]; then
  # Intel: Microsoft's last ONNX Runtime for Intel Macs, linked dynamically and bundled
  ORT_VERSION=1.23.2
  ORT_STEM="onnxruntime-osx-x86_64-$ORT_VERSION"
  if [ ! -d "$DEPS/$ORT_STEM/lib" ]; then
    scripts/ci/fetch.sh onnxruntime-osx-x86_64 "$DEPS/onnxruntime.tgz"
    tar -xzf "$DEPS/onnxruntime.tgz" -C "$DEPS"
  fi
  ln -sf "libonnxruntime.$ORT_VERSION.dylib" "$DEPS/$ORT_STEM/lib/libonnxruntime.dylib"
  export ORT_LIB_LOCATION="$DEPS/$ORT_STEM/lib" ORT_PREFER_DYNAMIC_LINK=1 ORT_DIR="$DEPS/$ORT_STEM"
  # thin LTO: the same program, built in a fraction of the time
  export CARGO_PROFILE_APP_LTO=thin CARGO_PROFILE_APP_CODEGEN_UNITS=16
fi

echo "==> Building TetalCut ($ARCH); the first time takes a while"
(cd src && cargo build --profile app -p concat)

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' src/Cargo.toml | head -1)
OUT="$ROOT/dist"
APP="$OUT/TetalCut.app"
echo "==> Bundling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp src/target/app/concat "$APP/Contents/MacOS/concat"

ICONSET="$OUT/TetalCut.iconset"
rm -rf "$ICONSET"; mkdir -p "$ICONSET"
I=assets/icons
cp "$I/concat_logo_16.png" "$ICONSET/icon_16x16.png"
cp "$I/concat_logo_32.png" "$ICONSET/icon_16x16@2x.png"
cp "$I/concat_logo_32.png" "$ICONSET/icon_32x32.png"
cp "$I/concat_logo_64.png" "$ICONSET/icon_32x32@2x.png"
cp "$I/concat_logo_128.png" "$ICONSET/icon_128x128.png"
cp "$I/concat_logo_256.png" "$ICONSET/icon_128x128@2x.png"
cp "$I/concat_logo_256.png" "$ICONSET/icon_256x256.png"
cp "$I/concat_logo_512.png" "$ICONSET/icon_256x256@2x.png"
cp "$I/concat_logo_512.png" "$ICONSET/icon_512x512.png"
cp "$I/tetalcut_1024.png" "$ICONSET/icon_512x512@2x.png"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/TetalCut.icns"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>TetalCut</string>
  <key>CFBundleDisplayName</key><string>TetalCut</string>
  <key>CFBundleIdentifier</key><string>com.ajmediastudio.tetalcut</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>concat</string>
  <key>CFBundleIconFile</key><string>TetalCut</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>${MACOSX_DEPLOYMENT_TARGET}</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>NSHumanReadableCopyright</key><string>TetalCut, a fork of Concat (Jareer and Concat contributors). AGPL-3.0-or-later.</string>
  <key>NSMicrophoneUsageDescription</key><string>TetalCut records voiceovers from your microphone.</string>
</dict>
</plist>
PLIST
# the licence and notices travel with the app (AGPL)
cp LICENSE THIRD_PARTY_NOTICES.md "$APP/Contents/Resources/"

# every non-system library the app loads, copied in and re-pointed at Frameworks
dylibbundler -od -b -x "$APP/Contents/MacOS/concat" -d "$APP/Contents/Frameworks" \
  -p @executable_path/../Frameworks ${ORT_DIR:+-s "$ORT_DIR/lib"}
if otool -L "$APP/Contents/MacOS/concat" | grep -E '/opt/homebrew|/usr/local'; then
  echo "the app still loads libraries from outside the bundle" >&2
  exit 1
fi

echo "==> Signing (ad-hoc)"
codesign --force --deep -s - "$APP"

DMG="$OUT/TetalCut-$VERSION-macos-$ARCH.dmg"
rm -f "$DMG"
hdiutil create -volname TetalCut -srcfolder "$APP" -ov -format UDZO "$DMG" >/dev/null
echo "==> DMG: $DMG"

if [ "$INSTALL" = 1 ]; then
  echo "==> Installing in /Applications"
  rm -rf "/Applications/TetalCut.app"
  ditto "$APP" "/Applications/TetalCut.app"
  xattr -dr com.apple.quarantine "/Applications/TetalCut.app" 2>/dev/null || true
  echo "==> Done: open TetalCut from Applications or Launchpad."
fi
