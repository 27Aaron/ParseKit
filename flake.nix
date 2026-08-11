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
              # Rust toolchain; keep aligned with Cargo.toml and CI.
              rustc
              cargo
              clippy
              rustfmt
              rust-analyzer

              # Cargo tools used during development.
              cargo-audit
              cargo-edit
              cargo-nextest
              cargo-watch

              # Native dependencies, media tooling, and diagnostics.
              pkg-config
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
