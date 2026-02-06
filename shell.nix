{ pkgs ? (import (builtins.fetchTarball {
  url = "https://github.com/nixos/nixpkgs/tarball/25.05";
  sha256 = "1915r28xc4znrh2vf4rrjnxldw2imysz819gzhk9qlrkqanmfsxd";
}) {}) }:

pkgs.mkShell {
  name = "teams-cli";

  buildInputs = with pkgs; [
    # General tools
    just
    jq
    ripgrep
    fd
    gh

    # Rust development
    rustc
    cargo
    rust-analyzer
    clippy
    rustfmt

    # Audio (ALSA - needed for cpal audio I/O)
    alsa-lib

    # Video capture/display (V4L2, SDL2, openh264)
    v4l-utils
    linuxHeaders
    SDL2
    SDL2.dev
    nasm

    # Build tools
    gnumake
    cmake
    pkg-config
    openssl
    llvmPackages.libclang
    clang
  ];

  shellHook = ''
    export PKG_CONFIG_PATH="${pkgs.alsa-lib}/lib/pkgconfig:${pkgs.openssl.dev}/lib/pkgconfig:${pkgs.SDL2.dev}/lib/pkgconfig:$PKG_CONFIG_PATH"
    export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
    export BINDGEN_EXTRA_CLANG_ARGS="-I${pkgs.linuxHeaders}/include -I${pkgs.glibc.dev}/include"
  '';
}
