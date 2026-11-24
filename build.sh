#!/usr/bin/env bash
# Build the Memrium APK without Gradle: cargo builds the native library, aapt2 makes the package from the manifest and
# the theme, zipalign and apksigner finish it. `--install` puts it on the phone over adb and starts it.
#
#   nix-shell shell.nix --run './build.sh --install'
#
# The rustup toolchain needs the Android standard library once: `rustup target add aarch64-linux-android`.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

# the oldest Android the library runs on (minSdkVersion in the manifest)
api=30
bin="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/bin"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$bin/aarch64-linux-android${api}-clang"
export CC_aarch64_linux_android="$bin/aarch64-linux-android${api}-clang"
export AR_aarch64_linux_android="$bin/llvm-ar"

cargo build --release --target aarch64-linux-android

out="$here/build"
rm -rf "$out"
mkdir -p "$out/apk/lib/arm64-v8a"
cp target/aarch64-linux-android/release/libmemrium_mobile.so "$out/apk/lib/arm64-v8a/"
# icons of the Memrium theme for the apps listed in icons.map (the launcher reads assets/icons/<package>.png)
theme="${MEMRIUM_ICONS:-$HOME/.local/share/icons/Memrium-Icons}"
mkdir -p "$out/assets/icons"
while read -r pkg name; do
    case "$pkg" in "" | \#*) continue ;; esac
    src="$theme/256x256/apps/$name.png"
    if [ -f "$src" ]; then cp "$src" "$out/assets/icons/$pkg.png"; else echo "icons.map: no $src for $pkg" >&2; fi
done < icons.map
"$BUILD_TOOLS/aapt2" compile --dir res -o "$out/res.zip"
"$BUILD_TOOLS/aapt2" link -o "$out/base.apk" -I "$ANDROID_JAR" --manifest AndroidManifest.xml -A "$out/assets" "$out/res.zip"
(cd "$out/apk" && zip -qr "$out/base.apk" lib)
"$BUILD_TOOLS/zipalign" -p -f 4 "$out/base.apk" "$out/aligned.apk"

# a debug key of this checkout, made once
ks="$here/debug.keystore"
if [ ! -f "$ks" ]; then
    keytool -genkeypair -keystore "$ks" -storepass android -keypass android -alias debug -keyalg RSA -keysize 2048 \
        -validity 10000 -dname "CN=Memrium Debug" >/dev/null
fi
java -jar "$BUILD_TOOLS/lib/apksigner.jar" sign --ks "$ks" --ks-pass pass:android --out "$out/memrium.apk" "$out/aligned.apk"
ls -la "$out/memrium.apk"

if [ "${1:-}" = "--install" ]; then
    adb install -r "$out/memrium.apk"
    adb shell am start -n ua.memrium.mobile/android.app.NativeActivity
fi
