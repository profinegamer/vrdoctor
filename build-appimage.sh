#!/usr/bin/env bash
# Build VRDoctor-x86_64.AppImage.
#
# The AppImage contains the vrdoctor binary plus exactly two bundled
# libraries: libxkbcommon and libxkbcommon-x11. Those are self-contained
# keyboard libraries that winit dlopens by name and that are NOT present
# on every system (a bare system panics with "Library libxkbcommon-x11.so
# could not be loaded"). Everything else — libX11, libxcb, libwayland,
# libGL/EGL — is the host's own graphics stack, which must match the
# host's GPU driver and compositor: bundling foreign copies of those and
# prepending them via LD_LIBRARY_PATH broke window creation on real
# machines in v0.7.0 (glutin: "Found no glutin configs matching the
# template" / "provided display handle is not supported"). On first run (no ~/.config/vrdoctor/
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
# vrdoctor-vr, the in-headset UI — must sit next to vrdoctor so
# `vrdoctor vr` finds it (and so the AppImage's `vr` subcommand works).
install -m755 target/release/vrdoctor-vr "$APPDIR/usr/bin/vrdoctor-vr"
install -m644 vrdoctor.desktop "$APPDIR/vrdoctor.desktop"
install -m644 vrdoctor.desktop "$APPDIR/usr/share/applications/vrdoctor.desktop"
install -m644 vrdoctor-vr.desktop "$APPDIR/usr/share/applications/vrdoctor-vr.desktop"

# Icon (generated at build time by make-icon.py, committed as vrdoctor.png if present).
if [ ! -f vrdoctor.png ]; then python3 make-icon.py; fi
install -m644 vrdoctor.png "$APPDIR/vrdoctor.png"
install -m644 vrdoctor.png "$APPDIR/usr/share/icons/hicolor/512x512/apps/vrdoctor.png"

# Bundle ONLY the xkbcommon pair (see the header comment). The build
# cache's libs/ dir is a fallback source on machines where the -x11
# library is not installed system-wide.
LIBCACHE="${XDG_CACHE_HOME:-$HOME/.cache}/vrdoctor-build/libs"
for lib in libxkbcommon.so.0 libxkbcommon-x11.so.0; do
  for d in /usr/lib/x86_64-linux-gnu /usr/lib64 /usr/lib "$LIBCACHE"; do
    if [ -e "$d/$lib" ]; then cp -L "$d/$lib" "$APPDIR/usr/lib/" && break; fi
  done
  # winit's xkbcommon-dl may dlopen the unversioned name too.
  (cd "$APPDIR/usr/lib" && ln -sf "$lib" "${lib%.0}")
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
