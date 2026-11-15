# Maintainer: VRDoctor contributors
pkgname=vrdoctor
pkgver=0.11.5
pkgrel=1
pkgdesc="GUI + CLI launcher for PCVR games on Meta Quest 3 via WiVRn / ALVR, with custom non-store games"
arch=('x86_64')
url="https://example.invalid/vrdoctor"
license=('MIT')
depends=('steam' 'android-tools' 'avahi' 'pipewire' 'libxkbcommon' 'libxkbcommon-x11' 'libxcb' 'libxcursor')
makedepends=('cargo')
optdepends=('wivrn-dashboard: recommended WiVRn backend'
            'alvr: SteamVR streaming fallback'
            'wine: fallback for Windows custom games without Proton')
source=()
build() { cargo build --release --locked 2>/dev/null || cargo build --release; }
package() {
  install -Dm755 "target/release/vrdoctor" "$pkgdir/usr/bin/vrdoctor"
  install -Dm755 "target/release/vrdoctor-vr" "$pkgdir/usr/bin/vrdoctor-vr"
  install -Dm644 "vrdoctor.desktop" "$pkgdir/usr/share/applications/vrdoctor.desktop"
  install -Dm644 "vrdoctor-vr.desktop" "$pkgdir/usr/share/applications/vrdoctor-vr.desktop"
}
