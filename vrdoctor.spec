Name:           vrdoctor
Version:        0.10.2
Release:        1%{?dist}
Summary:        GUI + CLI launcher for PCVR games on Meta Quest 3 via WiVRn / ALVR, with custom non-store games
License:        MIT
BuildRequires:  cargo, libxkbcommon-devel, libXcursor-devel, libXi-devel, libXrandr-devel, wayland-devel
Requires:       steam, android-tools, avahi, pipewire, libxkbcommon-x11, libXcursor
Recommends:     wivrn

%description
VRDoctor detects your distro, GPU, Steam and headset, starts the WiVRn
(recommended) or ALVR backend correctly, and launches Steam PCVR games —
plus custom games that are not on any store — to a Meta Quest 3.
Includes a GUI that defaults to dark mode.

%build
cargo build --release

%install
install -Dm755 target/release/vrdoctor %{buildroot}%{_bindir}/vrdoctor
install -Dm755 target/release/vrdoctor-vr %{buildroot}%{_bindir}/vrdoctor-vr
install -Dm644 vrdoctor.desktop %{buildroot}%{_datadir}/applications/vrdoctor.desktop
install -Dm644 vrdoctor-vr.desktop %{buildroot}%{_datadir}/applications/vrdoctor-vr.desktop

%files
%{_bindir}/vrdoctor
%{_bindir}/vrdoctor-vr
%{_datadir}/applications/vrdoctor.desktop
%{_datadir}/applications/vrdoctor-vr.desktop
