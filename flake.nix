{
  description = "Nekodex cross-device monitoring application";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, rust-overlay, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      reactSource = pkgs.fetchurl {
        url = "https://registry.npmjs.org/react/-/react-19.3.0.tgz";
        hash = "sha512-E8LUcbtBWt20bbl2YoHfx4ZDBdxVTfOKtCZn9cDSJ4l6/nuoApcpIBcj47t2wZoVX8g2ZHuMHbiShgCR1T5Sog==";
      };
      reactDomSource = pkgs.fetchurl {
        url = "https://registry.npmjs.org/react-dom/-/react-dom-19.3.0.tgz";
        hash = "sha512-JDk8dgif51OjFoDE70+OT9ICyYr+69HlmihNwp1+Nsfbna3t5sIiCa9ZJktDmQ4/1b/rn26hIAR2uYXDMr5r0Q==";
      };
      schedulerSource = pkgs.fetchurl {
        url = "https://registry.npmjs.org/scheduler/-/scheduler-0.28.0.tgz";
        hash = "sha512-juorfCmIkIw8tT+p5BXSm6PJjQF/ycEYmKyzURCIt/RaZIhL+PulbQ9Yu2z1HdOJDdqDTlxA1+xKBmHXJsczAw==";
      };
      frontendSource = pkgs.lib.sources.sourceByGlobs ./apps/ui [
        "index.html"
        "package.json"
        "package-lock.json"
        "src/**"
        "styles.css"
      ];
      packageSource = pkgs.lib.sources.sourceByGlobs ./. [
        "Cargo.lock"
        "Cargo.toml"
        "LICENSE"
        "apps/**"
        "crates/**"
        "generated/**"
        "plugins/**"
        "tools/build-frontend.mjs"
      ];
      frontend = pkgs.stdenvNoCC.mkDerivation {
        pname = "nekodex-frontend";
        version = "0.1.0";
        src = frontendSource;
        nativeBuildInputs = [ pkgs.esbuild pkgs.gnutar pkgs.gzip ];
        buildPhase = ''
          runHook preBuild
          mkdir -p node_modules/react node_modules/react-dom node_modules/scheduler
          tar -xzf ${reactSource} --strip-components=1 -C node_modules/react
          tar -xzf ${reactDomSource} --strip-components=1 -C node_modules/react-dom
          tar -xzf ${schedulerSource} --strip-components=1 -C node_modules/scheduler
          esbuild src/main.tsx \
            --bundle \
            --minify \
            --format=iife \
            --target=chrome108 \
            --jsx=automatic \
            --define:process.env.NODE_ENV='"production"' \
            --outfile=app.js
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          install -Dm644 index.html "$out/index.html"
          install -Dm644 styles.css "$out/styles.css"
          install -Dm644 app.js "$out/app.js"
          runHook postInstall
        '';
      };
      desktopItem = pkgs.makeDesktopItem {
        name = "nekodex";
        desktopName = "Nekodex";
        genericName = "Codex Cross-device Monitor";
        comment = "Monitor trusted Codex source computers on the local network";
        exec = "nekodex-shell";
        icon = "nekodex";
        categories = [ "Development" ];
        terminal = false;
      };
      nekodex = pkgs.rustPlatform.buildRustPackage {
        pname = "nekodex";
        version = "0.1.0";
        src = packageSource;
        cargoLock.lockFile = ./Cargo.lock;
        buildAndTestSubdir = "apps/shell/src-tauri";
        NEKODEX_FRONTEND_PREBUILT = "1";

        postPatch = ''
          rm -rf apps/ui/dist
          ln -s ${frontend} apps/ui/dist
        '';

        nativeBuildInputs = with pkgs; [
          pkg-config
          wrapGAppsHook4
        ];
        buildInputs = with pkgs; [
          glib-networking
          libsoup_3
          openssl
          webkitgtk_4_1
        ];

        # Focused tests run in development; the package build proves the
        # sandboxed release artifact without repeating the whole workspace.
        doCheck = false;

        postInstall = ''
          rm -f "$out/lib/libnekodex_shell_lib.a" "$out/lib/libnekodex_shell_lib.so"
          rmdir --ignore-fail-on-non-empty "$out/lib"
          install -Dm644 ${./apps/shell/src-tauri/icons/icon.svg} "$out/share/icons/hicolor/scalable/apps/nekodex.svg"
          install -Dm644 ${desktopItem}/share/applications/nekodex.desktop \
            "$out/share/applications/nekodex.desktop"
          install -Dm644 ${./LICENSE} "$out/share/licenses/nekodex/LICENSE"
        '';

        meta = with pkgs.lib; {
          description = "Decentralized local-network Codex monitor";
          license = licenses.mit;
          mainProgram = "nekodex-shell";
          platforms = platforms.linux;
        };
      };
    in {
      packages.${system} = {
        inherit frontend nekodex;
        default = nekodex;
      };

      apps.${system}.default = {
        type = "app";
        program = "${pkgs.lib.getExe nekodex}";
      };
    };
}
