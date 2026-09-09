{
  description = "Codex session, account quota, and BOOX monitor";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in {
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
