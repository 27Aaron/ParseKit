{
  description = "Rust development environment for ParseKit";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = {nixpkgs, ...}: let
    systems = [
      "aarch64-darwin"
      "aarch64-linux"
      "x86_64-linux"
    ];

    forEachSystem = nixpkgs.lib.genAttrs systems;
  in {
    devShells = forEachSystem (
      system: let
        pkgs = import nixpkgs {inherit system;};
        inherit (pkgs) lib;

        platformPackages =
          lib.optionals pkgs.stdenv.isDarwin [pkgs.libiconv];
      in {
        default = pkgs.mkShell {
          packages =
            (with pkgs; [
              # Rust toolchain and editor support (nixpkgs stable channel via flake.lock).
              # Keep in sync with Cargo.toml rust-version / CI stable when practical.
              rustc
              cargo
              clippy
              rustfmt
              rust-analyzer

              # Common Cargo development tools.
              cargo-audit
              cargo-edit
              cargo-nextest
              cargo-watch

              # Native deps, media probe, and diagnostics.
              pkg-config
              openssl
              ffmpeg-headless
              cacert
              git
              curl
              jq
              alejandra
            ])
            ++ platformPackages;

          RUST_BACKTRACE = "1";
          SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
        };
      }
    );

    formatter = forEachSystem (system: nixpkgs.legacyPackages.${system}.alejandra);
  };
}
