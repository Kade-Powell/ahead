Name:           ahead
Version:        0.0.1
Release:        1
Summary:        AI-native code editor written in Rust
License:        Apache-2.0
URL:            https://github.com/Kade-Powell/ahead

Source:        	{{{ git_dir_pack }}}

BuildRequires:  cargo libxkbcommon-x11-devel libxcb-devel vulkan-loader-devel wayland-devel openssl-devel pkgconf libxkbcommon-x11-devel

%description
AHEAD is an open source code editor written in Rust, forked from Lapce.
It uses rope-based text editing and a native GPUI and gpui-kit interface.

%prep
{{{ git_dir_setup_macro }}}
cargo fetch --locked

%build
cargo build --profile release-lto --package ahead-app --frozen

%install
install -Dm755 target/release-lto/ahead %{buildroot}%{_bindir}/ahead
install -Dm644 extra/linux/dev.ahead.ahead.desktop %{buildroot}/usr/share/applications/dev.ahead.ahead.desktop
install -Dm644 extra/linux/dev.ahead.ahead.metainfo.xml %{buildroot}/usr/share/metainfo/dev.ahead.ahead.metainfo.xml
install -Dm644 extra/images/logo.png %{buildroot}/usr/share/pixmaps/dev.ahead.ahead.png

%files
%license LICENSE*
%doc *.md
%{_bindir}/ahead
/usr/share/applications/dev.ahead.ahead.desktop
/usr/share/metainfo/dev.ahead.ahead.metainfo.xml
/usr/share/pixmaps/dev.ahead.ahead.png

%changelog
* Mon Sep 21 2026 Ahead Contributors
- See full changelog on GitHub
