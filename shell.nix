# The Android build environment: the SDK pieces an APK needs (platform 34, build-tools, the NDK's clang as the linker),
# a JDK for apksigner/keytool, adb, and rustup for a Rust with the aarch64-linux-android standard library.
# Use it as `nix-shell shell.nix --run '...'` (a plain shell.nix: nothing of the project is copied to the store).
{ pkgs ? import <nixpkgs> { config = { android_sdk.accept_license = true; allowUnfree = true; }; } }:
let
  ndkVersion = "27.3.13750724";
  android = pkgs.androidenv.composeAndroidPackages {
    platformVersions = [ "34" ];
    buildToolsVersions = [ "35.0.0" ];
    includeNDK = true;
    ndkVersions = [ ndkVersion ];
    includeEmulator = false;
    includeSystemImages = false;
    includeSources = false;
  };
  sdk = "${android.androidsdk}/libexec/android-sdk";
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [ rustup jdk17_headless android-tools zip unzip ];
  ANDROID_HOME = sdk;
  ANDROID_SDK_ROOT = sdk;
  ANDROID_NDK_ROOT = "${sdk}/ndk/${ndkVersion}";
  BUILD_TOOLS = "${sdk}/build-tools/35.0.0";
  ANDROID_JAR = "${sdk}/platforms/android-34/android.jar";
}
