#!/usr/bin/env bash
# Build an AppImage with linuxdeploy + its GTK plugin.
# Run on the OLDEST distribution you want to support (CI uses Ubuntu 24.04).
# Needs: cargo, libgtk-4-dev, libadwaita-1-dev, librsvg2-dev, wget, file, patchelf
set -euo pipefail
cd "$(dirname "$0")/../.."
ARCH=${ARCH:-$(uname -m)}
ID=io.github.linux_application_remover
OUT=${OUT:-dist}
mkdir -p "$OUT" build/appimage && cd build/appimage

cargo build --release --manifest-path ../../Cargo.toml
for f in linuxdeploy-$ARCH.AppImage linuxdeploy-plugin-gtk.sh; do
  [ -f "$f" ] || wget -q "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/$f" 2>/dev/null || true
done
[ -f linuxdeploy-plugin-gtk.sh ] || wget -q https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh
chmod +x linuxdeploy-* || true

rm -rf AppDir
export DEPLOY_GTK_VERSION=4
export LINUXDEPLOY_OUTPUT_VERSION=${VERSION:-0.1.0}
export APPIMAGE_EXTRACT_AND_RUN=1
./linuxdeploy-$ARCH.AppImage \
  --appdir AppDir \
  --executable ../../target/release/linux-application-remover \
  --desktop-file ../../packaging/$ID.desktop \
  --icon-file ../../packaging/icons/$ID.svg \
  --plugin gtk \
  --output appimage
mv Linux_Application_Remover*.AppImage "../../$OUT/linux-application-remover-${LINUXDEPLOY_OUTPUT_VERSION}-$ARCH.AppImage"
