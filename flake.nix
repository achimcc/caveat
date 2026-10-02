{
  description = "Shows a repository's known pitfall at the moment its error message appears: a Claude Code hook over a directory of caveats";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  # The RustSec advisory database, pinned like any other input. The `audit`
  # check reads it offline; `nix flake update advisory-db` brings news in.
  inputs.advisory-db = {
    url = "github:rustsec/advisory-db";
    flake = false;
  };

  outputs =
    {
      self,
      nixpkgs,
      advisory-db,
    }:
    let
      # Tested on Linux only.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "caveat";
          # Read out of Cargo.toml so the store path and the crate cannot disagree.
          version = (nixpkgs.lib.importTOML ./Cargo.toml).package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # The hook skips uncommitted caveats (git status); its test needs git.
          nativeCheckInputs = [ pkgs.git ];
          meta = {
            description = "Shows a repository's known pitfall at the moment its error message appears: a Claude Code hook over a directory of caveats";
            homepage = "https://github.com/achimcc/caveat";
            license = pkgs.lib.licenses.agpl3Only;
            mainProgram = "caveat";
            platforms = pkgs.lib.platforms.linux;
          };
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
          ];
        };
      });

      checks = forAll (
        pkgs:
        let
          package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        in
        {
          inherit package;
          # Known advisories against Cargo.lock, read offline from the pinned
          # database.
          audit = pkgs.runCommand "caveat-audit" { nativeBuildInputs = [ pkgs.cargo-audit ]; } ''
            HOME=$TMPDIR cargo-audit audit --no-fetch --db ${advisory-db} --file ${./Cargo.lock}
            touch $out
          '';
          # Bans, sources and licenses of the dependency tree (deny.toml).
          # Inside the package's build environment: the vendored crates are
          # what `cargo metadata` reads there, so nothing is fetched.
          deny = package.overrideAttrs (old: {
            pname = "caveat-deny";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.cargo-deny ];
            buildPhase = "cargo deny --offline check bans sources licenses";
            doCheck = false;
            installPhase = "touch $out";
          });
          clippy = package.overrideAttrs (old: {
            pname = "caveat-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            buildPhase = "cargo clippy --all-targets -- -D warnings";
            doCheck = false;
            installPhase = "touch $out";
          });
          fmt = package.overrideAttrs (old: {
            pname = "caveat-fmt";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.rustfmt ];
            buildPhase = "cargo fmt --check";
            doCheck = false;
            installPhase = "touch $out";
          });
        }
      );
    };
}
