#!/usr/bin/env bash
set -euo pipefail
# Build + install VRDoctor, then print backend setup for your distro.
# GUI build needs X11/Wayland dev headers on some systems:
#   Arch:   (usually already present with base-devel / vulkan deps)
#   Fedora: sudo dnf install libXcursor-devel libXi-devel libXrandr-devel libxkbcommon-devel wayland-devel
cd "$(dirname "$0")"
cargo build --release
install -Dm755 target/release/vrdoctor "$HOME/.local/bin/vrdoctor"
install -Dm644 vrdoctor.desktop "$HOME/.local/share/applications/vrdoctor.desktop" 2>/dev/null || true
echo "Installed: $HOME/.local/bin/vrdoctor"
echo "Now run:  vrdoctor          # GUI (dark mode by default)"
echo "          vrdoctor setup    # Arch/Fedora backend install steps"
echo "          vrdoctor doctor"
