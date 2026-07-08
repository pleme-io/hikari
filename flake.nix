{
  description = "hikari (光) — the pluggable syntax-highlighting spine";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  # hikari-core is zero-dependency std-only, so a plain
  # rustPlatform.buildRustPackage over the committed Cargo.lock builds it with
  # no crate2nix / gen ceremony. When the workspace grows dependency-heavy
  # members (hikari-ts over tree-sitter, etc.) this migrates to substrate's
  # rust-library-workspace-flake builder.
  outputs =
    { self, nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        hikari-core = pkgs.rustPlatform.buildRustPackage {
          pname = "hikari-core";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # Library workspace: build + test, nothing to install.
          doCheck = true;
        };
      in
      {
        packages = {
          inherit hikari-core;
          default = hikari-core;
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            pkgs.cargo
            pkgs.rustc
            pkgs.rustfmt
            pkgs.clippy
            pkgs.rust-analyzer
          ];
        };

        formatter = pkgs.nixpkgs-fmt;
      }
    );
}
