{
  description = "Codex cross-device workbench P0 development environment";

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
        config = {
          allowUnfree = true;
          android_sdk.accept_license = true;
        };
      };
      rust = pkgs.rust-bin.stable.latest.default.override {
        targets = [
          "aarch64-linux-android"
          "armv7-linux-androideabi"
          "i686-linux-android"
          "x86_64-linux-android"
        ];
      };
      android = pkgs.androidenv.composeAndroidPackages {
        platformVersions = [ "35" "36" ];
        buildToolsVersions = [ "35.0.0" "36.0.0" ];
        includeNDK = true;
        ndkVersions = [ "28.0.13004108" ];
        includeEmulator = false;
        includeSystemImages = false;
      };
      desktopPackages = with pkgs; [
        rust
        cargo-tauri
        nodejs
        pkg-config
        webkitgtk_4_1
        libsoup_3
        openssl
      ];
    in {
      devShells.${system} = {
        default = pkgs.mkShell {
          packages = desktopPackages;
        };

        android = pkgs.mkShell {
          packages = with pkgs; [
            jdk17_headless
            android.androidsdk
          ] ++ desktopPackages;
          ANDROID_HOME = "${android.androidsdk}/libexec/android-sdk";
          NDK_HOME = "${android.androidsdk}/libexec/android-sdk/ndk/28.0.13004108";
          JAVA_HOME = "${pkgs.jdk17_headless}/lib/openjdk";
        };
      };
    };
}
