{
  description = "starkit — the terminal-UI foundation shared by staramp and starcord";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    # Explicit rather than eachDefaultSystem, which would also claim systems
    # nobody has built this on. The same three as staramp, because the point of
    # this crate is that both applications build everywhere either of them
    # does; x86_64-darwin is absent because nixpkgs 26.11 dropped it, and
    # naming it fails evaluation with a release note rather than a build error.
    flake-utils.lib.eachSystem [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # One version, read rather than repeated.
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      in
      {
        # There is nothing to install -- this is a library. The package exists
        # so `nix flake check` compiles and tests the crate, which
        # buildRustPackage does as part of building it.
        #
        # `--all-features` both ways: a feature that only its own application
        # turns on is exactly the one that rots, so the flake build is the
        # place that always has every one of them on.
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "starkit";
          version = cargoToml.package.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          cargoBuildFlags = [ "--all-features" ];
          cargoTestFlags = [ "--all-features" ];
          doCheck = true;

          meta = with pkgs.lib; {
            description = "The terminal-UI foundation shared by staramp and starcord";
            homepage = "https://github.com/bstar/starkit";
            license = licenses.mit;
            platforms = platforms.linux ++ platforms.darwin;
          };
        };

        checks = {
          inherit (self.packages.${system}) default;

          fmt = pkgs.runCommand "cargo-fmt"
            { nativeBuildInputs = [ pkgs.rustfmt ]; }
            ''
              cd ${./.}
              find src -name '*.rs' -print0 \
                | xargs -0 rustfmt --check --edition 2021
              touch $out
            '';
        };

        formatter = pkgs.nixpkgs-fmt;

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            # The licence and advisory gate, so it can be answered here rather
            # than only in CI.
            cargo-deny
          ];

          shellHook = ''
            echo "starkit devshell · rustc $(rustc --version | cut -d' ' -f2)"
          '';
        };
      });
}
