#!/usr/bin/env bash
# Build VRDoctor-x86_64.AppImage.
#
# The AppImage contains the vrdoctor binary plus the X11/Wayland runtime
# libraries it dlopens (xkbcommon, wayland-client, xcb, …) so it runs on
# Arch and Fedora desktops alike. On first run (no ~/.config/vrdoctor/
# installed-v1 marker) the app opens the VRDoctor installer, which asks
# "Arch or Fedora?" and installs the VR backend accordingly. Later runs
# open the main GUI. CLI use: ./VRDoctor-x86_64.AppImage doctor | install …
set -euo pipefail
cd "$(dirname "$0")"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release

APPDIR="$(pwd)/AppDir"
OUT="$(pwd)/VRDoctor-x86_64.AppImage"
rm -rf "$APPDIR" "$OUT"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/512x512/apps"

install -m755 target/release/vrdoctor "$APPDIR/usr/bin/vrdoctor"
install -m644 vrdoctor.desktop "$APPDIR/vrdoctor.desktop"
install -m644 vrdoctor.desktop "$APPDIR/usr/share/applications/vrdoctor.desktop"

# Icon (generated at build time by make-icon.py, committed as vrdoctor.png if present).
if [ ! -f vrdoctor.png ]; then python3 make-icon.py; fi
install -m644 vrdoctor.png "$APPDIR/vrdoctor.png"
install -m644 vrdoctor.png "$APPDIR/usr/share/icons/hicolor/512x512/apps/vrdoctor.png"

# Runtime libraries the binary dlopens at runtime (not shown by ldd).
for lib in libxkbcommon.so.0 libxkbcommon-x11.so.0 libxcb-xkb.so.1 libxkbfile.so.1 \
           libwayland-client.so.0 libffi.so.8 libxcb.so.1 libX11.so.6 libXext.so.6 \
           libXcursor.so.1 libXfixes.so.3 libXi.so.6 libXrandr.so.2; do
  for d in /usr/lib/x86_64-linux-gnu /usr/lib64 /usr/lib; do
    if [ -e "$d/$lib" ]; then cp -L "$d/$lib" "$APPDIR/usr/lib/" && break; fi
  done
done

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
# First run opens the installer (Arch or Fedora?); after that, the GUI.
# Any vrdoctor CLI command works too: ./VRDoctor-x86_64.AppImage doctor
exec "$HERE/usr/bin/vrdoctor" "$@"
EOF
chmod +x "$APPDIR/AppRun"

chmod -R a+rX "$APPDIR"

# appimagetool (downloaded once, extracted — no FUSE needed).
TOOL_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/vrdoctor-build"
mkdir -p "$TOOL_DIR"
if [ ! -x "$TOOL_DIR/squashfs-root/AppRun" ]; then
  curl -L -o "$TOOL_DIR/appimagetool.AppImage" \
    https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod +x "$TOOL_DIR/appimagetool.AppImage"
  (cd "$TOOL_DIR" && ./appimagetool.AppImage --appimage-extract >/dev/null)
fi
ARCH=x86_64 "$TOOL_DIR/squashfs-root/AppRun" --no-appstream "$APPDIR" "$OUT"
echo "Built: $OUT"
