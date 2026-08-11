{
  description = "Rust development environment for the Parse library";

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
              # Rust toolchain and editor support.
              # MSRV is 1.88 (see Cargo.toml); nixpkgs may be newer, which is fine.
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

          shellHook = ''
            echo "parse-core dev shell: $(rustc --version) | ffprobe=$(command -v ffprobe >/dev/null && echo ok || echo missing)"
          '';
        };
      }
    );

    formatter = forEachSystem (system: nixpkgs.legacyPackages.${system}.alejandra);
  };
}
