{
  description = "Local Nix build shell for the Codex Monitor native Android peer node";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        config.allowUnfree = true;
      };
      android = pkgs.androidenv.composeAndroidPackages {
        includeEmulator = false;
        includeCmake = false;
        includeNDK = false;
        includeSystemImages = false;
        platformVersions = [ "35" ];
        buildToolsVersions = [ "35.0.0" ];
      };
      androidBuildSdk = pkgs.buildEnv {
        name = "codex-monitor-android-build-sdk";
        paths = android.build-tools ++ android.platforms;
        pathsToLink = [ "/libexec/android-sdk" ];
      };
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          androidBuildSdk
          jdk17_headless
          zip
        ];
        ANDROID_HOME = "${androidBuildSdk}/libexec/android-sdk";
        JAVA_HOME = "${pkgs.jdk17_headless}/lib/openjdk";
        ANDROID_COMPILE_SDK = "35";
        ANDROID_BUILD_TOOLS = "35.0.0";
      };
    };
}
