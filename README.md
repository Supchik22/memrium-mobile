# Memrium Mobile

The Memrium look on an Android phone: a home screen launcher drawn with [chebrium](https://github.com/Supchik22/chebrium)
(its software canvas in a `NativeActivity`, no Java code and no Gradle).

- a clock over the wallpaper and a glass dock; swipe up for the sheet of all apps (alphabetical grid), swipe down or
  Back to put it away; a tap starts the app
- the apps and their icons come from the system through JNI (`src/apps.rs`); icons get a glass edge
- icons of the Memrium icon theme can replace the apps' own (`icons.map`: Android package → theme icon name)
- the wallpaper and its frosted copy are painted once, the glass is a copy of rows, so a frame costs a few
  milliseconds on a mid-range phone (Helio G99) at 120 Hz

It can be chosen as the default home app (Settings → Apps → Default apps → Home app).

## Build

Needs rustup with `aarch64-linux-android`, the Android SDK (platform 34, build-tools) with an NDK, a JDK and `zip`.

```sh
rustup target add aarch64-linux-android
nix-shell shell.nix --run './build.sh --install'    # SDK and NDK from nixpkgs
# or, with an SDK already in ~/Android/Sdk:
source ./env-local.sh && ./build.sh --install
```

`build.sh` builds the native library, packs it with `aapt2`, signs the APK with a debug key made on the first build and,
with `--install`, puts it on the phone over `adb` and starts it. Phones from Xiaomi need "Install via USB" turned on
in the developer options.

## License

MIT
