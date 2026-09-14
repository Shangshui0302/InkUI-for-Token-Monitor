{
  description = "Codex session, account quota, and BOOX monitor";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      source = pkgs.lib.fileset.toSource {
        root = ./.;
        fileset = pkgs.lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./crates
          ./web
          (pkgs.lib.fileset.maybeMissing ./packaging/linux)
        ];
      };
      mkBinaries = rustPkgs: rustPkgs.rustPlatform.buildRustPackage {
        pname = "codex-monitor-node";
        version = "0.1.0";
        src = source;
        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = [ "--workspace" "--bins" ];

        installPhase = ''
          install -Dm755 target/${rustPkgs.stdenv.hostPlatform.rust.rustcTarget}/release/codex-monitor-server $out/bin/codex-monitor-node
          install -Dm755 target/${rustPkgs.stdenv.hostPlatform.rust.rustcTarget}/release/codex-monitor-agent $out/bin/codex-monitor-agent
        '';
      };
      codex-monitor-node = mkBinaries pkgs;
      codex-monitor-static-binaries = (mkBinaries pkgs.pkgsStatic).overrideAttrs (_: {
        pname = "codex-monitor-static-binaries";
      });
      codex-monitor-linux-x86_64 = pkgs.runCommand "codex-monitor-linux-x86_64-0.1.0" {
        nativeBuildInputs = [ pkgs.gnutar pkgs.gzip ];
      } ''
        mkdir -p "$out" staging/bin staging/systemd/user staging/scripts
        install -m755 ${codex-monitor-static-binaries}/bin/codex-monitor-node staging/bin/
        install -m755 ${codex-monitor-static-binaries}/bin/codex-monitor-agent staging/bin/
        install -m644 ${source}/packaging/linux/codex-monitor.service staging/systemd/user/
        install -m755 ${source}/packaging/linux/install.sh staging/scripts/
        install -m755 ${source}/packaging/linux/uninstall.sh staging/scripts/
        install -m644 ${source}/packaging/linux/README.md staging/
        tar -czf "$out/codex-monitor-linux-x86_64.tar.gz" -C staging .
        cp -a staging/. "$out/"
      '';
    in {
      packages.${system} = {
        codex-monitor-node = codex-monitor-node;
        codex-monitor-linux-x86_64 = codex-monitor-linux-x86_64;
        default = codex-monitor-node;
      };

      nixosModules.codex-monitor = { config, lib, pkgs, ... }:
        import ./packaging/nixos/module.nix {
          inherit config lib pkgs;
          defaultPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.codex-monitor-node;
        };

      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          clippy
          git
          jq
          nodejs
          rustc
          rustfmt
        ];

        shellHook = ''
          export CARGO_TERM_COLOR=always
          export RUST_BACKTRACE=1
        '';
      };
    };
}
