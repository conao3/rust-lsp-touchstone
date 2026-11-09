{
  description = "A strict LSP client, server and checking proxy for testing Language Server Protocol implementations";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix.url = "github:numtide/treefmt-nix";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];

      imports = [ inputs.treefmt-nix.flakeModule ];

      perSystem =
        { pkgs, ... }:
        {
          packages.default = pkgs.rustPlatform.buildRustPackage {
            pname = "lsp-touchstone";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            meta = {
              description = "A strict LSP client, server and checking proxy for testing Language Server Protocol implementations";
              homepage = "https://github.com/conao3/rust-lsp-touchstone";
              license = pkgs.lib.licenses.asl20;
              mainProgram = "lsp-touchstone";
              platforms = pkgs.lib.platforms.unix;
            };
          };

          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              gnumake
              jq
              rust-analyzer
              rustc
              rustfmt
            ];
          };

          treefmt.programs = {
            nixfmt.enable = true;
            rustfmt.enable = true;
          };
        };
    };
}
